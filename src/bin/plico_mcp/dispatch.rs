//! Action dispatchers — routes MCP tool calls to Plico kernel API.

use plico::api::semantic::{ApiRequest, DiscoveryScope, KnowledgeType, ProcedureStepDto};
use plico::kernel::AIKernel;
use serde_json::Value;

use super::format::{format_response, format_plico_response};
use super::check_rate_limit;

pub const DEFAULT_AGENT: &str = "mcp-agent";

pub fn dispatch_tool(name: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    check_rate_limit()?;
    match name {
        "plico" => dispatch_plico(args, kernel),
        "plico_store" => dispatch_plico_store(args, kernel),
        "plico_skills" => dispatch_plico_skills(args, kernel),
        _ => Err(format!("unknown tool: {name}")),
    }
}

/// Remote-mode dispatch: routes MCP tool calls via KernelClient (no direct kernel access).
pub fn dispatch_tool_remote(name: &str, args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    check_rate_limit()?;
    match name {
        "plico" => dispatch_plico_remote(args, client),
        "plico_store" => dispatch_plico_store_remote(args, client),
        "plico_skills" => Err("plico_skills not available in daemon mode (requires direct kernel access)".to_string()),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn dispatch_plico_remote(args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    let action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing required parameter: action")?;
    dispatch_plico_action_remote(action, args, client)
}

fn dispatch_plico_action_remote(action: &str, args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    check_read_only(action, PLICO_ACTIONS)?;
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    let req = match action {
        "help" => return Ok(generate_help_response()),
        "put" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            ApiRequest::Create { api_version: None, content: content.to_string(), content_encoding: Default::default(), tags, agent_id: agent.to_string(), tenant_id: None, agent_token: None, intent: None }
        }
        "get" => {
            let cid = args.get("cid").and_then(|c| c.as_str()).ok_or("get requires cid")?;
            ApiRequest::Read { cid: cid.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None }
        }
        "search" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("search requires query")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            ApiRequest::Search { query: query.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None, limit, offset: None, require_tags: vec![], exclude_tags: vec![], since: None, until: None, intent_context: None }
        }
        "remember" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("remember requires content")?;
            ApiRequest::Remember { agent_id: agent.to_string(), content: content.to_string(), tenant_id: None }
        }
        "recall" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).map(String::from);
            let query = args.get("query").and_then(|q| q.as_str()).map(String::from);
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            ApiRequest::Recall { agent_id: agent.to_string(), scope, query, limit }
        }
        "status" => ApiRequest::SystemStatus,
        "session_start" => {
            ApiRequest::StartSession { agent_id: agent.to_string(), agent_token: None, intent_hint: args.get("intent_hint").and_then(|v| v.as_str()).map(String::from), load_tiers: vec![], last_seen_seq: args.get("last_seen_seq").and_then(|v| v.as_u64()) }
        }
        "session_end" => {
            let session_id = args.get("session_id").and_then(|s| s.as_str()).ok_or("session_end requires session_id")?;
            ApiRequest::EndSession { agent_id: agent.to_string(), session_id: session_id.to_string(), auto_checkpoint: true }
        }
        "delta" => {
            let since_seq = args.get("since_seq").and_then(|s| s.as_u64()).ok_or("delta requires since_seq")?;
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            ApiRequest::DeltaSince { agent_id: agent.to_string(), since_seq, watch_cids: vec![], watch_tags: vec![], limit }
        }
        "hybrid" => {
            let query = args.get("query").and_then(|q| q.as_str()).ok_or("hybrid requires query")?;
            ApiRequest::HybridRetrieve { query_text: query.to_string(), agent_id: agent.to_string(), tenant_id: None, seed_tags: vec![], graph_depth: 2, edge_types: vec![], max_results: 20, token_budget: None }
        }
        _ => return Err(format!("action '{}' not available in daemon mode", action)),
    };

    let resp = client.request(req);
    super::format::format_plico_response(resp, args)
}

