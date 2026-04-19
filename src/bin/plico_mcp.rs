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

use plico::api::semantic::{ApiRequest, ApiResponse};
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
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);

    match name {
        "plico_search" => {
            let query = args.get("query").and_then(|q| q.as_str())
                .ok_or("missing required parameter: query")?;
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
            format_response(kernel.handle_api_request(req))
        }

        "plico_put" => {
            let content = args.get("content").and_then(|c| c.as_str())
                .ok_or("missing required parameter: content")?;
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

        "plico_read" => {
            let cid = args.get("cid").and_then(|c| c.as_str())
                .ok_or("missing required parameter: cid")?;
            let req = ApiRequest::Read {
                cid: cid.to_string(),
                agent_id: agent.to_string(),
                tenant_id: None,
                agent_token: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        "plico_nodes" => {
            let node_type = args.get("node_type").and_then(|t| t.as_str())
                .and_then(|t| serde_json::from_value(serde_json::json!(t)).ok());
            let limit = args.get("limit").and_then(|l| l.as_u64()).map(|l| l as usize);

            let req = ApiRequest::ListNodes {
                node_type,
                agent_id: agent.to_string(),
                tenant_id: None,
                limit,
                offset: None,
            };
            format_response(kernel.handle_api_request(req))
        }

        "plico_tags" => {
            let tags = kernel.list_tags();
            Ok(serde_json::to_string_pretty(&tags).unwrap_or_default())
        }

        "plico_skills_list" => {
            let entries = kernel.recall_procedural(agent, "default", None);
            let skills: Vec<Value> = entries.iter().filter_map(|e| {
                if let plico::memory::MemoryContent::Procedure(p) = &e.content {
                    Some(serde_json::json!({
                        "name": p.name,
                        "description": p.description,
                        "steps_count": p.steps.len(),
                        "learned_from": p.learned_from,
                        "tags": e.tags,
                    }))
                } else {
                    None
                }
            }).collect();
            Ok(serde_json::to_string_pretty(&serde_json::json!({
                "skills": skills,
                "count": skills.len(),
            })).unwrap_or_default())
        }

        "plico_skills_run" => {
            let name = args.get("name").and_then(|n| n.as_str())
                .ok_or("missing required parameter: name")?;
            let entries = kernel.recall_procedural(agent, "default", Some(name));
            if entries.is_empty() {
                return Err(format!("no skill named '{}' found for agent '{}'", name, agent));
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

        _ => Err(format!("unknown tool: {name}")),
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
        serde_json::json!({
            "name": "plico_search",
            "description": "Search Plico's content-addressed storage using BM25 keyword search and optional semantic vectors. Returns ranked results with CIDs and relevance scores.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (keywords or natural language)" },
                    "limit": { "type": "number", "description": "Maximum results to return (default: 10)" },
                    "require_tags": { "type": "array", "items": { "type": "string" }, "description": "Only return results with ALL of these tags" },
                    "exclude_tags": { "type": "array", "items": { "type": "string" }, "description": "Exclude results with ANY of these tags" },
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "plico_put",
            "description": "Store content in Plico's CAS (content-addressed storage). Returns the content ID (CID) — a SHA-256 hash. Identical content always gets the same CID (deduplication).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "content": { "type": "string", "description": "Content to store (UTF-8 text)" },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Semantic tags for classification and retrieval" },
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                },
                "required": ["content"]
            }
        }),
        serde_json::json!({
            "name": "plico_read",
            "description": "Read content from Plico's CAS by content ID (CID). Returns the stored text and its metadata.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "cid": { "type": "string", "description": "Content ID (SHA-256 hash)" },
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                },
                "required": ["cid"]
            }
        }),
        serde_json::json!({
            "name": "plico_nodes",
            "description": "List nodes in Plico's Knowledge Graph. Nodes are typed as entity, fact, event, or tag. Returns node IDs, labels, types, and properties.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "node_type": { "type": "string", "enum": ["entity", "fact", "event", "tag"], "description": "Filter by node type" },
                    "limit": { "type": "number", "description": "Maximum nodes to return" },
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                }
            }
        }),
        serde_json::json!({
            "name": "plico_tags",
            "description": "List all tags currently in use across Plico's CAS. Tags follow the convention plico:<dimension>:<value> for structured classification.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "Agent ID (not used, included for consistency)" }
                }
            }
        }),
        serde_json::json!({
            "name": "plico_skills_list",
            "description": "List all learned procedural skills (workflows) stored in Plico's procedural memory. Skills are learned workflows that agents have discovered and persisted for reuse. Returns skill names, descriptions, step counts, and provenance.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                }
            }
        }),
        serde_json::json!({
            "name": "plico_skills_run",
            "description": "Retrieve a learned procedural skill by name and return its full step-by-step workflow. The calling agent can then execute the steps. This enables cognitive reuse — skills learned by one agent session are available to future sessions.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name of the skill to retrieve" },
                    "agent_id": { "type": "string", "description": "Agent ID (default: mcp-agent)" }
                },
                "required": ["name"]
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
        assert_eq!(tools.len(), 7);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"plico_search"));
        assert!(names.contains(&"plico_put"));
        assert!(names.contains(&"plico_read"));
        assert!(names.contains(&"plico_nodes"));
        assert!(names.contains(&"plico_tags"));
        assert!(names.contains(&"plico_skills_list"));
        assert!(names.contains(&"plico_skills_run"));
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
    fn dispatch_search_missing_query_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_search", &serde_json::json!({}), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("query"));
    }

    #[test]
    fn dispatch_put_and_read_roundtrip() {
        let kernel = make_test_kernel();

        let put_result = dispatch_tool("plico_put", &serde_json::json!({
            "content": "MCP test content",
            "tags": ["mcp-test"]
        }), &kernel);
        assert!(put_result.is_ok(), "put failed: {:?}", put_result);
        let put_json: Value = serde_json::from_str(&put_result.unwrap()).unwrap();
        let cid = put_json["cid"].as_str().unwrap();

        let read_result = dispatch_tool("plico_read", &serde_json::json!({
            "cid": cid
        }), &kernel);
        assert!(read_result.is_ok(), "read failed: {:?}", read_result);
        let read_json: Value = serde_json::from_str(&read_result.unwrap()).unwrap();
        assert_eq!(read_json["data"].as_str().unwrap(), "MCP test content");
    }

    #[test]
    fn dispatch_search_finds_stored_content() {
        let kernel = make_test_kernel();

        dispatch_tool("plico_put", &serde_json::json!({
            "content": "Dijkstra weighted path algorithm",
            "tags": ["plico:type:experience", "plico:module:graph"]
        }), &kernel).unwrap();

        let result = dispatch_tool("plico_search", &serde_json::json!({
            "query": "Dijkstra weighted path"
        }), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        let results = json["results"].as_array().unwrap();
        assert!(!results.is_empty(), "search should find stored content");
    }

    #[test]
    fn dispatch_tags_returns_stored_tags() {
        let kernel = make_test_kernel();

        dispatch_tool("plico_put", &serde_json::json!({
            "content": "test",
            "tags": ["plico:type:adr", "plico:module:kernel"]
        }), &kernel).unwrap();

        let result = dispatch_tool("plico_tags", &serde_json::json!({}), &kernel);
        assert!(result.is_ok());
        let tags: Vec<String> = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(tags.contains(&"plico:type:adr".to_string()));
    }

    #[test]
    fn dispatch_nodes_returns_ok() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_nodes", &serde_json::json!({}), &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn tools_call_response_has_content_array() {
        let kernel = make_test_kernel();
        let resp = handle_tools_call(
            serde_json::json!(5),
            &serde_json::json!({"name": "plico_tags", "arguments": {}}),
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
            &serde_json::json!({"name": "plico_search", "arguments": {}}),
            &kernel,
        );
        assert_eq!(resp["result"]["isError"], true);
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
        let result = dispatch_tool("plico_skills_list", &serde_json::json!({}), &kernel);
        assert!(result.is_ok());
        let json: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["count"], 0);
        assert_eq!(json["skills"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn dispatch_skills_list_finds_stored_skill() {
        let kernel = make_test_kernel();
        store_test_skill(&kernel);
        let result = dispatch_tool("plico_skills_list", &serde_json::json!({}), &kernel);
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
        let result = dispatch_tool("plico_skills_run", &serde_json::json!({
            "name": "bootstrap-module"
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
        let result = dispatch_tool("plico_skills_run", &serde_json::json!({}), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("name"));
    }

    #[test]
    fn dispatch_skills_run_not_found_returns_error() {
        let kernel = make_test_kernel();
        let result = dispatch_tool("plico_skills_run", &serde_json::json!({
            "name": "nonexistent-skill"
        }), &kernel);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonexistent-skill"));
    }
}
