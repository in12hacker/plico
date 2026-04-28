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
//!     "env": { "PLICO_ROOT": "~/.plico/dogfood", "EMBEDDING_BACKEND": "stub" } } } }

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use plico::api::semantic::ApiRequest;
use plico::client::{KernelClient, EmbeddedClient, RemoteClient};
use plico::kernel::AIKernel;
use serde_json::Value;
use tracing_subscriber::EnvFilter;

mod dispatch;
mod format;
mod tools;

use dispatch::DEFAULT_AGENT;

const SERVER_NAME: &str = "plico-mcp";
const SERVER_VERSION: &str = "1.0.0";
const PROTOCOL_VERSION: &str = "2024-11-05";


// ── F-32: Rate limiter (sliding window, atomic) ──

static RATE_WINDOW_START: AtomicU64 = AtomicU64::new(0);
static RATE_WINDOW_COUNT: AtomicU64 = AtomicU64::new(0);

fn rate_limit_max() -> u64 {
    std::env::var("PLICO_RATE_LIMIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60) // default 60 req/min
}

pub(crate) fn check_rate_limit() -> Result<(), String> {
    let max = rate_limit_max();
    if max == 0 { return Ok(()); } // 0 = unlimited

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let window = RATE_WINDOW_START.load(Ordering::Relaxed);

    if now - window >= 60 {
        RATE_WINDOW_START.store(now, Ordering::Relaxed);
        RATE_WINDOW_COUNT.store(1, Ordering::Relaxed);
        return Ok(());
    }

    let count = RATE_WINDOW_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if count > max {
        Err(format!("rate_limit: {count}/{max} requests in current minute. Retry after window resets."))
    } else {
        Ok(())
    }
}


fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let args: Vec<String> = std::env::args().collect();
    let daemon_mode = args.iter().any(|a| a == "--daemon");

    let root = std::env::var("PLICO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(".plico")
        });

    // Connect to plicod daemon or embed kernel directly
    let kernel: Arc<dyn KernelClient> = if daemon_mode {
        let sock_path = root.join("plico.sock");
        let client = RemoteClient::uds(sock_path.clone());
        if !client.is_reachable() {
            let tcp_addr = args.iter().position(|a| a == "--addr")
                .and_then(|i| args.get(i + 1).cloned())
                .unwrap_or_else(|| "127.0.0.1:7878".to_string());
            let tcp_client = RemoteClient::tcp(tcp_addr.clone());
            if !tcp_client.is_reachable() {
                let err = make_error_response(Value::Null, -32603,
                    &format!("daemon not reachable at {:?} or tcp://{}", sock_path, tcp_addr));
                let _ = writeln!(io::stdout(), "{}", serde_json::to_string(&err).unwrap());
                std::process::exit(1);
            }
            Arc::new(tcp_client)
        } else {
            Arc::new(client)
        }
    } else {
        match AIKernel::new(root.clone()) {
            Ok(k) => {
                let embedded = EmbeddedClient { kernel: k };
                Arc::new(embedded)
            }
            Err(e) => {
                let err = make_error_response(Value::Null, -32603, &format!("kernel init failed: {e}"));
                let _ = writeln!(io::stdout(), "{}", serde_json::to_string(&err).unwrap());
                std::process::exit(1);
            }
        }
    };

    // Seed built-in skills (only works in embedded mode — needs direct kernel access)
    if !daemon_mode {
        if let Some(embedded) = downcast_embedded(&kernel) {
            dispatch::ensure_builtin_skills(&embedded.kernel);
        }
    }

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
            "tools/call" => {
                if let Some(embedded) = downcast_embedded(&kernel) {
                    handle_tools_call(id, &msg["params"], &embedded.kernel)
                } else {
                    handle_tools_call_remote(id, &msg["params"], kernel.as_ref())
                }
            }
            "resources/list" => {
                if let Some(embedded) = downcast_embedded(&kernel) {
                    handle_resources_list(id, &embedded.kernel)
                } else {
                    handle_resources_list_remote(id, kernel.as_ref())
                }
            }
            "resources/read" => {
                if let Some(embedded) = downcast_embedded(&kernel) {
                    handle_resources_read(id, &msg["params"], &embedded.kernel)
                } else {
                    handle_resources_read_remote(id, &msg["params"], kernel.as_ref())
                }
            }
            "prompts/list" => tools::handle_prompts_list(id),
            "prompts/get" => tools::handle_prompts_get(id, &msg["params"]),
            "ping" => make_result(id, serde_json::json!({})),
            _ => make_error_response(id, -32601, &format!("unknown method: {method}")),
        };

        let _ = writeln!(stdout, "{}", serde_json::to_string(&response).unwrap());
        let _ = stdout.flush();
    }
}

