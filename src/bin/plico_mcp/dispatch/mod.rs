//! Action dispatchers — routes MCP tool calls to Plico kernel API.

use plico::kernel::AIKernel;
use serde_json::Value;

use super::check_rate_limit;

mod handlers;

pub const DEFAULT_AGENT: &str = "mcp-agent";

pub fn dispatch_tool(name: &str, args: &Value, kernel: &AIKernel) -> Result<String, String> {
    check_rate_limit()?;
    match name {
        "plico" => dispatch_plico(args, kernel),
        "plico_store" => handlers::store::dispatch_plico_store(args, kernel),
        "plico_skills" => handlers::skills::dispatch_plico_skills(args, kernel),
        _ => Err(format!("unknown tool: {name}")),
    }
}

pub fn dispatch_tool_remote(name: &str, args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    check_rate_limit()?;
    match name {
        "plico" => dispatch_plico_remote(args, client),
        "plico_store" => handlers::store::dispatch_plico_store_remote(args, client),
        "plico_skills" => Err("plico_skills not available in daemon mode (requires direct kernel access)".to_string()),
        _ => Err(format!("unknown tool: {name}")),
    }
}

fn dispatch_plico(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    if args.get("pipeline").is_some() {
        return handlers::pipeline::execute_pipeline(args, kernel);
    }

    let action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing required parameter: action")?;

    handlers::action::dispatch_plico_action(action, args, kernel)
}

fn dispatch_plico_remote(args: &Value, client: &dyn plico::client::KernelClient) -> Result<String, String> {
    let action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("missing required parameter: action")?;
    handlers::action::dispatch_plico_action_remote(action, args, client)
}

pub fn ensure_builtin_skills(kernel: &AIKernel) {
    handlers::skills::ensure_builtin_skills(kernel);
}

// ── Action registry — data-driven action metadata ──

pub(crate) struct ActionMeta {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [(&'static str, &'static str, bool)],
    pub is_write: bool,
}

pub(crate) const PLICO_ACTIONS: &[ActionMeta] = &[
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

pub(crate) const STORE_WRITE_ACTIONS: &[&str] = &["put"];

pub(crate) fn is_read_only_mode() -> bool {
    std::env::var("PLICO_READ_ONLY").map(|v| v == "true" || v == "1").unwrap_or(false)
}

pub(crate) fn check_read_only(action: &str, registry: &[ActionMeta]) -> Result<(), String> {
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
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("ok").is_some());
    }

    #[test]
    fn dispatch_tool_plico_store_routes_to_dispatch_plico_store() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "read", "cid": "nonexistent-cid"});
        let result = dispatch_tool("plico_store", &args, &kernel);
        assert!(result.is_err());
        assert!(!result.unwrap_err().contains("unknown store action"));
    }

    #[test]
    fn dispatch_tool_plico_skills_routes_to_dispatch_plico_skills() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "list"});
        let result = dispatch_tool("plico_skills", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("help", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("status", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("ok").is_some() || out.get("cas_object_count").is_some());
    }

    #[test]
    fn dispatch_plico_action_get_missing_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "get"});
        let result = handlers::action::dispatch_plico_action("get", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("put", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
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
        let result = handlers::action::dispatch_plico_action("put", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_get_nonexistent_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "get", "cid": "nonexistent-cid-12345"});
        let result = handlers::action::dispatch_plico_action("get", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("not found") || err.contains("no such object") || err.contains("nonexistent"));
    }

    #[test]
    fn dispatch_plico_action_search_requires_query() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "search"});
        let result = handlers::action::dispatch_plico_action("search", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("search requires query"));
    }

    #[test]
    fn dispatch_plico_action_search_ok_with_empty_query() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "search", "query": "", "agent_id": "test-agent"});
        let result = handlers::action::dispatch_plico_action("search", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("remember", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_remember_requires_content() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "remember"});
        let result = handlers::action::dispatch_plico_action("remember", &args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("remember requires content"));
    }

    #[test]
    fn dispatch_plico_action_recall_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "recall", "agent_id": "test-agent"});
        let result = handlers::action::dispatch_plico_action("recall", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_memory_stats_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "memory_stats", "agent_id": "test-agent"});
        let result = handlers::action::dispatch_plico_action("memory_stats", &args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_action_growth_unknown_agent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "growth", "agent_id": "nonexistent-agent"});
        let result = handlers::action::dispatch_plico_action("growth", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("session_start", &args, &kernel);
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
        let result = handlers::action::dispatch_plico_action("session_start", &args, &kernel);
        assert!(result.is_ok());
        let out: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert!(out.get("handover").is_some());
    }

    #[test]
    fn dispatch_plico_action_session_end_requires_session_id() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "session_end", "agent_id": "test-agent"});
        let result = handlers::action::dispatch_plico_action("session_end", &args, &kernel);
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
        let result = handlers::pipeline::execute_pipeline(&args, &kernel);
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
        let result = handlers::pipeline::execute_pipeline(&args, &kernel);
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
        let result = handlers::pipeline::execute_pipeline(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("missing action"));
    }

    // ── substitute_pipeline_vars ─────────────────────────────────────────────

    #[test]
    fn substitute_pipeline_vars_empty_context_keeps_step() {
        let ctx = std::collections::HashMap::new();
        let step = json!({"action": "status"});
        let result = handlers::pipeline::substitute_pipeline_vars(&step, &ctx);
        assert!(result.is_ok());
        let out = result.unwrap();
        assert_eq!(out.get("action").and_then(|v| v.as_str()), Some("status"));
    }

    #[test]
    fn substitute_pipeline_vars_no_match_keeps_original() {
        let ctx = std::collections::HashMap::new();
        let step = json!({"action": "get", "cid": "unchanged-value"});
        let result = handlers::pipeline::substitute_pipeline_vars(&step, &ctx);
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
        let result = handlers::store::dispatch_plico_store(&args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_store_read_nonexistent_cid_returns_error() {
        let (kernel, _dir) = make_kernel();
        let read_args = json!({"action": "read", "cid": "nonexistent-cid", "agent_id": "test-agent"});
        let result = handlers::store::dispatch_plico_store(&read_args, &kernel);
        assert!(result.is_err());
    }

    #[test]
    fn dispatch_plico_store_unknown_action_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "delete"});
        let result = handlers::store::dispatch_plico_store(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("unknown store action: delete"));
    }

    // ── dispatch_plico_skills ────────────────────────────────────────────────

    #[test]
    fn dispatch_plico_skills_list_ok() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "list"});
        let result = handlers::skills::dispatch_plico_skills(&args, &kernel);
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
        let result = handlers::skills::dispatch_plico_skills(&args, &kernel);
        assert!(result.is_ok());
    }

    #[test]
    fn dispatch_plico_skills_run_missing_name_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "run"});
        let result = handlers::skills::dispatch_plico_skills(&args, &kernel);
        let err = result.unwrap_err();
        assert!(err.contains("run requires name"));
    }

    #[test]
    fn dispatch_plico_skills_run_unknown_skill_returns_error() {
        let (kernel, _dir) = make_kernel();
        let args = json!({"action": "run", "name": "nonexistent-skill-xyz"});
        let result = handlers::skills::dispatch_plico_skills(&args, &kernel);
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

        assert!(names.contains(&"put"));
        assert!(names.contains(&"get"));
        assert!(names.contains(&"search"));
        assert!(names.contains(&"status"));
        assert!(names.contains(&"help"));
        assert!(names.contains(&"remember"));
        assert!(names.contains(&"recall"));
    }
}
