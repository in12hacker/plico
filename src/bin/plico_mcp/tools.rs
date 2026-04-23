//! MCP tool definitions, instructions, content profile, and prompt templates.

use plico::kernel::AIKernel;
use serde_json::Value;

use super::{make_result, make_error_response};

// ── F-35: MCP Prompts — Skills as prompt templates ──

const MCP_PROMPTS: &[(&str, &str, &[(&str, &str, bool)])] = &[
    ("debug-issue", "Search related memories + causal chain + propose solution",
     &[("issue", "Description of the issue to debug", true)]),
    ("store-experience", "Store an experience with context: put + remember + causal link",
     &[("content", "What you learned", true), ("tags", "Comma-separated tags", false)]),
    ("project-review", "Start session + growth report + recent delta for project overview",
     &[("period", "Time period: 7d, 30d, or all", false)]),
    ("handover", "Generate a handover summary for the next agent session",
     &[("intent", "What the next session should focus on", false)]),
];

pub fn handle_prompts_list(id: Value) -> Value {
    let prompts: Vec<Value> = MCP_PROMPTS.iter().map(|(name, desc, args)| {
        let arguments: Vec<Value> = args.iter().map(|(aname, adesc, required)| {
            serde_json::json!({ "name": aname, "description": adesc, "required": required })
        }).collect();
        serde_json::json!({ "name": name, "description": desc, "arguments": arguments })
    }).collect();
    make_result(id, serde_json::json!({ "prompts": prompts }))
}

pub fn handle_prompts_get(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(serde_json::json!({}));

    let messages = match name {
        "debug-issue" => {
            let issue = args.get("issue").and_then(|v| v.as_str()).unwrap_or("unknown issue");
            vec![serde_json::json!({
                "role": "user",
                "content": { "type": "text", "text": format!(
                    "I need to debug: {issue}\n\n\
                    Steps:\n\
                    1. plico(action=\"search\", query=\"{issue}\", agent_id=\"debug\") — find related memories\n\
                    2. plico_cold(method=\"causal_path\", params={{\"from\":\"<relevant_node>\",\"to\":\"<issue_node>\"}}) — trace cause\n\
                    3. Based on findings, propose a fix and store the experience:\n\
                       plico(action=\"remember\", content=\"<solution>\", agent_id=\"debug\")"
                )}
            })]
        }
        "store-experience" => {
            let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let tags = args.get("tags").and_then(|v| v.as_str()).unwrap_or("experience");
            let tag_list: Vec<String> = tags.split(',').map(|t| format!("\"{}\"", t.trim())).collect();
            vec![serde_json::json!({
                "role": "user",
                "content": { "type": "text", "text": format!(
                    "Store this experience in Plico:\n\n\
                    Content: {content}\n\
                    Tags: {tags}\n\n\
                    Execute:\n\
                    1. plico(action=\"put\", content=\"{content}\", tags=[{tag_csv}], agent_id=\"experience\")\n\
                    2. plico(action=\"remember\", content=\"{content}\", agent_id=\"experience\", scope=\"shared\")",
                    tag_csv = tag_list.join(",")
                )}
            })]
        }
        "project-review" => {
            let period = args.get("period").and_then(|v| v.as_str()).unwrap_or("7d");
            vec![serde_json::json!({
                "role": "user",
                "content": { "type": "text", "text": format!(
                    "Give me a project review using Plico:\n\n\
                    1. plico(action=\"session_start\", agent_id=\"reviewer\", handover_mode=\"full\")\n\
                    2. plico(action=\"growth\", agent_id=\"reviewer\", period=\"{period}\")\n\
                    3. plico(action=\"delta\", agent_id=\"reviewer\")\n\
                    4. Summarize: what changed, what's healthy, what needs attention"
                )}
            })]
        }
        "handover" => {
            let intent = args.get("intent").and_then(|v| v.as_str()).unwrap_or("continue work");
            vec![serde_json::json!({
                "role": "user",
                "content": { "type": "text", "text": format!(
                    "Generate a handover for the next agent:\n\n\
                    1. plico(action=\"session_start\", agent_id=\"handover\", handover_mode=\"full\", intent_hint=\"{intent}\")\n\
                    2. Review the handover.recent_objects and handover.kg_causal_edges\n\
                    3. Summarize: what was done, what's pending, key decisions, known issues"
                )}
            })]
        }
        _ => {
            return make_error_response(id, -32602, &format!("unknown prompt: {name}"));
        }
    };

    make_result(id, serde_json::json!({ "messages": messages }))
}

