//! plico-mcp — MCP (Model Context Protocol) Server for Plico
//!
//! Thin JSON-RPC 2.0 adapter over Plico's kernel, communicating via stdio.
//! Exposes Plico CAS, search, and Knowledge Graph as MCP tools.
//!
//! Usage:
//!   PLICO_ROOT=/path/to/store cargo run --bin plico-mcp
//!
//! Claude Code config (~/.claude.json):
//!   { "mcpServers": { "plico": { "command": "cargo", "args": ["run", "--bin", "plico-mcp"],
//!     "env": { "PLICO_ROOT": "/tmp/plico-dogfood", "EMBEDDING_BACKEND": "stub" } } } }

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use plico::api::semantic::{ApiRequest, ApiResponse, ProcedureStepDto};
use plico::kernel::AIKernel;
use serde_json::Value;

const SERVER_NAME: &str = "plico-mcp";
const SERVER_VERSION: &str = "1.0.0";
const PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_AGENT: &str = "mcp-agent";

fn main() {
    let root = std::env::var("PLICO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp/plico"));

    let kernel = match AIKernel::new(root) {
        Ok(k) => Arc::new(k),
        Err(e) => {
            let err = make_error_response(Value::Null, -32603, &format!("kernel init failed: {e}"));
            let _ = writeln!(io::stdout(), "{}", serde_json::to_string(&err).unwrap());
            std::process::exit(1);
        }
    };

    // Seed 6 pre-installed skills on first MCP server start
    ensure_builtin_skills(&kernel);

    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();

    for line in stdin.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = make_error_response(Value::Null, -32700, &format!("parse error: {e}"));
                let _ = writeln!(stdout, "{}", serde_json::to_string(&resp).unwrap());
                continue;
            }
        };

        if msg.get("id").is_none() {
            continue;
        }

        let id = msg["id"].clone();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");

        let response = match method {
            "initialize" => handle_initialize(id),
            "tools/list" => handle_tools_list(id),
            "tools/call" => handle_tools_call(id, &msg["params"], &kernel),
            "ping" => make_result(id, serde_json::json!({})),
            _ => make_error_response(id, -32601, &format!("unknown method: {method}")),
        };

        let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap());
        let _ = stdout.flush();
    }
}

fn handle_initialize(id: Value) -> Value {
    make_result(id, serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        }
    }))
}

fn handle_tools_list(id: Value) -> Value {
    make_result(id, serde_json::json!({ "tools": tool_definitions() }))
}

fn handle_tools_call(id: Value, params: &Value, kernel: &AIKernel) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    let result = dispatch_tool(name, &args, kernel);
    match result {
        Ok(text) => make_result(id, serde_json::json!({
            "content": [{ "type": "text", "text": text }]
        })),
        Err(e) => make_result(id, serde_json::json!({
            "content": [{ "type": "text", "text": e }],
            "isError": true
        })),
    }
}

