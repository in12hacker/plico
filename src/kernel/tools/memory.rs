//! Memory tool handlers (store, recall, forget, procedural).

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "memory.store" => handle_store(kernel, params, agent_id),
        "memory.recall" => handle_recall(kernel, params, agent_id),
        "memory.forget" => {
            kernel.forget_ephemeral(agent_id);
            ToolResult::ok(json!({"forgotten": true}))
        }
        "memory.store_procedure" => handle_store_procedure(kernel, params, agent_id),
        "memory.recall_procedure" => handle_recall_procedure(kernel, params, agent_id),
        _ => ToolResult::error(format!("unknown memory tool: {}", name)),
    }
}

fn handle_store(kernel: &AIKernel, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tier = params.get("tier").and_then(|v| v.as_str()).unwrap_or("working");
    let tags: Vec<String> = params.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let importance = params.get("importance").and_then(|v| v.as_u64()).unwrap_or(50) as u8;

    match tier {
        "working" => {
            match kernel.remember_working(agent_id, "default", content, tags) {
                Ok(()) => ToolResult::ok(json!({"id": "", "tier": "working"})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "long-term" => {
            match kernel.remember_long_term(agent_id, "default", content, tags.clone(), importance) {
                Ok(id) => {
                    kernel.link_memory_to_kg(&id, agent_id, "default", &tags);
                    ToolResult::ok(json!({"id": id, "tier": "long-term"}))
                }
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "procedural" => {
            let steps = vec![crate::memory::layered::ProcedureStep {
                step_number: 0,
                description: content.clone(),
                action: content.clone(),
                expected_outcome: String::new(),
            }];
            match kernel.remember_procedural(agent_id, "default", crate::kernel::ops::memory::ProceduralEntry {
                name: "tool-procedure".into(), description: content, steps, learned_from: "tool".into(), tags: tags.clone(),
            }) {
                Ok(id) => {
                    kernel.link_memory_to_kg(&id, agent_id, "default", &tags);
                    ToolResult::ok(json!({"id": id, "tier": "procedural"}))
                }
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        _ => {
            let ttl_ms = params.get("ttl_ms").and_then(|v| v.as_u64());
            let entry_id = uuid::Uuid::new_v4().to_string();
            let now = crate::memory::layered::now_ms();
            let entry = crate::memory::MemoryEntry {
                id: entry_id.clone(),
                agent_id: agent_id.to_string(),
                tenant_id: "default".to_string(),
                tier: crate::memory::MemoryTier::Ephemeral,
                content: crate::memory::MemoryContent::Text(content),
                importance,
                access_count: 0,
                last_accessed: now,
                created_at: now,
                tags: tags.clone(),
                embedding: None,
                ttl_ms,
                original_ttl_ms: ttl_ms,
                scope: crate::memory::MemoryScope::Private,
                memory_type: crate::memory::MemoryType::default(),
                causal_parent: None,
                supersedes: None,
            };
            let aid = crate::scheduler::AgentId(agent_id.to_string());
            let quota = kernel.scheduler.get_resources(&aid)
                .map(|r| r.memory_quota)
                .unwrap_or(0);
            match kernel.memory.store_checked(entry, quota) {
                Ok(()) => {
                    kernel.persist_memories();
                    ToolResult::ok(json!({"id": entry_id, "tier": "ephemeral"}))
                }
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
    }
}

fn handle_recall(kernel: &AIKernel, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    let param_agent_id = params.get("agent_id").and_then(|v| v.as_str());
    let effective_agent = if let Some(name_or_id) = param_agent_id {
        match kernel.resolve_agent(name_or_id) {
            Some(id) => id,
            None => {
                let available: Vec<String> = kernel.scheduler.list_agents().into_iter().map(|h| h.name).collect();
                return ToolResult::error(format!(
                    "Contract violation: agent '{}' not found. Available agents: {:?}",
                    name_or_id, available
                ));
            }
        }
    } else {
        agent_id.to_string()
    };
    let tier_filter = params.get("tier").and_then(|v| v.as_str());
    let memories = kernel.recall(&effective_agent, "default");
    let filtered: Vec<_> = match tier_filter {
        Some(t) => {
            let tier = match t.to_lowercase().replace(['-', '_'], "").as_str() {
                "ephemeral" | "l0" | "ephem" => crate::memory::MemoryTier::Ephemeral,
                "working" | "l1" | "wk" => crate::memory::MemoryTier::Working,
                "longterm" | "l2" | "lt" | "long" => crate::memory::MemoryTier::LongTerm,
                "procedural" | "l3" | "proc" => crate::memory::MemoryTier::Procedural,
                _ => return ToolResult::error(format!("Unknown tier: {}", t)),
            };
            memories.into_iter().filter(|m| m.tier == tier).collect()
        }
        None => memories,
    };
    let dto: Vec<serde_json::Value> = filtered.into_iter().map(|m| json!({
        "id": m.id,
        "tier": m.tier.name(),
        "content": m.content.display(),
        "importance": m.importance,
        "access_count": m.access_count,
        "tags": m.tags,
    })).collect();
    ToolResult::ok(json!({"memories": dto}))
}

fn handle_store_procedure(kernel: &AIKernel, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let description = params.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let learned_from = params.get("learned_from").and_then(|v| v.as_str()).unwrap_or("manual").to_string();
    let tags: Vec<String> = params.get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let steps: Vec<crate::memory::layered::ProcedureStep> = params.get("steps")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().enumerate().map(|(i, s)| {
            crate::memory::layered::ProcedureStep {
                step_number: (i + 1) as u32,
                description: s.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                action: s.get("action").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                expected_outcome: s.get("expected_outcome").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            }
        }).collect())
        .unwrap_or_default();
    match kernel.remember_procedural(agent_id, "default", crate::kernel::ops::memory::ProceduralEntry {
        name, description, steps, learned_from, tags,
    }) {
        Ok(entry_id) => ToolResult::ok(json!({"entry_id": entry_id, "stored": true})),
        Err(e) => ToolResult::error(e),
    }
}

fn handle_recall_procedure(kernel: &AIKernel, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    let name = params.get("name").and_then(|v| v.as_str());
    let entries = kernel.recall_procedural(agent_id, "default", name);
    let data: Vec<serde_json::Value> = entries.iter().map(|e| {
        match &e.content {
            crate::memory::MemoryContent::Procedure(p) => {
                json!({
                    "id": e.id,
                    "name": p.name,
                    "description": p.description,
                    "steps": p.steps.iter().map(|s| json!({
                        "step_number": s.step_number,
                        "description": s.description,
                        "action": s.action,
                        "expected_outcome": s.expected_outcome,
                    })).collect::<Vec<_>>(),
                    "learned_from": p.learned_from,
                    "tags": e.tags,
                    "importance": e.importance,
                })
            }
            _ => json!({"id": e.id, "content": e.content.display(), "tags": e.tags, "importance": e.importance})
        }
    }).collect();
    ToolResult::ok(json!({"procedures": data, "count": data.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use crate::kernel::tools::memory::handle;

    #[test]
    fn test_memory_store_working() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({"content": "test memory", "tier": "working", "tags": ["test"]});
        let result = handle(&*kernel, "memory.store", &params, "test");
        assert!(result.error.is_none(), "store working should succeed: {:?}", result.error);
    }

    #[test]
    fn test_memory_store_long_term() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({"content": "important fact", "tier": "long-term", "tags": ["fact"], "importance": 80});
        let result = handle(&*kernel, "memory.store", &params, "test");
        assert!(result.error.is_none(), "store long-term should succeed: {:?}", result.error);
        let data = result.output;
        assert_eq!(data["tier"], "long-term");
        assert!(data["id"].as_str().unwrap().len() > 0);
    }

    #[test]
    fn test_memory_store_ephemeral() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({"content": "temp data", "tier": "ephemeral"});
        let result = handle(&*kernel, "memory.store", &params, "test");
        assert!(result.error.is_none(), "store ephemeral should succeed: {:?}", result.error);
    }

    #[test]
    fn test_memory_recall() {
        let (kernel, _tmp) = make_kernel();
        handle(&*kernel, "memory.store", &json!({"content": "recall me", "tier": "working"}), "test");
        let result = handle(&*kernel, "memory.recall", &json!({}), "test");
        assert!(result.error.is_none());
        let memories = result.output["memories"].as_array().unwrap();
        assert!(!memories.is_empty());
    }

    #[test]
    fn test_memory_recall_with_tier_filter() {
        let (kernel, _tmp) = make_kernel();
        handle(&*kernel, "memory.store", &json!({"content": "working mem", "tier": "working"}), "test");
        handle(&*kernel, "memory.store", &json!({"content": "lt mem", "tier": "long-term"}), "test");

        let result = handle(&*kernel, "memory.recall", &json!({"tier": "long-term"}), "test");
        assert!(result.error.is_none());
        let memories = result.output["memories"].as_array().unwrap();
        for m in memories {
            assert_eq!(m["tier"], "long_term");
        }
    }

    #[test]
    fn test_memory_recall_invalid_tier() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "memory.recall", &json!({"tier": "nonexistent"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_memory_forget() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "memory.forget", &json!({}), "test");
        assert!(result.error.is_none());
        assert_eq!(result.output["forgotten"], true);
    }

    #[test]
    fn test_memory_store_procedure() {
        let (kernel, _tmp) = make_kernel();
        let params = json!({
            "name": "test_proc",
            "description": "a test procedure",
            "steps": [
                {"description": "step 1", "action": "do thing", "expected_outcome": "done"},
                {"description": "step 2", "action": "do other", "expected_outcome": "finished"}
            ],
            "tags": ["test"]
        });
        let result = handle(&*kernel, "memory.store_procedure", &params, "test");
        assert!(result.error.is_none(), "store_procedure should succeed: {:?}", result.error);
    }

    #[test]
    fn test_memory_recall_procedure() {
        let (kernel, _tmp) = make_kernel();
        handle(&*kernel, "memory.store_procedure", &json!({
            "name": "my_proc",
            "description": "desc",
            "steps": [{"description": "s1", "action": "a1", "expected_outcome": "o1"}]
        }), "test");

        let result = handle(&*kernel, "memory.recall_procedure", &json!({}), "test");
        assert!(result.error.is_none());
        let procs = result.output["procedures"].as_array().unwrap();
        assert!(!procs.is_empty());
    }

    #[test]
    fn test_memory_unknown_tool() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "memory.nonexistent", &json!({}), "test");
        assert!(result.error.is_some());
    }
}