/// MCP Resources definitions: URI, name, mimeType
pub const RESOURCES: &[(&str, &str, &str)] = &[
    ("plico://status", "System health and active sessions", "application/json"),
    ("plico://delta", "Changes since last session", "application/json"),
    ("plico://skills", "Available skills", "application/json"),
    ("plico://instructions", "How to use Plico (read first)", "text/plain"),
    ("plico://profile", "Content profile — what Plico contains", "application/json"),
    ("plico://actions", "All available actions with params (zero round-trip)", "application/json"),
];

// ── F-28: Consumer instructions ──

pub fn generate_instructions() -> String {
    r#"# Plico — AI Agent Memory & Knowledge OS

## Three Tools
- plico(action, ...) — main gateway: session, memory, search, pipeline
- plico_cold(method, ...) — advanced: KG, storage stats, cold-layer ops
- plico_skills(action, ...) — skill lifecycle: list, execute, register

## Core Actions (plico tool)
| Action | Purpose | Key Params |
|--------|---------|------------|
| session_start | Begin work session | agent, intent_hint, last_seen_seq |
| session_end | End session + checkpoint | agent, session_id |
| put | Store content | content, tags |
| get | Retrieve by CID | cid |
| search | Semantic + keyword search | query, tags, limit |
| remember | Store to memory tier | content, tier, scope |
| recall | Retrieve memories | tier, scope, query |
| recall_semantic | Semantic memory search | query, limit |
| pipeline | Batch sequential ops | pipeline=[{step,action,...}] |
| delta | Changes since seq N | since, watch_tags |
| growth | Agent growth report | period (7d/30d/all) |
| hybrid | Graph-RAG retrieval | query, seed_tags, depth |
| discover | Browse shared knowledge | scope, knowledge_types |
| memory_stats | Memory tier statistics | tier |
| intent_declare | Prefetch context | intent, token_budget |
| intent_fetch | Get prefetched context | assembly_id |

## Scoping Model
- agent: your agent ID (auto-registered on first call)
- tenant_id: isolation boundary (default: "default")
- CAS objects visible to all agents within same tenant

## Best Patterns
1. Start with session_start(intent_hint="your goal") — gets delta + prefetch
2. Search before put — avoid storing duplicates
3. Use pipeline for batch ops — saves round-trips
4. Use select/preview on search — reduces token cost
5. End with session_end — auto-checkpoints your state
6. Use remember(scope="shared") for team-visible knowledge

## Token Saving
- select:["cid","tags","summary"] — field projection on search results
- preview:200 — truncate content to N chars
- token_budget:2000 — cap context assembly size
- limit:5 — fewer results = fewer tokens"#
        .to_string()
}

// ── F-29: Content profile ──

