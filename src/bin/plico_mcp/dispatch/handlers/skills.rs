//! plico_skills handler — list, run, create skills + seed built-in skills.

use plico::api::semantic::{ApiRequest, ProcedureStepDto};
use plico::kernel::AIKernel;
use serde_json::Value;

use crate::dispatch::DEFAULT_AGENT;
use crate::format::format_response;

pub(in crate::dispatch) fn dispatch_plico_skills(args: &Value, kernel: &AIKernel) -> Result<String, String> {
    let agent = args.get("agent_id").and_then(|a| a.as_str()).unwrap_or(DEFAULT_AGENT);
    let skill_action = args.get("action")
        .and_then(|a| a.as_str())
        .ok_or("plico_skills requires action")?;

    match skill_action {
        "list" => {
            let private_entries = kernel.recall_procedural(agent, "default", None);
            let shared_entries = kernel.recall_shared_procedural(None);

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

            let steps: Vec<ProcedureStepDto> = steps_json
                .map(|arr| {
                    arr.iter().map(|s| {
                        ProcedureStepDto {
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

pub fn ensure_builtin_skills(kernel: &AIKernel) {
    let existing = kernel.recall_shared_procedural(None);
    let has_builtins = existing.iter().any(|e| {
        e.tags.contains(&"plico:builtin".to_string())
    });
    if has_builtins {
        return;
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