fn dispatch_tool(name: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    match name {
        "plico" => dispatch_plico(args, kernel),
        "plico_store" => dispatch_plico_store(args, kernel),
        "plico_skills" => dispatch_plico_skills(args, kernel),
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// Main gateway dispatcher for plico tool
fn dispatch_plico(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    // Check for pipeline mode first
    if args.get("pipeline").is_some() {
        return execute_pipeline(args, kernel);
    }

    // Single action mode
    let action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing required parameter: action")?;

    dispatch_plico_action(action, args, kernel)
}

/// Execute a pipeline of steps sequentially
fn execute_pipeline(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let pipeline = args.get("pipeline")
        .and_then(|p| p.as_array())
        .ok_or("pipeline must be an array")?;

    let mut results: Value = serde_json::json!({});
    let mut context: std::collections::HashMap<String, Value> = std::collections::HashMap::new();

    for (idx, step) in pipeline.iter().enumerate() {
        let step_name = step.get("step")
            .and_then(|s| s.as_str())
            .map(String::from)
            .unwrap_or_else(|| format!("step{}", idx));

        // Substitute $step.field references in arguments
        let substituted_args = substitute_pipeline_vars(step, &context)?;

        let action = substituted_args.get("action")
            .and_then(|a| a.as_str())
            .ok_or(format!("step '{}': missing action", step_name))?;

        let step_result = dispatch_plico_action(action, &substituted_args, kernel)?;

        // Parse and store result
        let result_json: Value = serde_json::from_str(&step_result)
            .unwrap_or_else(|_| serde_json::json!(step_result));
        context.insert(step_name.clone(), result_json.clone());

        // Add to results object
        results[step_name] = result_json;
    }

    Ok(serde_json::to_string_pretty(&results).unwrap_or_default())
}

/// Substitute $step.field references in step arguments
fn substitute_pipeline_vars(step: &Value, context: &std::collections::HashMap<String, Value>) -> Result<Value, String> {
    let step_json = serde_json::to_string(step).map_err(|e| e.to_string())?;
    let mut result = step_json.clone();

    for (key, value) in context.iter() {
        let value_str = serde_json::to_string(value).unwrap_or_default();
        // Replace $key with the actual value string
        result = result.replace(&format!("${}", key), &value_str);
    }

    serde_json::from_str(&result).map_err(|e| e.to_string())
}

/// Dispatch individual plico actions to kernel API requests
fn dispatch_plico_action(action: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    match action {
        "plico" => {
            // Cold-layer params routing with teaching errors
            return dispatch_cold_layer(args, kernel);
        }

        "session_start" => {
            let req = ApiRequest::StartSession {
                agent_id: agent.to_string(),
                agent_token: args.get("agent_token").and_then(|v| v.as_str()).map(String::from),
                intent_hint: args.get("intent_hint").and_then(|v| v.as_str()).map(String::from),
                load_tiers: vec![],
                last_seen_seq: args.get("last_seen_seq").and_then(|v| v.as_u64()),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "session_end" => {
            let session_id = args.get("session_id")
                .and_then(|s| s.as_str())
                .ok_or("session_end requires session_id")?;
            let req = ApiRequest::EndSession {
                agent_id: agent.to_string(),
                session_id: session_id.to_string(),
                auto_checkpoint: true,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "remember" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("remember requires content")?;

            let req = ApiRequest::Remember {
                agent_id: agent.to_string(),
                content: content.to_string(),
                tenant_id: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "recall" => {
            let req = ApiRequest::Recall {
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "recall_semantic" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("recall_semantic requires query")?;
            let k = args.get("k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

            let req = ApiRequest::RecallSemantic {
                agent_id: agent.to_string(),
                query: query.to_string(),
                k,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "search" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("search requires query")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let require_tags: Vec<String> = args.get("require_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let exclude_tags: Vec<String> = args.get("exclude_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::Search {
                query: query.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
                limit,
                offset: None,
                require_tags,
                exclude_tags,
                since: None,
                until: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "hybrid" => {
            let query = args.get("query")
                .and_then(|q| q.as_str())
                .ok_or("hybrid requires query")?;
            let max_results = args.get("limit").and_then(|l| l.as_u64()).unwrap_or(20) as usize;
            let token_budget = args.get("token_budget").and_then(|t| t.as_u64()).map(|t| t as usize);
            let seed_tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::HybridRetrieve {
                query_text: query.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                seed_tags,
                graph_depth: 2,
                edge_types: vec![],
                max_results,
                token_budget,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "intent_declare" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("intent_declare requires content")?;
            let priority = args.get("priority").and_then(|p| p.as_str()).unwrap_or("normal");

            let req = ApiRequest::SubmitIntent {
                description: content.to_string(),
                priority: priority.to_string(),
                action: None,
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "intent_fetch" => {
            let cids_json = args.get("cids");
            let cids: Vec<plico::api::semantic::ContextAssembleCandidate> = cids_json
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|v| {
                        Some(plico::api::semantic::ContextAssembleCandidate {
                            cid: v.get("cid")?.as_str()?.to_string(),
                            relevance: v.get("relevance").and_then(|r| r.as_f64()).unwrap_or(1.0) as f32,
                        })
                    }).collect()
                })
                .unwrap_or_default();
            let budget_tokens = args.get("token_budget").and_then(|t| t.as_u64()).unwrap_or(4096) as usize;

            let req = ApiRequest::ContextAssemble {
                agent_id: agent.to_string(),
                cids,
                budget_tokens,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "delta" => {
            let since_seq = args.get("since_seq")
                .and_then(|s| s.as_u64())
                .ok_or("delta requires since_seq")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let watch_cids: Vec<String> = args.get("watch_cids")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let watch_tags: Vec<String> = args.get("watch_tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::DeltaSince {
                agent_id: agent.to_string(),
                since_seq,
                watch_cids,
                watch_tags,
                limit,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "growth" => {
            let req = ApiRequest::AgentUsage {
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "status" => {
            let req = ApiRequest::SystemStatus;
            format_plico_response(kernel.handle_api_request(req), args)
        }

        _ => Err(format!("unknown action: {action}")),
    }
}

/// Cold-layer params routing with teaching error messages.
/// When AI sends invalid params for kg/task/batch operations, we return
/// teaching-style errors with examples of the correct format.
fn dispatch_cold_layer(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    let params = args.get("params")
        .and_then(|p| p.as_object())
        .ok_or("Cold-layer operation requires 'params' object. Example: {action:'plico', params:{method:'add_node', label:'MyNode', node_type:'entity'}, agent_id:'my-agent'}")?;

    let method = params.get("method")
        .and_then(|m| m.as_str())
        .ok_or("Missing 'method' in params. Valid methods: add_node, add_edge, causal_path, impact, delegate, complete, batch_create, batch_read, register, checkpoint, restore, subscribe, poll, unsubscribe, storage_stats, object_usage, evict_expired")?;

    // Pre-validate required params for known methods before calling kernel
    if let Some(err) = validate_cold_params(method, params) {
        return Err(err);
    }

    // Route to appropriate kernel API based on method
    let req = build_cold_request(method, params, agent)?;
    let resp = kernel.handle_api_request(req);

    // If kernel returned an error, try to enhance it with teaching example
    if !resp.ok {
        let err = resp.error.unwrap_or_else(|| "unknown error".to_string());
        return Err(enhance_cold_error(method, &err));
    }

    format_plico_response(resp, args)
}

/// Validate required params for cold-layer methods. Returns Some(error) if validation fails.
fn validate_cold_params(method: &str, params: &serde_json::Map<String, Value>) -> Option<String> {
    match method {
        "add_node" => {
            if !params.contains_key("label") {
                return Some("Missing 'label'. Example: {method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("node_type") {
                return Some("Missing 'node_type'. Example: {method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}".to_string());
            }
        }
        "add_edge" => {
            if !params.contains_key("src_id") {
                return Some("Missing 'src_id'. Example: {method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("dst_id") {
                return Some("Missing 'dst_id'. Example: {method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}".to_string());
            }
        }
        "causal_path" => {
            if !params.contains_key("from_id") {
                return Some("Missing 'from_id'. Example: {method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}".to_string());
            }
            if !params.contains_key("to_id") {
                return Some("Missing 'to_id'. Example: {method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}".to_string());
            }
        }
        "impact" => {
            if !params.contains_key("node_id") {
                return Some("Missing 'node_id'. Example: {method:'impact', node_id:'<node_id>', depth:3, agent_id:'your-agent'}".to_string());
            }
        }
        "delegate" => {
            if !params.contains_key("task_description") {
                return Some("Missing 'task_description'. Example: {method:'delegate', task_description:'analyze logs', to_agent:'<agent>', agent_id:'your-agent'}".to_string());
            }
        }
        "complete" => {
            if !params.contains_key("task_id") {
                return Some("Missing 'task_id'. Example: {method:'complete', task_id:'<task_id>', agent_id:'your-agent'}".to_string());
            }
        }
        "batch_create" => {
            if !params.contains_key("items") {
                return Some("Missing 'items' array. Example: {method:'batch_create', items:[{content:'text', tags:['tag']}], agent_id:'your-agent'}".to_string());
            }
        }
        _ => {}
    }
    None
}

/// Build kernel ApiRequest from cold-layer params
fn build_cold_request(method: &str, params: &serde_json::Map<String, Value>, agent: &str) -> Result<ApiRequest, String> {
    use plico::fs::KGNodeType;
    use plico::fs::KGEdgeType;

    match method {
        "add_node" => {
            let label = params.get("label").and_then(|v| v.as_str()).unwrap();
            let node_type_str = params.get("node_type").and_then(|v| v.as_str()).unwrap_or("entity");
            let node_type = match node_type_str {
                "entity" => KGNodeType::Entity,
                "fact" => KGNodeType::Fact,
                "document" => KGNodeType::Document,
                "agent" => KGNodeType::Agent,
                "memory" => KGNodeType::Memory,
                _ => KGNodeType::Entity,
            };
            Ok(ApiRequest::AddNode {
                label: label.to_string(),
                node_type,
                properties: params.get("properties").cloned().unwrap_or(Value::Null),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "add_edge" => {
            let src_id = params.get("src_id").and_then(|v| v.as_str()).unwrap();
            let dst_id = params.get("dst_id").and_then(|v| v.as_str()).unwrap();
            let edge_type_str = params.get("edge_type").and_then(|v| v.as_str()).unwrap_or("causes");
            let edge_type = match edge_type_str {
                "causes" => KGEdgeType::Causes,
                "reminds" => KGEdgeType::Reminds,
                "part_of" => KGEdgeType::PartOf,
                "similar_to" => KGEdgeType::SimilarTo,
                "related_to" => KGEdgeType::RelatedTo,
                _ => KGEdgeType::Causes,
            };
            Ok(ApiRequest::AddEdge {
                src_id: src_id.to_string(),
                dst_id: dst_id.to_string(),
                edge_type,
                weight: params.get("weight").and_then(|v| v.as_f64()).map(|w| w as f32),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "causal_path" => {
            let from_id = params.get("from_id").and_then(|v| v.as_str()).unwrap();
            let to_id = params.get("to_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::FindPaths {
                src_id: from_id.to_string(),
                dst_id: to_id.to_string(),
                max_depth: params.get("depth").and_then(|v| v.as_u64()).map(|d| d as u8),
                weighted: params.get("weighted").and_then(|v| v.as_bool()).unwrap_or(false),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "impact" => {
            let node_id = params.get("node_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::FindPaths {
                src_id: node_id.to_string(),
                dst_id: "*".to_string(),
                max_depth: params.get("depth").and_then(|v| v.as_u64()).map(|d| d as u8),
                weighted: params.get("weighted").and_then(|v| v.as_bool()).unwrap_or(false),
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "delegate" => {
            let task_description = params.get("task_description").and_then(|v| v.as_str()).unwrap();
            let task_id = params.get("task_id").and_then(|v| v.as_str()).map(String::from).unwrap_or_else(|| {
                use std::time::{SystemTime, UNIX_EPOCH};
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string()
            });
            Ok(ApiRequest::DelegateTask {
                task_id: task_id.to_string(),
                from_agent: agent.to_string(),
                to_agent: params.get("to_agent").and_then(|v| v.as_str()).unwrap_or("default").to_string(),
                intent: task_description.to_string(),
                context_cids: vec![],
                deadline_ms: None,
            })
        }
        "complete" => {
            let task_id = params.get("task_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::TaskComplete {
                task_id: task_id.to_string(),
                agent_id: agent.to_string(),
                result_cids: vec![],
            })
        }
        "query_task" => {
            let task_id = params.get("task_id").and_then(|v| v.as_str()).unwrap();
            Ok(ApiRequest::QueryTaskStatus {
                task_id: task_id.to_string(),
            })
        }
        "batch_create" => {
            let items = params.get("items")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter().filter_map(|item| {
                        Some(plico::api::semantic::BatchCreateItem {
                            content: item.get("content")?.as_str()?.to_string(),
                            tags: item.get("tags").and_then(|t| t.as_array())
                                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                .unwrap_or_default(),
                            content_encoding: Default::default(),
                            intent: item.get("intent").and_then(|v| v.as_str()).map(String::from),
                        })
                    }).collect()
                })
                .unwrap_or_default();
            Ok(ApiRequest::BatchCreate {
                items,
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        "register" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
            Ok(ApiRequest::RegisterAgent {
                name: name.to_string(),
            })
        }
        "checkpoint" => {
            Ok(ApiRequest::AgentCheckpoint {
                agent_id: agent.to_string(),
            })
        }
        "restore" => {
            let checkpoint_cid = params.get("checkpoint_cid").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::AgentRestore {
                agent_id: agent.to_string(),
                checkpoint_cid: checkpoint_cid.to_string(),
            })
        }
        "subscribe" => {
            let event_types = params.get("event_types").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            Ok(ApiRequest::EventSubscribe {
                agent_id: agent.to_string(),
                event_types,
                agent_ids: None,
            })
        }
        "poll" => {
            let subscription_id = params.get("subscription_id").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::EventPoll {
                subscription_id: subscription_id.to_string(),
            })
        }
        "unsubscribe" => {
            let subscription_id = params.get("subscription_id").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::EventUnsubscribe {
                subscription_id: subscription_id.to_string(),
            })
        }
        "storage_stats" => {
            Ok(ApiRequest::CacheStats)
        }
        "object_usage" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ApiRequest::Explore {
                cid: cid.to_string(),
                edge_type: None,
                depth: None,
                agent_id: agent.to_string(),
            })
        }
        "evict_expired" => {
            Ok(ApiRequest::EvictExpired {
                agent_id: agent.to_string(),
                tenant_id: None,
            })
        }
        _ => Err(format!("unknown cold method: {method}")),
    }
}

/// Enhance cold-layer error messages with teaching examples
fn enhance_cold_error(method: &str, error: &str) -> String {
    // Only enhance errors that are about missing/required fields
    if !error.contains("missing") && !error.contains("required") && !error.contains("not found") {
        return error.to_string();
    }

    let example = match method {
        "add_node" => "{method:'add_node', label:'MyEntity', node_type:'entity', agent_id:'your-agent'}",
        "add_edge" => "{method:'add_edge', src_id:'<node_a>', dst_id:'<node_b>', edge_type:'causes', agent_id:'your-agent'}",
        "causal_path" => "{method:'causal_path', from_id:'<node_a>', to_id:'<node_b>', agent_id:'your-agent'}",
        "impact" => "{method:'impact', node_id:'<node_id>', depth:3, agent_id:'your-agent'}",
        "delegate" => "{method:'delegate', task_description:'analyze logs', to_agent:'<agent>', agent_id:'your-agent'}",
        "complete" => "{method:'complete', task_id:'<task_id>', agent_id:'your-agent'}",
        "query_task" => "{method:'query_task', task_id:'<task_id>'}",
        "batch_create" => "{method:'batch_create', items:[{content:'text', tags:['tag']}], agent_id:'your-agent'}",
        "register" => "{method:'register', name:'my-agent'}",
        "checkpoint" => "{method:'checkpoint', agent_id:'your-agent'}",
        "restore" => "{method:'restore', agent_id:'your-agent', checkpoint_cid:'<cid>'}",
        "subscribe" => "{method:'subscribe', event_types:['memory_stored','agent_registered'], agent_id:'your-agent'}",
        "poll" => "{method:'poll', subscription_id:'<id>', agent_id:'your-agent'}",
        "unsubscribe" => "{method:'unsubscribe', subscription_id:'<id>'}",
        "storage_stats" => "{method:'storage_stats', agent_id:'your-agent'}",
        "object_usage" => "{method:'object_usage', cid:'<cid>', agent_id:'your-agent'}",
        "evict_expired" => "{method:'evict_expired', agent_id:'your-agent'}",
        _ => return error.to_string(),
    };

    format!("{}. Example: {}", error, example)
}

/// Apply select (field projection) and preview (truncation) to search/hybrid results
fn apply_response_shaping(response_json: &mut Value, args: &Value) {
    // Apply preview for truncation first (applies to entire response)
    if let Some(preview) = args.get("preview").and_then(|p| p.as_u64()) {
        if preview > 0 {
            truncate_by_preview(response_json, preview as usize);
        }
    }

    // Apply select for field projection - only to results array items
    if let Some(select) = args.get("select").and_then(|s| s.as_array()) {
        let fields: Vec<&str> = select.iter().filter_map(|v| v.as_str()).collect();
        if !fields.is_empty() {
            // Only apply select to the results array, not the top-level response
            if let Some(results) = response_json.get_mut("results").and_then(|r| r.as_array_mut()) {
                for item in results.iter_mut() {
                    if let Some(obj) = item.as_object_mut() {
                        obj.retain(|key, _| fields.iter().any(|f| *f == key));
                    }
                }
            }
        }
    }
}

/// Recursively truncate string fields to preview chars per result
fn truncate_by_preview(value: &mut Value, preview: usize) {
    match value {
        Value::String(s) if s.len() > preview => {
            *s = format!("{}...", &s[..preview]);
        }
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                truncate_by_preview(item, preview);
            }
        }
        Value::Object(obj) => {
            for (_, v) in obj.iter_mut() {
                truncate_by_preview(v, preview);
            }
        }
        _ => {}
    }
}

/// Format API response with optional response shaping
fn format_plico_response(resp: ApiResponse, args: &Value) -> Result<String, String> {
    if !resp.ok {
        return Err(resp.error.unwrap_or_else(|| "unknown error".to_string()));
    }

    let mut response_json: Value = serde_json::to_value(&resp).map_err(|e| e.to_string())?;

    // Apply response shaping for search/hybrid actions
    let action = args.get("action").and_then(|a| a.as_str()).unwrap_or("");
    if action == "search" || action == "hybrid" || action == "recall" || action == "recall_semantic" {
        apply_response_shaping(&mut response_json, args);
    }

    Ok(serde_json::to_string_pretty(&response_json).unwrap_or_default())
}

/// Dispatch plico_store tool (simplified CAS operations)
fn dispatch_plico_store(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let store_action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("plico_store requires action")?;

    match store_action {
        "put" => {
            let content = args.get("content")
                .and_then(|c| c.as_str())
                .ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let req = ApiRequest::Create {
                api_version: None,
                content: content.to_string(),
                content_encoding: Default::default(),
                tags,
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
                intent: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        "read" => {
            let cid = args.get("cid")
                .and_then(|c| c.as_str())
                .ok_or("read requires cid")?;
            let req = ApiRequest::Read {
                cid: cid.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        _ => Err(format!("unknown store action: {}", store_action)),
    }
}

/// Ensure 6 pre-installed skills exist in procedural memory.
/// Called on first plico_skills list/run to seed builtins if empty.
fn ensure_builtin_skills(kernel: &AIKernel) {
    // Check if any builtin skills already exist
    let existing = kernel.recall_shared_procedural(None);
    let has_builtins = existing.iter().any(|e| {
        e.tags.contains(&"plico:builtin".to_string())
    });
    if has_builtins {
        return; // Already seeded
    }

    let builtins = [
        (
            "knowledge-graph",
            "Build and query causal knowledge graphs in Plico",
            vec![
                ("Create an entity node", r#"plico(action="kg", params={"method": "add_node", "label": "<label>", "node_type": "entity"})"#, "returns node_id"),
                ("Create a causal edge between nodes", r#"plico(action="kg", params={"method": "add_edge", "src_id": "<node_a>", "dst_id": "<node_b>", "edge_type": "causes"})"#, "returns edge confirmation"),
                ("Query causal path between nodes", r#"plico(action="kg", params={"method": "causal_path", "from_id": "<node_a>", "to_id": "<node_b>"})"#, "returns path with intermediate nodes"),
                ("Analyze impact of a node", r#"plico(action="kg", params={"method": "impact", "node_id": "<node>", "depth": 3})"#, "returns all affected nodes within 3 hops"),
            ],
        ),
        (
            "task-delegation",
            "Delegate tasks to other agents and track completion",
            vec![
                ("Discover available agents", "plico(action=\"status\")", "returns list of active agents"),
                ("Delegate a task", r#"plico(action="params", params={"method": "delegate", "task_description": "<description>", "to_agent": "<agent-name>"})"#, "returns task_id"),
                ("Check task status", r#"plico(action="params", params={"method": "query_task", "task_id": "<task_id>"})"#, "returns status (pending/in_progress/completed/failed)"),
                ("Mark task complete", r#"plico(action="params", params={"method": "complete", "task_id": "<task_id>"})"#, "returns confirmation"),
            ],
        ),
        (
            "batch-operations",
            "Efficiently store and retrieve multiple items",
            vec![
                ("Store multiple content items at once", r#"plico(action="params", params={"method": "batch_create", "items": [{"content": "item1", "tags": ["tag1"]}, {"content": "item2", "tags": ["tag2"]}]})"#, "returns array of CIDs"),
                ("Store multiple memories at once", r#"plico(action="params", params={"method": "batch_memory", "entries": [{"content": "remember X", "tier": "working"}, {"content": "remember Y", "tier": "long_term"}]})"#, "returns confirmation"),
                ("Retrieve multiple items by CID", r#"plico(action="params", params={"method": "batch_read", "cids": ["<cid1>", "<cid2>"]})"#, "returns array of content items"),
            ],
        ),
        (
            "agent-lifecycle",
            "Register agents, checkpoint, and restore state",
            vec![
                ("Register a new agent", r#"plico(action="params", params={"method": "register", "name": "<agent-name>"})"#, "returns agent_id"),
                ("Create a checkpoint (save state)", r#"plico(action="params", params={"method": "checkpoint", "agent_id": "<agent-id>"})"#, "returns checkpoint_cid"),
                ("Restore from checkpoint", r#"plico(action="params", params={"method": "restore", "agent_id": "<agent-id>", "checkpoint_cid": "<cid>"})"#, "returns restoration confirmation"),
                ("Suspend agent (pause)", r#"plico(action="params", params={"method": "suspend", "agent_id": "<agent-id>"})"#, "returns confirmation"),
                ("Resume agent", r#"plico(action="params", params={"method": "resume", "agent_id": "<agent-id>"})"#, "returns confirmation"),
            ],
        ),
        (
            "event-system",
            "Subscribe to and poll for system events",
            vec![
                ("Subscribe to events", r#"plico(action="params", params={"method": "subscribe", "event_types": ["memory_stored", "agent_registered"]})"#, "returns subscription_id"),
                ("Poll for new events", r#"plico(action="params", params={"method": "poll", "subscription_id": "<id>"})"#, "returns array of events since last poll"),
                ("Unsubscribe when done", r#"plico(action="params", params={"method": "unsubscribe", "subscription_id": "<id>"})"#, "returns confirmation"),
            ],
        ),
        (
            "storage-governance",
            "Monitor storage usage and evict cold data",
            vec![
                ("Get storage statistics", r#"plico(action="params", params={"method": "storage_stats"})"#, "returns total_size, object_count, tier_breakdown"),
                ("Get per-object usage stats", r#"plico(action="params", params={"method": "object_usage", "cid": "<cid>"})"#, "returns access_count, last_access, tier"),
                ("Evict expired/cold entries", r#"plico(action="params", params={"method": "evict_expired"})"#, "returns number of entries evicted"),
                ("Move memory between tiers", r#"plico(action="params", params={"method": "memory_move", "entry_id": "<id>", "target_tier": "long_term"})"#, "returns confirmation"),
            ],
        ),
    ];

    for (name, description, steps) in builtins {
        let proc_steps: Vec<ProcedureStepDto> = steps
            .iter()
            .map(|(desc, action, expected)| ProcedureStepDto {
                description: desc.to_string(),
                action: action.to_string(),
                expected_outcome: Some(expected.to_string()),
            })
            .collect();

        let req = ApiRequest::RememberProcedural {
            agent_id: "system".to_string(),
            name: name.to_string(),
            description: description.to_string(),
            steps: proc_steps,
            learned_from: Some("Plico OS v5.0 builtin".to_string()),
            tags: vec!["plico:skill".to_string(), "plico:builtin".to_string()],
            scope: Some("shared".to_string()),
        };
        let _ = kernel.handle_api_request(req);
    }
}

/// Dispatch plico_skills tool (list/run/create)
fn dispatch_plico_skills(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let skill_action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("plico_skills requires action")?;

    match skill_action {
        "list" => {
            let private_entries = kernel.recall_procedural(agent, "default", None);
            let shared_entries = kernel.recall_shared_procedural(None);

            // Combine and deduplicate by name (shared takes precedence for same name)
            let mut skills_map: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
            for e in private_entries.iter().chain(shared_entries.iter()) {
                if let plico::memory::MemoryContent::Procedure(p) = &e.content {
                    skills_map.entry(p.name.clone()).or_insert_with(|| {
                        serde_json::json!({
                            "name": p.name,
                            "description": p.description,
                            "steps_count": p.steps.len(),
                            "learned_from": p.learned_from,
                            "tags": e.tags,
                        })
                    });
                }
            }
            let skills: Vec<Value> = skills_map.into_values().collect();
            Ok(serde_json::to_string_pretty(&serde_json::json!({
                "skills": skills,
                "count": skills.len(),
            })).unwrap_or_default())
        }

        "run" => {
            let name = args.get("name")
                .and_then(|n| n.as_str())
                .ok_or("run requires name")?;

            // Check private first, then shared
            let mut entries = kernel.recall_procedural(agent, "default", Some(name));
            if entries.is_empty() {
                entries = kernel.recall_shared_procedural(Some(name));
            }
            if entries.is_empty() {
                return Err(format!("no skill named '{}' found", name));
            }
            let entry = &entries[0];
            if let plico::memory::MemoryContent::Procedure(p) = &entry.content {
                Ok(serde_json::to_string_pretty(&serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "steps": p.steps.iter().map(|s| serde_json::json!({
                        "step_number": s.step_number,
                        "description": s.description,
                        "action": s.action,
                        "expected_outcome": s.expected_outcome,
                    })).collect::<Vec<Value>>(),
                    "learned_from": p.learned_from,
                })).unwrap_or_default())
            } else {
                Err("entry is not a procedure".to_string())
            }
        }

        "create" => {
            let name = args.get("name")
                .and_then(|n| n.as_str())
                .ok_or("create requires name")?;
            let description = args.get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let steps_json = args.get("steps").and_then(|s| s.as_array());
            let learned_from = args.get("learned_from").and_then(|l| l.as_str()).map(String::from);
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();

            let steps: Vec<plico::api::semantic::ProcedureStepDto> = steps_json
                .map(|arr| {
                    arr.iter().map(|s| {
                        plico::api::semantic::ProcedureStepDto {
                            description: s.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                            action: s.get("action").and_then(|a| a.as_str()).unwrap_or("").to_string(),
                            expected_outcome: s.get("expected_outcome").and_then(|e| e.as_str()).map(String::from),
                        }
                    }).collect()
                })
                .unwrap_or_default();

            let req = ApiRequest::RememberProcedural {
                agent_id: agent.to_string(),
                name: name.to_string(),
                description: description.to_string(),
                steps,
                learned_from,
                tags,
                scope: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        _ => Err(format!("unknown skills action: {}", skill_action)),
    }
}

fn format_response(resp: ApiResponse) -> Result<String, String> {
    if resp.ok {
        Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
    } else {
        Err(resp.error.unwrap_or_else(|| "unknown error".to_string()))
    }
}

fn make_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn make_error_response(id: Value, code: i32, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

fn tool_definitions() -> Vec<Value> {
    vec![
        // Main gateway tool - plico
        serde_json::json!({
            "name": "plico",
            "description": "Plico AIOS kernel gateway. Single mode: session_start/end, remember/recall/recall_semantic, search/hybrid, intent_declare/intent_fetch, delta/growth/status. Batch mode: pipeline=[...] for sequential multi-step execution. Use select:[\"field\"] for field projection on search/hybrid. Use preview:N for result previews. For advanced ops (KG, tasks, batch), use plico_skills.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["session_start","session_end","remember","recall","recall_semantic",
                                 "search","hybrid","intent_declare","intent_fetch","delta","growth","status"],
                        "description": "Single operation mode"
                    },
                    "pipeline": {
                        "type": "array",
                        "description": "Batch mode: [{step,action,...}]. Steps run sequentially. Use $step.field for references.",
                        "items": { "type": "object" }
                    },
                    "agent_id": { "type": "string" },
                    "content": { "type": "string", "description": "For remember, intent_declare" },
                    "query": { "type": "string", "description": "For recall/search/hybrid" },
                    "tier": { "type": "string", "enum": ["working","long_term"], "description": "For recall" },
                    "scope": { "type": "string", "enum": ["private","shared"], "description": "For remember" },
                    "token_budget": { "type": "number", "description": "Max tokens for context (intent_fetch)" },
                    "intent_hint": { "type": "string", "description": "For session_start: triggers delta+prefetch" },
                    "session_id": { "type": "string", "description": "For session_end" },
                    "intent_id": { "type": "string", "description": "For intent_fetch" },
                    "since_seq": { "type": "number", "description": "For delta" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "select": { "type": "array", "items": { "type": "string" }, "description": "Field projection for search/hybrid" },
                    "preview": { "type": "number", "description": "Preview chars per result (0=full)" },
                    "params": { "type": "object", "description": "Additional/cold-path parameters" },
                    "limit": { "type": "number", "description": "Max results for search/hybrid" },
                    "require_tags": { "type": "array", "items": { "type": "string" }, "description": "For search" },
                    "exclude_tags": { "type": "array", "items": { "type": "string" }, "description": "For search" }
                },
                "oneOf": [
                    { "required": ["action", "agent_id"] },
                    { "required": ["pipeline"] }
                ]
            }
        }),
        // CAS store tool - plico_store
        serde_json::json!({
            "name": "plico_store",
            "description": "CAS read/write. action:\"put\" stores content, returns CID. action:\"read\" retrieves by CID.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["put", "read"] },
                    "content": { "type": "string", "description": "Content to store (for put)" },
                    "cid": { "type": "string", "description": "CID to read (for read)" },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "agent_id": { "type": "string" }
                },
                "required": ["action", "agent_id"]
            }
        }),
        // Skills tool - plico_skills
        serde_json::json!({
            "name": "plico_skills",
            "description": "Discover, run, and create reusable workflows. Skills teach you how to use advanced Plico features (knowledge graph, task delegation, batch operations). Skills are procedural memories — once learned, available across all sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["list", "run", "create"] },
                    "name": { "type": "string", "description": "Skill name (for run/create)" },
                    "agent_id": { "type": "string" },
                    "description": { "type": "string", "description": "Skill description (for create)" },
                    "steps": { "type": "array", "description": "Workflow steps (for create)" },
                    "learned_from": { "type": "string", "description": "Provenance (for create)" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "For create" }
                },
                "required": ["action", "agent_id"]
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_returns_protocol_version() {
        let resp = handle_initialize(serde_json::json!(1));
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert!(result["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_all_tools() {
        let resp = handle_tools_list(serde_json::json!(2));
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3, "should have 3 tools: plico, plico_store, plico_skills");
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"plico"));
        assert!(names.contains(&"plico_store"));
        assert!(names.contains(&"plico_skills"));
    }

    #[test]
    fn tools_have_input_schema() {
        let tools = tool_definitions();
        for tool in &tools {
            assert!(tool["inputSchema"]["type"].as_str() == Some("object"),
                "tool {} must have object inputSchema", tool["name"]);
        }
    }

    #[test]
    fn make_error_response_format() {
        let resp = make_error_response(serde_json::json!(99), -32601, "not found");
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 99);
        assert_eq!(resp["error"]["code"], -32601);
        assert_eq!(resp["error"]["message"], "not found");
    }

    #[test]
    fn dispatch_unknown_tool_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("nonexistent", &serde_json::json!({}), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn dispatch_plico_search_missing_query_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "search",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("query"));
    }

    #[test]
    fn dispatch_plico_put_and_store_read_roundtrip() {
        let kernel = make_test_kernel();

        // Put via plico_store (uses Create API which supports tags)
        let put_result = dispatch_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "MCP test content",
            "tags": ["mcp-test"],
            "agent_id": "test"
        }), &kernel);
        assert!(put_result.is_ok(), "put failed: {:?}", put_result);
        let put_json: Value = serde_json::from_str(&put_result.unwrap()).unwrap();
        let cid = put_json["cid"].as_str().unwrap();

        // Read via plico_store
        let read_result = dispatch_tool("plico_store", &serde_json::json!({
            "action": "read",
            "agent_id": "test",
            "cid": cid
        }), &kernel);
        assert!(read_result.is_ok(), "read failed: {:?}", read_result);
        let read_json: Value = serde_json::from_str(&read_result.unwrap()).unwrap();
        assert_eq!(read_json["data"].as_str().unwrap(), "MCP test content");
    }

    #[test]
    fn dispatch_plico_search_finds_stored_content() {
        let kernel = make_test_kernel();

        // Store via plico_store (uses Create API which indexes for search)
        dispatch_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "Dijkstra weighted path algorithm",
            "tags": ["plico:type:experience", "plico:module:graph"],
            "agent_id": "test"
        }), &kernel).unwrap();

        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "search",
            "agent_id": "test",
            "query": "Dijkstra weighted path"
        }), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        let results = json["results"].as_array().unwrap();
        assert!(!results.is_empty(), "search should find stored content");
    }

    #[test]
    fn dispatch_plico_store_put_and_read_roundtrip() {
        let kernel = make_test_kernel();

        let put_result = dispatch_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "Store test content",
            "tags": ["store-test"],
            "agent_id": "test"
        }), &kernel);
        assert!(put_result.is_ok(), "put failed: {:?}", put_result);
        let put_json: Value = serde_json::from_str(&put_result.unwrap()).unwrap();
        let cid = put_json["cid"].as_str().unwrap();

        let read_result = dispatch_tool("plico_store", &serde_json::json!({
            "action": "read",
            "cid": cid,
            "agent_id": "test"
        }), &kernel);
        assert!(read_result.is_ok(), "read failed: {:?}", read_result);
        let read_json: Value = serde_json::from_str(&read_result.unwrap()).unwrap();
        assert_eq!(read_json["data"].as_str().unwrap(), "Store test content");
    }

    #[test]
    fn tools_call_response_has_content_array() {
        let kernel = make_test_kernel();
        let resp = handle_tools_call(
            serde_json::json!(5),
            &serde_json::json!({"name": "plico", "arguments": {"action": "status", "agent_id": "test"}}),
            &kernel,
        );
        let content = resp["result"]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
    }

    #[test]
    fn tools_call_error_has_is_error_flag() {
        let kernel = make_test_kernel();
        let resp = handle_tools_call(
            serde_json::json!(6),
            &serde_json::json!({"name": "plico", "arguments": {"action": "search", "agent_id": "test"}}),
            &kernel,
        );
        assert_eq!(resp["result"]["isError"], true);
    }

    #[test]
    fn dispatch_plico_recall_semantic_works() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "recall_semantic",
            "agent_id": "test",
            "query": "test query"
        }), &kernel);
        // Should not error, returns whatever kernel returns
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_status_works() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "status",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_growth_works() {
        let kernel = make_test_kernel();
        // growth (AgentUsage) requires a registered agent
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "growth",
            "agent_id": "test"
        }), &kernel);
        // Agent not found is expected since "test" isn't a registered agent
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("Agent not found") || err_msg.contains("not found"),
            "Expected 'Agent not found' error, got: {}", err_msg);
    }

    #[test]
    fn dispatch_plico_intent_declare_and_fetch() {
        let kernel = make_test_kernel();

        // Declare an intent
        let declare_result = dispatch_tool("plico", &serde_json::json!({
            "action": "intent_declare",
            "agent_id": "test",
            "content": "Test intent content"
        }), &kernel);
        assert!(declare_result.is_ok(), "intent_declare failed: {:?}", declare_result);
        let declare_json: Value = serde_json::from_str(&declare_result.unwrap()).unwrap();
        let intent_id = declare_json["intent_id"].as_str().unwrap();

        // Fetch the intent
        let fetch_result = dispatch_tool("plico", &serde_json::json!({
            "action": "intent_fetch",
            "agent_id": "test",
            "intent_id": intent_id
        }), &kernel);
        assert!(fetch_result.is_ok(), "intent_fetch failed: {:?}", fetch_result);
    }

    #[test]
    fn dispatch_plico_delta_requires_since_seq() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "delta",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("since_seq"));
    }

    fn make_test_kernel() -> AIKernel {
        let dir = tempfile::TempDir::new().unwrap();
        AIKernel::new(dir.path().to_path_buf()).unwrap()
    }

    fn store_test_skill(kernel: &AIKernel) {
        use plico::api::semantic::{ApiRequest, ProcedureStepDto};
        let req = ApiRequest::RememberProcedural {
            agent_id: DEFAULT_AGENT.to_string(),
            name: "bootstrap-module".to_string(),
            description: "Standard workflow to bootstrap a new Plico module".to_string(),
            steps: vec![
                ProcedureStepDto {
                    description: "Check existing modules".to_string(),
                    action: "nodes --type entity".to_string(),
                    expected_outcome: Some("List of current module entities".to_string()),
                },
                ProcedureStepDto {
                    description: "Create module entity node".to_string(),
                    action: "node --label <name> --type entity".to_string(),
                    expected_outcome: Some("New entity node ID".to_string()),
                },
                ProcedureStepDto {
                    description: "Store ADR for the module".to_string(),
                    action: "put --content <adr> --tags plico:type:adr".to_string(),
                    expected_outcome: Some("CID of stored ADR".to_string()),
                },
            ],
            learned_from: Some("v2.0 development experience".to_string()),
            tags: vec!["plico:type:skill".to_string()],
            scope: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.ok, "store_test_skill failed: {:?}", resp.error);
    }

    #[test]
    fn dispatch_skills_list_empty() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "list",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["count"], 0);
        assert_eq!(json["skills"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn dispatch_skills_list_finds_stored_skill() {
        let kernel = make_test_kernel();
        store_test_skill(&kernel);
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "list",
            "agent_id": DEFAULT_AGENT
        }), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["count"], 1);
        let skill = &json["skills"][0];
        assert_eq!(skill["name"], "bootstrap-module");
        assert_eq!(skill["steps_count"], 3);
    }

    #[test]
    fn dispatch_skills_run_returns_full_procedure() {
        let kernel = make_test_kernel();
        store_test_skill(&kernel);
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "run",
            "name": "bootstrap-module",
            "agent_id": DEFAULT_AGENT
        }), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["name"], "bootstrap-module");
        let steps = json["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0]["step_number"], 1);
        assert_eq!(steps[0]["action"], "nodes --type entity");
        assert_eq!(steps[2]["action"], "put --content <adr> --tags plico:type:adr");
    }

    #[test]
    fn dispatch_skills_run_missing_name_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "run",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn dispatch_skills_run_not_found_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "run",
            "name": "nonexistent-skill",
            "agent_id": "test"
        }), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonexistent-skill"));
    }

    #[test]
    fn dispatch_skills_create_works() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_skills", &serde_json::json!({
            "action": "create",
            "agent_id": "test",
            "name": "test-skill",
            "description": "A test skill",
            "steps": [
                {"description": "Step 1", "action": "echo hello"},
                {"description": "Step 2", "action": "echo world"}
            ],
            "learned_from": "test suite"
        }), &kernel);
        assert!(result.is_ok(), "create failed: {:?}", result);
    }

    #[test]
    fn pipeline_substitution_basic() {
        let kernel = make_test_kernel();

        // Store something
        dispatch_tool("plico", &serde_json::json!({
            "action": "remember",
            "agent_id": "test",
            "content": "Pipeline test content",
            "tags": ["pipeline-test"]
        }), &kernel).unwrap();

        // Search with pipeline
        let result = dispatch_tool("plico", &serde_json::json!({
            "pipeline": [
                {
                    "step": "search_step",
                    "action": "search",
                    "agent_id": "test",
                    "query": "Pipeline test content"
                }
            ]
        }), &kernel);

        assert!(result.is_ok(), "pipeline failed: {:?}", result);
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(json["search_step"].is_object());
    }

    #[test]
    fn select_field_projection() {
        let kernel = make_test_kernel();

        // Store content via plico_store
        dispatch_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "Select test content",
            "tags": ["select-test"],
            "agent_id": "test"
        }), &kernel).unwrap();

        // Search with select
        let result = dispatch_tool("plico", &serde_json::json!({
            "action": "search",
            "agent_id": "test",
            "query": "Select test",
            "select": ["cid", "score"]
        }), &kernel);

        assert!(result.is_ok(), "select failed: {:?}", result);
        let json_str = result.unwrap();
        let json: Value = serde_json::from_str(&json_str).unwrap();
        // Search results should be in the response
        let results = json["results"].as_array();
        assert!(results.is_some(), "search response should have results: {}", json_str);
        let results = results.unwrap();
        assert!(!results.is_empty(), "search should find stored content");
        let result_obj = &results[0];
        // Should only have cid and score fields
        let keys: Vec<String> = result_obj.as_object().unwrap().keys().cloned().collect();
        assert!(keys.iter().all(|k| k == "cid" || k == "score"),
            "Expected only cid and score, got: {:?}", keys);
    }
}