pub fn generate_content_profile(kernel: &AIKernel) -> Value {
    let status = kernel.system_status();
    let tags = kernel.list_tags();

    let mut tag_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for tag in &tags {
        let prefix = tag.split(':').take(2).collect::<Vec<&str>>().join(":");
        *tag_counts.entry(prefix).or_default() += 1;
    }
    let mut sorted_tags: Vec<_> = tag_counts.into_iter().collect();
    sorted_tags.sort_by(|a, b| b.1.cmp(&a.1));
    sorted_tags.truncate(20);

    let agents = kernel.list_agents();
    let agent_list: Vec<Value> = agents.iter().map(|a| {
        serde_json::json!({
            "id": a.name,
            "state": format!("{:?}", a.state),
        })
    }).collect();

    // KG edge type distribution
    let kg_summary = if let Some(kg) = kernel.knowledge_graph() {
        let mut edge_types: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        if let Ok(edges) = kg.get_valid_edges_at(u64::MAX) {
            for e in &edges {
                *edge_types.entry(format!("{:?}", e.edge_type)).or_default() += 1;
            }
        }
        serde_json::json!({
            "nodes": status.kg_node_count,
            "edges": status.kg_edge_count,
            "edge_types": edge_types,
        })
    } else {
        serde_json::json!({ "nodes": 0, "edges": 0, "edge_types": {} })
    };

    serde_json::json!({
        "summary": {
            "cas_objects": status.cas_object_count,
            "agents": status.agent_count,
            "unique_tags": tags.len(),
            "kg_nodes": status.kg_node_count,
            "kg_edges": status.kg_edge_count,
        },
        "top_tags": sorted_tags.iter().map(|(k, v)| serde_json::json!({"tag": k, "count": v})).collect::<Vec<_>>(),
        "agents": agent_list,
        "kg_summary": kg_summary,
        "health": status.health,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_definitions_returns_three_tools() {
        let tools = tool_definitions();
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"plico"));
        assert!(names.contains(&"plico_store"));
        assert!(names.contains(&"plico_skills"));
    }

    #[test]
    fn all_tools_have_input_schema() {
        for tool in tool_definitions() {
            assert!(tool["inputSchema"]["type"].as_str() == Some("object"),
                "tool {} must have object inputSchema", tool["name"]);
        }
    }

    #[test]
    fn resources_have_valid_uris() {
        for (uri, name, mime) in RESOURCES {
            assert!(uri.starts_with("plico://"), "URI should start with plico://: {}", uri);
            assert!(!name.is_empty(), "name should not be empty");
            assert!(!mime.is_empty(), "mime should not be empty");
        }
        assert_eq!(RESOURCES.len(), 6);
    }

    #[test]
    fn generate_instructions_non_empty() {
        let instructions = generate_instructions();
        assert!(instructions.contains("plico"));
        assert!(instructions.contains("session_start"));
        assert!(instructions.len() > 500);
    }

    #[test]
    fn handle_prompts_list_returns_all_prompts() {
        let resp = handle_prompts_list(serde_json::json!(1));
        let prompts = resp["result"]["prompts"].as_array().unwrap();
        assert_eq!(prompts.len(), 4);
        let names: Vec<&str> = prompts.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"debug-issue"));
        assert!(names.contains(&"store-experience"));
        assert!(names.contains(&"project-review"));
        assert!(names.contains(&"handover"));
    }

    #[test]
    fn handle_prompts_get_debug_issue() {
        let params = serde_json::json!({
            "name": "debug-issue",
            "arguments": {"issue": "auth token expired"}
        });
        let resp = handle_prompts_get(serde_json::json!(1), &params);
        let messages = resp["result"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        let text = messages[0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("auth token expired"));
    }

    #[test]
    fn handle_prompts_get_unknown_returns_error() {
        let params = serde_json::json!({"name": "nonexistent"});
        let resp = handle_prompts_get(serde_json::json!(1), &params);
        assert!(resp["error"].is_object());
    }

    #[test]
    fn handle_prompts_get_store_experience() {
        let params = serde_json::json!({
            "name": "store-experience",
            "arguments": {"content": "learned X", "tags": "rust,testing"}
        });
        let resp = handle_prompts_get(serde_json::json!(1), &params);
        let messages = resp["result"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
    }

    #[test]
    fn handle_prompts_get_project_review() {
        let params = serde_json::json!({"name": "project-review"});
        let resp = handle_prompts_get(serde_json::json!(1), &params);
        assert!(resp["result"]["messages"].is_array());
    }

    #[test]
    fn handle_prompts_get_handover() {
        let params = serde_json::json!({
            "name": "handover",
            "arguments": {"intent": "fix CI"}
        });
        let resp = handle_prompts_get(serde_json::json!(1), &params);
        let text = resp["result"]["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("fix CI"));
    }
}

pub fn tool_definitions() -> Vec<Value> {
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
                        "enum": ["session_start","session_end","put","get","remember","recall","recall_semantic",
                                 "search","hybrid","intent_declare","intent_fetch","delta","growth","status",
                                 "discover","memory_stats","help"],
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
                    "handover_mode": { "type": "string", "enum": ["full", "compact", "none"], "description": "For session_start: context assembly mode for session recovery" },
                    "session_id": { "type": "string", "description": "For session_end" },
                    "cid": { "type": "string", "description": "For get: content ID to retrieve" },
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