fn dispatch_plico_store_remote(args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let store_action = args.get("action").and_then(|a| a.as_str()).ok_or("plico_store requires action")?;

    let req = match store_action {
        "put" => {
            let content = args.get("content").and_then(|c| c.as_str()).ok_or("put requires content")?;
            let tags: Vec<String> = args.get("tags")
                .and_then(|t| t.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            ApiRequest::Create { api_version: None, content: content.to_string(), content_encoding: Default::default(), tags, agent_id: agent.to_string(), tenant_id: None, agent_token: None, intent: None }
        }
        "read" => {
            let cid = args.get("cid").and_then(|c| c.as_str()).ok_or("read requires cid")?;
            ApiRequest::Read { cid: cid.to_string(), agent_id: agent.to_string(), tenant_id: None, agent_token: None }
        }
        _ => return Err(format!("unknown store action: {}", store_action)),
    };

    let resp = client.request(req);
    super::format::format_response(resp)
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

// ── F-31: ActionRegistry — data-driven action metadata ──

struct ActionMeta {
    name: &'static str,
    description: &'static str,
    params: &'static [(&'static str, &'static str, bool)], // (name, desc, required)
    is_write: bool,
}

const PLICO_ACTIONS: &[ActionMeta] = &[
    ActionMeta { name: "session_start", description: "Begin a work session with optional handover context", params: &[("agent_id", "Agent identifier", true), ("intent_hint", "What you plan to do", false), ("last_seen_seq", "Resume from event seq", false), ("handover_mode", "full|compact|none — context assembly for session recovery", false)], is_write: true },
    ActionMeta { name: "session_end", description: "End session, auto-checkpoint state", params: &[("agent_id", "Agent identifier", true), ("session_id", "Session to end", true)], is_write: true },
    ActionMeta { name: "put", description: "Store content to CAS, returns CID", params: &[("content", "Content to store", true), ("tags", "Classification tags", false), ("agent_id", "Agent identifier", true)], is_write: true },
    ActionMeta { name: "get", description: "Retrieve content by CID", params: &[("cid", "Content ID", true), ("agent_id", "Agent identifier", true)], is_write: false },
    ActionMeta { name: "search", description: "Semantic + keyword search", params: &[("query", "Search query", true), ("agent_id", "Agent identifier", true), ("tags", "Filter by tags", false), ("limit", "Max results", false), ("select", "Field projection", false), ("preview", "Preview char count", false)], is_write: false },
    ActionMeta { name: "hybrid", description: "Graph-RAG hybrid retrieval", params: &[("query", "Search query", true), ("agent_id", "Agent identifier", true), ("seed_tags", "Graph walk start tags", false), ("depth", "Graph walk depth", false)], is_write: false },
    ActionMeta { name: "remember", description: "Store to layered memory", params: &[("content", "Memory content", true), ("agent_id", "Agent identifier", true), ("scope", "private|shared", false)], is_write: true },
    ActionMeta { name: "recall", description: "Retrieve memories", params: &[("agent_id", "Agent identifier", true), ("tier", "working|long_term", false), ("scope", "private|shared — shared returns cross-agent memories", false), ("query", "Filter query for shared recall", false), ("limit", "Max results for shared recall", false)], is_write: false },
    ActionMeta { name: "recall_semantic", description: "Semantic memory search", params: &[("query", "Search query", true), ("agent_id", "Agent identifier", true), ("limit", "Max results", false)], is_write: false },
    ActionMeta { name: "pipeline", description: "Batch sequential operations", params: &[("pipeline", "Array of {step,action,...}", true)], is_write: true },
    ActionMeta { name: "delta", description: "Changes since event seq N", params: &[("since_seq", "Starting sequence number", false), ("agent_id", "Agent identifier", true)], is_write: false },
    ActionMeta { name: "growth", description: "Agent growth/activity report", params: &[("agent_id", "Agent identifier", true), ("period", "7d|30d|all", false)], is_write: false },
    ActionMeta { name: "status", description: "System health status", params: &[], is_write: false },
    ActionMeta { name: "discover", description: "Browse shared knowledge", params: &[("scope", "shared|private", false), ("knowledge_types", "Filter types", false)], is_write: false },
    ActionMeta { name: "memory_stats", description: "Memory tier statistics", params: &[("agent_id", "Agent identifier", true)], is_write: false },
    ActionMeta { name: "intent_declare", description: "Declare intent for context prefetch", params: &[("content", "Intent description", true), ("agent_id", "Agent identifier", true), ("token_budget", "Max tokens", false)], is_write: true },
    ActionMeta { name: "intent_fetch", description: "Fetch prefetched context", params: &[("intent_id", "Assembly ID from intent_declare", true), ("agent_id", "Agent identifier", true)], is_write: false },
    ActionMeta { name: "because", description: "Create a causal edge between two CIDs (one-step causality)", params: &[("cause_cid", "Source CID that caused the effect", true), ("effect_cid", "Target CID that was caused", true), ("reason", "Optional description of the causal relationship", false)], is_write: true },
    ActionMeta { name: "help", description: "List all available actions with parameters", params: &[], is_write: false },
];

const STORE_WRITE_ACTIONS: &[&str] = &["put"];

fn is_read_only_mode() -> bool {
    std::env::var("PLICO_READ_ONLY").map(|v| v == "true" || v == "1").unwrap_or(false)
}

fn check_read_only(action: &str, registry: &[ActionMeta]) -> Result<(), String> {
    if !is_read_only_mode() { return Ok(()); }
    if let Some(meta) = registry.iter().find(|m| m.name == action) {
        if meta.is_write {
            return Err(format!("read_only: action '{}' is a write operation. Set PLICO_READ_ONLY=false to allow writes.", action));
        }
    }
    Ok(())
}

pub fn generate_help_response() -> String {
    let actions: Vec<Value> = PLICO_ACTIONS.iter().map(|m| {
        let params: Vec<Value> = m.params.iter().map(|(name, desc, required)| {
            serde_json::json!({"name": name, "description": desc, "required": required})
        }).collect();
        serde_json::json!({
            "name": m.name,
            "description": m.description,
            "params": params,
            "is_write": m.is_write,
        })
    }).collect();
    serde_json::to_string_pretty(&serde_json::json!({"actions": actions})).unwrap_or_default()
}

// ── F-30: Smart Handover — assemble recovery context on session start ──

fn assemble_handover(kernel: &AIKernel, mode: &str) -> Value {
    let tags = kernel.list_tags();
    let status = kernel.system_status();

    let max_results = match mode { "compact" => 3, _ => 10 };
    let tag_limit = match mode { "compact" => 10, _ => 20 };

    let resp = kernel.handle_api_request(ApiRequest::Search {
        query: String::new(),
        agent_id: "system".to_string(),
        tenant_id: None,
        agent_token: None,
        limit: Some(max_results),
        offset: None,
        require_tags: vec![],
        exclude_tags: vec![],
        since: None,
        until: None,
        intent_context: None,
    });

    let recent: Vec<Value> = resp.results.unwrap_or_default().into_iter().map(|h| {
        serde_json::json!({ "cid": h.cid, "tags": h.tags, "relevance": h.relevance })
    }).collect();

    let active_tags: Vec<&String> = tags.iter().take(tag_limit).collect();

    // KG causal edges for handover context
    let kg_causal: Vec<Value> = if mode != "compact" {
        kernel.knowledge_graph()
            .and_then(|kg| kg.get_valid_edges_at(u64::MAX).ok())
            .map(|edges| {
                edges.into_iter()
                    .filter(|e| !matches!(e.edge_type, plico::fs::graph::types::KGEdgeType::AssociatesWith))
                    .take(5)
                    .map(|e| serde_json::json!({
                        "src": e.src, "dst": e.dst,
                        "type": format!("{:?}", e.edge_type),
                        "weight": e.weight,
                    }))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };

    serde_json::json!({
        "mode": mode,
        "recent_objects": recent,
        "active_tags": active_tags,
        "kg_causal_edges": kg_causal,
        "summary": {
            "total_objects": status.cas_object_count,
            "total_tags": tags.len(),
            "agents": status.agent_count,
        }
    })
}

/// Dispatch individual plico actions to kernel API requests
fn dispatch_plico_action(action: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    check_read_only(action, PLICO_ACTIONS)?;

    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    match action {
        "help" => Ok(generate_help_response()),

        "plico" => {
            return dispatch_cold_layer(args, kernel);
        }

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
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "get" => {
            let cid = args.get("cid")
                .and_then(|c| c.as_str())
                .ok_or("get requires cid")?;
            let req = ApiRequest::Read {
                cid: cid.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "session_start" => {
            let handover_mode = args.get("handover_mode").and_then(|v| v.as_str());

            let req = ApiRequest::StartSession {
                agent_id: agent.to_string(),
                agent_token: args.get("agent_token").and_then(|v| v.as_str()).map(String::from),
                intent_hint: args.get("intent_hint").and_then(|v| v.as_str()).map(String::from),
                load_tiers: vec![],
                last_seen_seq: args.get("last_seen_seq").and_then(|v| v.as_u64()),
            };
            let resp = kernel.handle_api_request(req);
            let mut json_str = format_plico_response(resp, args)?;

            if let Some(mode) = handover_mode {
                if mode != "none" {
                    let handover = assemble_handover(kernel, mode);
                    if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                        // F-34: Lost in the Middle — important fields first and last.
                        // LLMs attend most to start/end of context.
                        // Order: handover (most important) → session_started → health (least) → ok (anchor)
                        let mut ordered = serde_json::Map::new();
                        ordered.insert("handover".to_string(), handover);
                        if let Some(ss) = parsed.get("session_started") {
                            ordered.insert("session_started".to_string(), ss.clone());
                        }
                        // Middle: less important metadata
                        for (k, v) in parsed.as_object().into_iter().flatten() {
                            if k != "session_started" && k != "ok" {
                                ordered.entry(k.clone()).or_insert_with(|| v.clone());
                            }
                        }
                        ordered.insert("ok".to_string(), Value::Bool(true));
                        json_str = serde_json::to_string_pretty(&Value::Object(ordered))
                            .unwrap_or(json_str);
                    }
                }
            }

            Ok(json_str)
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

        "because" => {
            let cause_cid = args.get("cause_cid")
                .and_then(|c| c.as_str())
                .ok_or("because requires cause_cid")?;
            let effect_cid = args.get("effect_cid")
                .and_then(|c| c.as_str())
                .ok_or("because requires effect_cid")?;
            let reason = args.get("reason").and_then(|r| r.as_str()).unwrap_or("");

            let kg = kernel.knowledge_graph()
                .ok_or("knowledge graph not available")?;
            kg.upsert_document(cause_cid, &[], agent)
                .map_err(|e| format!("failed to upsert cause node: {e}"))?;
            kg.upsert_document(effect_cid, &[], agent)
                .map_err(|e| format!("failed to upsert effect node: {e}"))?;

            kernel.kg_add_edge(
                cause_cid, effect_cid,
                plico::fs::graph::types::KGEdgeType::Causes,
                Some(0.9),
                agent,
                "default",
            ).map_err(|e| format!("failed to add causal edge: {e}"))?;

            let mut resp = serde_json::json!({
                "ok": true,
                "edge": { "src": cause_cid, "dst": effect_cid, "type": "Causes" }
            });
            if !reason.is_empty() {
                resp["edge"]["reason"] = serde_json::Value::String(reason.to_string());
            }
            Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
        }

        "recall" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).map(String::from);
            let query = args.get("query").and_then(|q| q.as_str()).map(String::from);
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);
            let req = ApiRequest::Recall {
                agent_id: agent.to_string(),
                scope,
                query,
                limit,
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
                intent_context: None,
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

        "discover" => {
            let scope = args.get("scope").and_then(|s| s.as_str()).unwrap_or("shared");
            let knowledge_types = args.get("knowledge_types").and_then(|kt| kt.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default();
            let req = ApiRequest::DiscoverKnowledge {
                query: args.get("query").and_then(|q| q.as_str()).unwrap_or("").to_string(),
                scope: match scope {
                    "shared" => DiscoveryScope::Shared,
                    "all" => DiscoveryScope::AllAccessible,
                    _ => DiscoveryScope::Shared,
                },
                knowledge_types: knowledge_types.iter().map(|kt| match *kt {
                    "memory" => KnowledgeType::Memory,
                    "procedure" => KnowledgeType::Procedure,
                    "knowledge" => KnowledgeType::Knowledge,
                    _ => KnowledgeType::Memory,
                }).collect(),
                max_results: args.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
                token_budget: args.get("token_budget").and_then(|v| v.as_u64()).map(|t| t as usize),
                agent_id: agent.to_string(),
            };
            format_plico_response(kernel.handle_api_request(req), args)
        }

        "memory_stats" => {
            let tier = args.get("tier").and_then(|t| t.as_str()).map(String::from);
            let req = ApiRequest::MemoryStats {
                agent_id: agent.to_string(),
                tier,
                tenant_id: None,
            };
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
        "discover_knowledge" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let scope = params.get("scope").and_then(|v| v.as_str()).unwrap_or("shared");
            let scope_enum = match scope {
                "shared" => plico::api::semantic::DiscoveryScope::Shared,
                "all" => plico::api::semantic::DiscoveryScope::AllAccessible,
                _ => plico::api::semantic::DiscoveryScope::Shared,
            };
            let knowledge_types = params.get("knowledge_types").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| {
                    match t.as_str()? {
                        "memory" => Some(plico::api::semantic::KnowledgeType::Memory),
                        "procedure" => Some(plico::api::semantic::KnowledgeType::Procedure),
                        "knowledge" => Some(plico::api::semantic::KnowledgeType::Knowledge),
                        _ => None,
                    }
                }).collect())
                .unwrap_or_default();
            Ok(ApiRequest::DiscoverKnowledge {
                query: query.to_string(),
                scope: scope_enum,
                knowledge_types,
                max_results: params.get("max_results").and_then(|v| v.as_u64()).unwrap_or(10) as usize,
                token_budget: params.get("token_budget").and_then(|v| v.as_u64()).map(|t| t as usize),
                agent_id: agent.to_string(),
            })
        }
        _ => {
            let available = [
                "add_node", "add_edge", "causal_path", "impact",
                "delegate", "complete", "query_task", "batch_create",
                "register", "checkpoint", "restore",
                "subscribe", "poll", "unsubscribe",
                "storage_stats", "object_usage", "evict_expired", "discover_knowledge",
            ];
            Err(format!(
                "unknown cold method: '{}'. Available methods: {}",
                method,
                available.join(", ")
            ))
        }
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
        "discover_knowledge" => "{method:'discover_knowledge', query:'search terms', scope:'shared', knowledge_types:['memory','procedure']}",
        _ => return error.to_string(),
    };

    format!("{}. Example: {}", error, example)
}

fn dispatch_plico_store(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let store_action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("plico_store requires action")?;

    if is_read_only_mode() && STORE_WRITE_ACTIONS.contains(&store_action) {
        return Err(format!("read_only: action '{}' is a write operation. Set PLICO_READ_ONLY=false to allow writes.", store_action));
    }

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

pub fn ensure_builtin_skills(kernel: &AIKernel) {
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

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    fn make_kernel() -> (Arc<AIKernel>, tempfile::TempDir) {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = Arc::new(AIKernel::new(dir.path().to_path_buf()).expect("kernel init"));
        (kernel, dir)
    }

    // ── dispatch_tool ────────────────────────────────────────────────────────

    #[test]
    fn dispatch_tool_plico_routes_to_dispatch_plico() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "status"});
        let result = dispatch_tool("plico", &args, &kernel);
        // "status" is valid → ok, not "unknown action"
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("ok").is_some());
    }

    #[test]
    fn dispatch_tool_plico_store_routes_to_dispatch_plico_store() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "read", "cid": "nonexistent-cid"});
        let result = dispatch_tool("plico_store", &args, &kernel);
        // read with nonexistent cid returns error, not "unknown store action"
        assert!(result.is_err()); // "no such object" or similar
        assert!(!result.unwrap_err().contains("unknown store action"));
    }

    #[test]
    fn dispatch_tool_plico_skills_routes_to_dispatch_plico_skills() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "list"});
        let result = dispatch_tool("plico_skills", &args, &kernel);
        // list returns skills json with "skills" key
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("skills").is_some());
    }

    #[test]
    fn dispatch_tool_unknown_tool_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({});
        let result = dispatch_tool("unknown_tool", &args, &kernel);
        assert_eq!(result.unwrap_err(), "unknown tool: unknown_tool");
    }

    // ── dispatch_plico (pipeline vs single action) ─────────────────────────────

    #[test]
    fn dispatch_plico_missing_action_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({});
        let result = dispatch_plico(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("missing required parameter: action"));
    }

    #[test]
    fn dispatch_plico_unknown_action_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "not_a_real_action"});
        let result = dispatch_plico(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("unknown action: not_a_real_action"));
    }

    // ── dispatch_plico_action ──────────────────────────────────────────────────

    #[test]
    fn dispatch_plico_action_help_returns_actions_list() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "help"});
        let result = dispatch_plico_action("help", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        let actions = out.get("actions").expect("help should return actions array");
        let names: Vec<&str> = actions.as_array().unwrap().iter().filter_map(|a| a.get("name").and_then(|n| n.as_str())).collect();
        assert!(names.contains(&"put"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"status"));
    }

    #[test]
    fn dispatch_plico_action_status_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "status"});
        let result = dispatch_plico_action("status", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("ok").is_some() || out.get("cas_object_count").is_some());
    }

    #[test]
    fn dispatch_plico_action_get_missing_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "get"});
        let result = dispatch_plico_action("get", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("get requires cid"));
    }

    #[test]
    fn dispatch_plico_action_put_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "put",
            "content": "hello from test",
            "agent_id": "test-agent"
        });
        let result = dispatch_plico_action("put", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        // successful put returns an object with ok:true and a cid
        assert_eq!(out.get("ok").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn dispatch_plico_action_put_with_tags() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "put",
            "content": "tagged content",
            "tags": ["test-tag", "unit-test"],
            "agent_id": "test-agent"
        });
        let result = dispatch_plico_action("put", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_get_nonexistent_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "get", "cid": "nonexistent-cid-12345"});
        let result = dispatch_plico_action("get", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("not found") || err.contains("no such object") || err.contains("nonexistent"));
    }

    #[test]
    fn dispatch_plico_action_search_requires_query() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "search"});
        let result = dispatch_plico_action("search", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("search requires query"));
    }

    #[test]
    fn dispatch_plico_action_search_ok_with_empty_query() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "search", "query": "", "agent_id": "test-agent"});
        let result = dispatch_plico_action("search", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_remember_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "remember",
            "content": "remember this thought",
            "agent_id": "test-agent"
        });
        let result = dispatch_plico_action("remember", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_remember_requires_content() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "remember"});
        let result = dispatch_plico_action("remember", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("remember requires content"));
    }

    #[test]
    fn dispatch_plico_action_recall_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "recall", "agent_id": "test-agent"});
        let result = dispatch_plico_action("recall", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_memory_stats_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "memory_stats", "agent_id": "test-agent"});
        let result = dispatch_plico_action("memory_stats", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_growth_unknown_agent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "growth", "agent_id": "nonexistent-agent"});
        let result = dispatch_plico_action("growth", &args, &kernel);
        // Agent not registered → error response (not panick)
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("not found") || err.contains("Agent"));
    }

    #[test]
    fn dispatch_plico_action_session_start_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "session_start",
            "agent_id": "test-agent",
            "intent_hint": "testing session start"
        });
        let result = dispatch_plico_action("session_start", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_session_start_with_handover() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "session_start",
            "agent_id": "test-agent",
            "handover_mode": "compact"
        });
        let result = dispatch_plico_action("session_start", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        // handover mode compact should include handover key
        assert!(out.get("handover").is_some());
    }

    #[test]
    fn dispatch_plico_action_session_end_requires_session_id() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "session_end", "agent_id": "test-agent"});
        let result = dispatch_plico_action("session_end", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("session_end requires session_id"));
    }

    // ── execute_pipeline ───────────────────────────────────────────────────────

    #[test]
    fn execute_pipeline_single_step_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "pipeline": [
                {"step": "s1", "action": "status"}
            ]
        });
        let result = execute_pipeline(&args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("s1").is_some());
    }

    #[test]
    fn execute_pipeline_multiple_steps_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "pipeline": [
                {"step": "step_a", "action": "put", "content": "p1", "agent_id": "test"},
                {"step": "step_b", "action": "status"}
            ]
        });
        let result = execute_pipeline(&args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("step_a").is_some());
        assert!(out.get("step_b").is_some());
    }

    #[test]
    fn execute_pipeline_missing_action_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "pipeline": [
                {"step": "s1"}
            ]
        });
        let result = execute_pipeline(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("missing action"));
    }

    // ── substitute_pipeline_vars ─────────────────────────────────────────────

    #[test]
    fn substitute_pipeline_vars_empty_context_keeps_step() {
        let ctx = std::collections::HashMap::new();
        let step = json!({"action": "status"});
        let result = substitute_pipeline_vars(&step, &ctx);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out.get("action").and_then(|v| v.as_str()), Some("status"));
    }

    #[test]
    fn substitute_pipeline_vars_no_match_keeps_original() {
        let ctx = std::collections::HashMap::new();
        let step = json!({"action": "get", "cid": "unchanged-value"});
        let result = substitute_pipeline_vars(&step, &ctx);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out.get("cid").and_then(|v| v.as_str()), Some("unchanged-value"));
    }

    // ── dispatch_plico_store ──────────────────────────────────────────────────

    #[test]
    fn dispatch_plico_store_put_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "put",
            "content": "store test",
            "agent_id": "test-agent"
        });
        let result = dispatch_plico_store(&args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_store_read_nonexistent_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        // read with nonexistent cid returns an error (not ok)
        let read_args = json!({"action": "read", "cid": "nonexistent-cid", "agent_id": "test-agent"});
        let result = dispatch_plico_store(&read_args, &kernel);
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_plico_store_unknown_action_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "delete"});
        let result = dispatch_plico_store(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("unknown store action: delete"));
    }

    // ── dispatch_plico_skills ────────────────────────────────────────────────

    #[test]
    fn dispatch_plico_skills_list_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "list"});
        let result = dispatch_plico_skills(&args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("skills").is_some());
        assert!(out.get("count").is_some());
    }

    #[test]
    fn dispatch_plico_skills_create_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({
            "action": "create",
            "name": "test-skill",
            "description": "A test skill",
            "steps": [
                {"description": "step one", "action": "plico(action=\"status\")"}
            ],
            "agent_id": "test-agent"
        });
        let result = dispatch_plico_skills(&args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_skills_run_missing_name_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "run"});
        let result = dispatch_plico_skills(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("run requires name"));
    }

    #[test]
    fn dispatch_plico_skills_run_unknown_skill_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "run", "name": "nonexistent-skill-xyz"});
        let result = dispatch_plico_skills(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("no skill named 'nonexistent-skill-xyz' found"));
    }

    // ── generate_help_response ───────────────────────────────────────────────

    #[test]
    fn generate_help_response_contains_expected_actions() {
        let out = generate_help_response();
        let json: serde_json::Value = serde_json::from_str(&out).unwrap();
        let actions = json.get("actions").expect("actions key");
        let names: Vec<&str> = actions.as_array().unwrap()
            .iter()
            .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
            .collect();

        // key actions that must be present
        assert!(names.contains(&"put"));
        assert!(names.contains(&"get"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"status"));
        assert!(names.contains(&"help"));
        assert!(names.contains(&"remember"));
        assert!(names.contains(&"recall"));
    }
}