/// Downcast Arc<dyn KernelClient> to EmbeddedClient for direct kernel access.
fn downcast_embedded(client: &Arc<dyn KernelClient>) -> Option<&EmbeddedClient> {
    use std::any::Any;
    (client.as_ref() as &dyn Any).downcast_ref::<EmbeddedClient>()
}

fn handle_initialize(id: Value) -> Value {
    make_result(id, serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {
            "tools": {},
            "resources": {},
            "prompts": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": SERVER_VERSION
        }
    }))
}


fn handle_tools_list(id: Value) -> Value {
    make_result(id, serde_json::json!({ "tools": tools::tool_definitions() }))
}

fn handle_tools_call(id: Value, params: &Value, kernel: &AIKernel) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    let result = dispatch::dispatch_tool(name, &args, kernel);
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

fn handle_resources_list(id: Value, _kernel: &AIKernel) -> Value {
    let resources: Vec<Value> = tools::RESOURCES.iter().map(|(uri, name, mime)| {
        serde_json::json!({
            "uri": uri,
            "name": name,
            "mimeType": mime,
        })
    }).collect();
    make_result(id, serde_json::json!({ "resources": resources }))
}

fn handle_resources_read(id: Value, params: &Value, kernel: &AIKernel) -> Value {
    let uri = params.get("uri")
        .and_then(|u| u.as_str())
        .unwrap_or("");

    let (contents, mime) = match uri {
        "plico://status" => {
            let resp = kernel.handle_api_request(ApiRequest::SystemStatus);
            let json = serde_json::to_value(&resp).unwrap_or(serde_json::Value::Null);
            (serde_json::to_string_pretty(&json).unwrap_or_default(), "application/json")
        }
        "plico://delta" => {
            // Fix: query real delta events from the EventBus
            let resp = kernel.handle_api_request(ApiRequest::DeltaSince {
                agent_id: DEFAULT_AGENT.to_string(),
                since_seq: 0,
                watch_cids: vec![],
                watch_tags: vec![],
                limit: Some(20),
            });
            let delta_json: Value = serde_json::to_value(&resp.delta_result).unwrap_or(Value::Null);
            (serde_json::to_string_pretty(&delta_json).unwrap_or_default(), "application/json")
        }
        "plico://skills" => {
            // Fix: query both shared (system) and private (agent) procedural layers
            let shared_entries = kernel.recall_shared_procedural(None);
            let private_entries = kernel.recall_procedural(DEFAULT_AGENT, plico::DEFAULT_TENANT, None);

            // Combine and deduplicate by name (shared takes precedence for same name)
            let mut skills_map: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
            for e in shared_entries.iter().chain(private_entries.iter()) {
                if let plico::memory::MemoryContent::Procedure(p) = &e.content {
                    skills_map.entry(p.name.clone()).or_insert_with(|| {
                        serde_json::json!({
                            "name": p.name,
                            "description": p.description,
                        })
                    });
                }
            }
            let skills: Vec<Value> = skills_map.into_values().collect();
            (serde_json::to_string_pretty(&serde_json::json!({ "skills": skills })).unwrap_or_default(), "application/json")
        }
        "plico://instructions" => {
            (tools::generate_instructions(), "text/plain")
        }
        "plico://profile" => {
            let profile = tools::generate_content_profile(kernel);
            (serde_json::to_string_pretty(&profile).unwrap_or_default(), "application/json")
        }
        "plico://actions" => {
            (dispatch::generate_help_response(), "application/json")
        }
        _ => {
            return make_error_response(id, -32602, &format!("unknown resource: {uri}"));
        }
    };

    make_result(id, serde_json::json!({
        "contents": [{
            "uri": uri,
            "mimeType": mime,
            "text": contents,
        }]
    }))
}

/// Remote-mode tools/call: dispatch via KernelClient (limited feature set)
fn handle_tools_call_remote(id: Value, params: &Value, client: &dyn KernelClient) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    let result = dispatch::dispatch_tool_remote(name, &args, client);
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

fn handle_resources_list_remote(id: Value, _client: &dyn KernelClient) -> Value {
    let resources: Vec<Value> = tools::RESOURCES.iter().map(|(uri, name, mime)| {
        serde_json::json!({ "uri": uri, "name": name, "mimeType": mime })
    }).collect();
    make_result(id, serde_json::json!({ "resources": resources }))
}

fn handle_resources_read_remote(id: Value, params: &Value, client: &dyn KernelClient) -> Value {
    let uri = params.get("uri").and_then(|u| u.as_str()).unwrap_or("");
    let (contents, mime) = match uri {
        "plico://status" => {
            let resp = client.request(ApiRequest::SystemStatus);
            let json = serde_json::to_value(&resp).unwrap_or(Value::Null);
            (serde_json::to_string_pretty(&json).unwrap_or_default(), "application/json")
        }
        "plico://delta" => {
            let resp = client.request(ApiRequest::DeltaSince {
                agent_id: DEFAULT_AGENT.to_string(),
                since_seq: 0, watch_cids: vec![], watch_tags: vec![], limit: Some(20),
            });
            let delta_json = serde_json::to_value(&resp.delta_result).unwrap_or(Value::Null);
            (serde_json::to_string_pretty(&delta_json).unwrap_or_default(), "application/json")
        }
        "plico://instructions" => (tools::generate_instructions(), "text/plain"),
        "plico://actions" => (dispatch::generate_help_response(), "application/json"),
        _ => {
            return make_error_response(id, -32602, &format!("resource '{uri}' not available in daemon mode"));
        }
    };
    make_result(id, serde_json::json!({
        "contents": [{ "uri": uri, "mimeType": mime, "text": contents }]
    }))
}

pub(crate) fn make_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}


pub(crate) fn make_error_response(id: Value, code: i32, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::dispatch::dispatch_tool;
    use super::tools::tool_definitions;

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

    #[test]
    fn resources_list_returns_all_resources() {
        let kernel = make_test_kernel();
        let resp = handle_resources_list(serde_json::json!(1), &kernel);
        let resources = resp["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 6);
        let uris: Vec<&str> = resources.iter().map(|r| r["uri"].as_str().unwrap()).collect();
        assert!(uris.contains(&"plico://status"));
        assert!(uris.contains(&"plico://delta"));
        assert!(uris.contains(&"plico://skills"));
        assert!(uris.contains(&"plico://instructions"));
        assert!(uris.contains(&"plico://profile"));
        assert!(uris.contains(&"plico://actions"));
    }

    #[test]
    fn resources_read_status_returns_json() {
        let kernel = make_test_kernel();
        let resp = handle_resources_read(
            serde_json::json!(1),
            &serde_json::json!({"uri": "plico://status"}),
            &kernel,
        );
        assert!(resp["result"]["contents"].is_array());
        let contents = resp["result"]["contents"][0].as_object().unwrap();
        assert_eq!(contents["mimeType"], "application/json");
        assert!(!contents["text"].as_str().unwrap().is_empty());
    }

    #[test]
    fn resources_read_delta_returns_delta_result() {
        let kernel = make_test_kernel();
        let resp = handle_resources_read(
            serde_json::json!(1),
            &serde_json::json!({"uri": "plico://delta"}),
            &kernel,
        );
        assert!(resp["result"]["contents"].is_array());
        let contents = resp["result"]["contents"][0].as_object().unwrap();
        assert_eq!(contents["mimeType"], "application/json");
        // After fix, plico://delta returns a real DeltaResult (empty in fresh kernel)
        let text = contents["text"].as_str().unwrap();
        // Should contain delta result fields (from_seq, to_seq, changes)
        assert!(text.contains("from_seq") || text.contains("changes") || text.is_empty(),
            "Expected delta result fields, got: {}", text);
    }

    #[test]
    fn resources_read_skills_returns_skills_list() {
        let kernel = make_test_kernel();
        let resp = handle_resources_read(
            serde_json::json!(1),
            &serde_json::json!({"uri": "plico://skills"}),
            &kernel,
        );
        assert!(resp["result"]["contents"].is_array());
        let contents = resp["result"]["contents"][0].as_object().unwrap();
        assert_eq!(contents["mimeType"], "application/json");
        let text = contents["text"].as_str().unwrap();
        assert!(text.contains("skills"));
    }

    #[test]
    fn resources_read_unknown_returns_error() {
        let kernel = make_test_kernel();
        let resp = handle_resources_read(
            serde_json::json!(1),
            &serde_json::json!({"uri": "plico://unknown"}),
            &kernel,
        );
        assert!(resp["error"].is_object());
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[test]
    fn initialize_advertises_resources_capability() {
        let resp = handle_initialize(serde_json::json!(1));
        assert!(resp["result"]["capabilities"]["resources"].is_object(),
            "initialize should advertise resources capability");
    }
}