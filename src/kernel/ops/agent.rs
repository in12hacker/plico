//! Agent lifecycle operations — register, suspend, resume, terminate, checkpoint.

use crate::scheduler::{Agent, AgentHandle, AgentId, AgentState, AgentResources, Intent, IntentPriority, TransitionError};
use crate::kernel::event_bus::KernelEvent;
use crate::api::semantic::{AgentUsageDto, AgentCardDto, SkillDto};

fn transition_err(e: TransitionError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
}

impl crate::kernel::AIKernel {
    /// Ensure an agent is registered, creating a minimal registration if needed.
    /// This enables lazy agent registration on first API call.
    /// Checks both UUID and name-based resolution before creating a new agent.
    pub(crate) fn ensure_agent_registered(&self, agent_id: &str) {
        if self.scheduler.resolve(agent_id).is_some() {
            return;
        }
        let _ = self.register_agent_internal(agent_id);
    }

    /// Internal agent registration (used for lazy registration).
    fn register_agent_internal(&self, name: &str) -> String {
        let agent = Agent::new(name.to_string());
        let id = agent.id().to_string();
        self.scheduler.register(agent);
        id
    }

    pub fn register_agent(&self, name: String) -> String {
        let agent = Agent::new(name.clone());
        let id = agent.id().to_string();
        self.scheduler.register(agent);

        // F-5: Create KG Entity anchor for this agent (enables skill linking)
        use crate::fs::KGNodeType;
        let props = serde_json::json!({ "kind": "agent", "name": name });
        let _ = self.kg_add_node(&id, KGNodeType::Entity, props, &id, "default");

        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: id.clone(),
            old_state: "None".into(),
            new_state: "Waiting".into(),
        });
        self.persist_agents();
        id
    }

    pub fn list_agents(&self) -> Vec<AgentHandle> {
        self.scheduler.list_agents()
    }

    pub fn pending_intent_count(&self) -> usize {
        self.scheduler.snapshot_intents().len()
    }

    pub fn submit_intent(
        &self,
        priority: IntentPriority,
        description: String,
        action: Option<String>,
        agent_id: Option<String>,
    ) -> Result<String, String> {
        if let Some(ref aid_str) = agent_id {
            let aid = AgentId(aid_str.clone());
            if let Some(agent) = self.scheduler.get(&aid) {
                if agent.state().is_terminal() {
                    return Err(format!(
                        "Agent {} is in terminal state {:?} — cannot accept intents",
                        aid_str, agent.state()
                    ));
                }
                if agent.state() == AgentState::Created {
                    let _ = self.scheduler.update_state(&aid, AgentState::Waiting);
                }
            }
        }
        let mut intent = Intent::new(priority, description);
        if let Some(a) = action {
            intent = intent.with_action(a);
        }
        let emit_agent_id = agent_id.clone();
        if let Some(aid) = agent_id {
            intent = intent.with_agent(AgentId(aid));
        }
        let id = intent.id.0.clone();
        let priority_str = format!("{:?}", priority);
        self.scheduler.submit(intent);
        self.event_bus.emit(KernelEvent::IntentSubmitted {
            intent_id: id.clone(),
            agent_id: emit_agent_id,
            priority: priority_str,
        });
        self.persist_intents();
        Ok(id)
    }

    pub fn agent_status(&self, name_or_id: &str) -> Option<(String, String, usize)> {
        let aid = self.scheduler.resolve(name_or_id)?;
        let agent = self.scheduler.get(&aid)?;
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(&aid.0))
            .count();
        Some((agent.id().to_string(), format!("{:?}", agent.state()), pending))
    }

    /// Resolve name or UUID to AgentId string.
    /// B21 fix: enables name-based lookup for quota/status commands.
    pub fn resolve_agent(&self, name_or_id: &str) -> Option<String> {
        self.scheduler.resolve(name_or_id).map(|a| a.0)
    }

    /// Track a CLI command as a tool call for the given agent.
    pub fn track_cli_usage(&self, agent_name_or_id: &str) {
        self.ensure_agent_registered(agent_name_or_id);
        let resolved = self.scheduler.resolve(agent_name_or_id)
            .unwrap_or_else(|| AgentId(agent_name_or_id.to_string()));
        self.scheduler.record_tool_call(&resolved);
    }

    /// Track token consumption for a CLI command response.
    pub fn track_cli_token_usage(&self, agent_name_or_id: &str, tokens: u64) {
        let resolved = self.scheduler.resolve(agent_name_or_id)
            .unwrap_or_else(|| AgentId(agent_name_or_id.to_string()));
        self.scheduler.record_token_usage(&resolved, tokens);
        self.persist_usage();
    }

    pub fn agent_suspend(&self, name_or_id: &str) -> std::io::Result<()> {
        let aid = self.scheduler.resolve(name_or_id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", name_or_id))
        })?;

        // Auto-checkpoint to CAS before suspend (best-effort)
        let checkpoint_cid = self.checkpoint_agent(&aid.0).ok();

        let state_before = format!("{:?}", self.scheduler.get(&aid).map(|a| a.state()).unwrap_or(AgentState::Created));
        let memories = self.memory.get_all(&aid.0);
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(&aid.0))
            .count();
        let last_intent = self.scheduler.snapshot_intents()
            .iter().rfind(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(&aid.0))
            .map(|i| i.description.clone());

        let snapshot = crate::memory::context_snapshot::ContextSnapshot {
            agent_id: aid.0.clone(),
            timestamp_ms: crate::memory::layered::now_ms(),
            state_before_suspend: state_before.clone(),
            pending_intents: pending,
            active_memory_count: memories.len(),
            last_intent_description: last_intent,
        };

        let mut entry = snapshot.to_memory_entry();
        if let Some(cid) = checkpoint_cid {
            entry.tags.push(format!("checkpoint:{}", cid));
        }
        self.memory.store(entry);

        self.scheduler.update_state(&aid, AgentState::Suspended).map_err(transition_err)?;
        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: aid.0.clone(),
            old_state: state_before.clone(),
            new_state: "Suspended".into(),
        });
        self.persist_agents();
        Ok(())
    }

    pub fn agent_resume(&self, name_or_id: &str) -> std::io::Result<()> {
        let aid = self.scheduler.resolve(name_or_id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", name_or_id))
        })?;

        let memories = self.memory.get_all(&aid.0);

        // Extract context summary BEFORE any restore (snapshot may be cleared by restore)
        let ctx_summary = crate::memory::context_snapshot::find_latest_snapshot(&memories)
            .map(|s| s.to_context_string());

        // Try to auto-restore from the latest checkpoint CID
        let checkpoint_cid = memories.iter()
            .filter(|e| e.tags.contains(&crate::memory::context_snapshot::SNAPSHOT_TAG.to_string()))
            .max_by_key(|e| e.created_at)
            .and_then(|e| {
                e.tags.iter()
                    .find(|t| t.starts_with("checkpoint:"))
                    .map(|t| t[11..].to_string())
            });

        if let Some(ref cid) = checkpoint_cid {
            if let Ok(count) = self.restore_agent_checkpoint(&aid.0, cid) {
                tracing::info!(
                    "Agent {} auto-restored from checkpoint {} ({} entries)",
                    aid.0, cid, count
                );
            }
        }

        // Inject context summary for cognitive continuity
        if let Some(ctx_text) = ctx_summary {
            let entry = crate::memory::MemoryEntry::ephemeral(&aid.0, ctx_text);
            self.memory.store(entry);
        }

        self.scheduler.update_state(&aid, AgentState::Waiting).map_err(transition_err)?;
        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: aid.0.clone(),
            old_state: "Suspended".into(),
            new_state: "Waiting".into(),
        });
        self.persist_agents();
        Ok(())
    }

    pub fn agent_terminate(&self, name_or_id: &str) -> std::io::Result<()> {
        let aid = self.scheduler.resolve(name_or_id).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", name_or_id))
        })?;
        let old_state = format!("{:?}", self.scheduler.get(&aid).map(|a| a.state()).unwrap_or(AgentState::Created));
        self.scheduler.update_state(&aid, AgentState::Terminated).map_err(transition_err)?;
        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: aid.0.clone(),
            old_state,
            new_state: "Terminated".into(),
        });
        self.persist_agents();
        Ok(())
    }

    pub fn agent_complete(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        let old_state = format!("{:?}", agent.state());
        self.scheduler.update_state(&aid, AgentState::Completed).map_err(transition_err)?;
        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: agent_id.to_string(),
            old_state,
            new_state: "Completed".into(),
        });
        self.persist_agents();
        Ok(())
    }

    pub fn agent_fail(&self, agent_id: &str, reason: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        let old_state = format!("{:?}", agent.state());
        tracing::info!("Agent {} explicitly failed: {}", agent_id, reason);
        self.scheduler.update_state(&aid, AgentState::Failed).map_err(transition_err)?;
        self.event_bus.emit(KernelEvent::AgentStateChanged {
            agent_id: agent_id.to_string(),
            old_state,
            new_state: "Failed".into(),
        });
        self.persist_agents();
        Ok(())
    }

    pub fn agent_set_resources(
        &self,
        agent_id: &str,
        memory_quota: Option<u64>,
        cpu_time_quota: Option<u64>,
        allowed_tools: Option<Vec<String>>,
    ) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let current = self.scheduler.get_resources(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;

        let resources = AgentResources {
            memory_quota: memory_quota.unwrap_or(current.memory_quota),
            cpu_time_quota: cpu_time_quota.unwrap_or(current.cpu_time_quota),
            allowed_tools: allowed_tools.unwrap_or(current.allowed_tools),
        };

        if self.scheduler.set_resources(&aid, resources) {
            self.persist_agents();
            Ok(())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Agent not found"))
        }
    }

    /// Checkpoint an agent's full memory state to CAS.
    ///
    /// Serializes all memory entries (across all tiers) as a JSON array,
    /// stores the result as a CAS object, and returns the content-addressed ID.
    /// The checkpoint is immutable and deduplicated by content hash.
    pub fn checkpoint_agent(&self, agent_id: &str) -> Result<String, String> {
        let aid = AgentId(agent_id.to_string());
        self.scheduler.get(&aid).ok_or_else(|| format!("Agent not found: {}", agent_id))?;

        let entries = self.memory.get_all(agent_id);
        let payload = serde_json::to_vec(&entries)
            .map_err(|e| format!("Failed to serialize checkpoint: {}", e))?;

        let cid = self.semantic_create(
            payload,
            vec![
                "checkpoint".into(),
                format!("agent:{}", agent_id),
            ],
            agent_id,
            None,
        ).map_err(|e| format!("Failed to store checkpoint: {}", e))?;

        tracing::info!(
            "Checkpoint created for agent {}: CID={} ({} entries)",
            agent_id, cid, entries.len()
        );

        Ok(cid)
    }

    /// Restore an agent's memory state from a CAS checkpoint.
    ///
    /// Fetches the checkpoint by CID, deserializes memory entries,
    /// clears the agent's current memory, and replaces it with the
    /// checkpoint data. Returns the number of entries restored.
    pub fn restore_agent_checkpoint(&self, agent_id: &str, checkpoint_cid: &str) -> Result<usize, String> {
        let aid = AgentId(agent_id.to_string());
        self.scheduler.get(&aid).ok_or_else(|| format!("Agent not found: {}", agent_id))?;

        let obj = self.get_object(checkpoint_cid, agent_id, "default")
            .map_err(|e| format!("Failed to fetch checkpoint: {}", e))?;

        let entries: Vec<crate::memory::MemoryEntry> = serde_json::from_slice(&obj.data)
            .map_err(|e| format!("Failed to deserialize checkpoint: {}", e))?;

        let count = entries.len();
        self.memory.clear_agent(agent_id);

        for entry in entries {
            self.memory.store(entry);
        }
        self.persist_memories();

        tracing::info!(
            "Checkpoint restored for agent {}: CID={} ({} entries)",
            agent_id, checkpoint_cid, count
        );

        Ok(count)
    }

    pub fn agent_usage(&self, agent_id: &str) -> Option<AgentUsageDto> {
        let aid = AgentId(agent_id.to_string());
        let resources = self.scheduler.get_resources(&aid)?;
        let usage = self.scheduler.get_usage(&aid);
        let memory_entries = self.memory.count_for_agent(agent_id);
        Some(AgentUsageDto {
            agent_id: agent_id.to_string(),
            memory_entries,
            memory_quota: resources.memory_quota,
            tool_call_count: usage.tool_call_count,
            total_tokens_consumed: usage.total_tokens_consumed,
            cpu_time_quota: resources.cpu_time_quota,
            allowed_tools: resources.allowed_tools,
            last_active_ms: usage.last_active_ms,
        })
    }

    pub fn discover_agents(
        &self,
        state_filter: Option<&str>,
        tool_filter: Option<&str>,
    ) -> Vec<AgentCardDto> {
        let handles = self.scheduler.list_agents();
        let all_tool_names: Vec<String> = self.tool_registry.list()
            .iter().map(|t| t.name.clone()).collect();

        handles.into_iter()
            .filter(|h| {
                if let Some(sf) = state_filter {
                    let state_str = format!("{:?}", h.state);
                    if !state_str.eq_ignore_ascii_case(sf) {
                        return false;
                    }
                }
                true
            })
            .filter_map(|h| {
                let aid = AgentId(h.id.clone());
                let resources = self.scheduler.get_resources(&aid).unwrap_or_default();
                let usage = self.scheduler.get_usage(&aid);
                let memory_entries = self.memory.count_for_agent(&h.id);

                let tools = if resources.allowed_tools.is_empty() {
                    all_tool_names.clone()
                } else {
                    resources.allowed_tools.clone()
                };

                if let Some(tf) = tool_filter {
                    if !tools.iter().any(|t| t.contains(tf)) {
                        return None;
                    }
                }

                Some(AgentCardDto {
                    agent_id: h.id.clone(),
                    name: h.name.clone(),
                    description: h.description.clone(),
                    version: String::new(), // Not yet configurable
                    state: format!("{:?}", h.state),
                    memory_quota: resources.memory_quota,
                    cpu_time_quota: resources.cpu_time_quota,
                    tools,
                    memory_entries,
                    tool_call_count: usage.tool_call_count,
                    last_active_ms: usage.last_active_ms,
                    created_at_ms: h.created_at_ms,
                })
            })
            .collect()
    }

    pub fn register_skill(
        &self,
        agent_id: &str,
        name: &str,
        description: &str,
        tags: Vec<String>,
    ) -> Result<String, String> {
        use crate::fs::{KGNodeType, KGEdgeType};

        let aid = AgentId(agent_id.to_string());
        self.scheduler.get(&aid)
            .ok_or_else(|| format!("Agent not found: {}", agent_id))?;

        let mut props = serde_json::json!({
            "kind": "skill",
            "description": description,
        });
        if !tags.is_empty() {
            props["tags"] = serde_json::json!(tags);
        }

        let node_id = self.kg_add_node(name, KGNodeType::Fact, props, agent_id, "default")
            .map_err(|e| e.to_string())?;

        // F-5: Link to Entity anchor (now guaranteed to exist via register_agent)
        let agent_nodes = self.kg_list_nodes(Some(KGNodeType::Entity), agent_id, "default")
            .unwrap_or_default();
        let agent_entity = agent_nodes.iter().find(|n| n.label == agent_id);
        if let Some(entity) = agent_entity {
            let _ = self.kg_add_edge(&entity.id, &node_id, KGEdgeType::HasFact, None, agent_id, "default");
        }

        // F-5: Dual-write to Memory (Procedural tier) for skills list visibility
        let steps = vec![crate::memory::layered::ProcedureStep {
            step_number: 0,
            description: description.to_string(),
            action: format!("invoke skill: {}", name),
            expected_outcome: String::new(),
        }];
        let _ = self.remember_procedural(
            agent_id, "default",
            super::memory::ProceduralEntry {
                name: name.to_string(),
                description: description.to_string(),
                steps,
                learned_from: "skill_register".to_string(),
                tags: tags.clone(),
            },
        );

        Ok(node_id)
    }

    pub fn discover_skills(
        &self,
        query: Option<&str>,
        agent_id_filter: Option<&str>,
        tag_filter: Option<&str>,
    ) -> Vec<SkillDto> {
        use crate::fs::KGNodeType;

        let agent_ids: Vec<String> = if let Some(aid) = agent_id_filter {
            vec![aid.to_string()]
        } else {
            self.scheduler.list_agents().into_iter().map(|h| h.id).collect()
        };
        let all_facts: Vec<_> = agent_ids.iter()
            .flat_map(|aid| self.kg_list_nodes(Some(KGNodeType::Fact), aid, "default").unwrap_or_default())
            .collect();
        all_facts.into_iter()
            .filter(|n| {
                n.properties.get("kind").and_then(|v| v.as_str()) == Some("skill")
            })
            .filter(|n| {
                if let Some(q) = query {
                    let q_lower = q.to_lowercase();
                    n.label.to_lowercase().contains(&q_lower)
                        || n.properties.get("description")
                            .and_then(|v| v.as_str())
                            .map(|d| d.to_lowercase().contains(&q_lower))
                            .unwrap_or(false)
                } else {
                    true
                }
            })
            .filter(|n| {
                if let Some(tf) = tag_filter {
                    n.properties.get("tags")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().any(|t| t.as_str() == Some(tf)))
                        .unwrap_or(false)
                } else {
                    true
                }
            })
            .map(|n| {
                let tags = n.properties.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                let description = n.properties.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                SkillDto {
                    node_id: n.id.clone(),
                    name: n.label.clone(),
                    description,
                    agent_id: n.agent_id.clone(),
                    tags,
                }
            })
            .collect()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_agent_multiple() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id1 = kernel.register_agent("agent-alpha".to_string());
        let id2 = kernel.register_agent("agent-beta".to_string());
        assert_ne!(id1, id2);

        let agents = kernel.list_agents();
        let names: Vec<_> = agents.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"agent-alpha".to_string()));
        assert!(names.contains(&"agent-beta".to_string()));
    }

    #[test]
    fn test_resolve_agent_by_uuid() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.register_agent("resolver-test".to_string());

        let resolved = kernel.resolve_agent(&id);
        assert_eq!(resolved, Some(id));
    }

    #[test]
    fn test_resolve_agent_by_name() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("named-agent".to_string());

        let resolved = kernel.resolve_agent("named-agent");
        assert!(resolved.is_some());
    }

    #[test]
    fn test_resolve_agent_nonexistent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        assert!(kernel.resolve_agent("does-not-exist").is_none());
    }

    #[test]
    fn test_submit_intent_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("intent-agent".to_string());

        let result = kernel.submit_intent(
            IntentPriority::Medium,
            "test intent".to_string(),
            None,
            Some("intent-agent".to_string()),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_agent_status() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.register_agent("status-agent".to_string());

        let status = kernel.agent_status(&id);
        assert!(status.is_some());
        let (agent_id, _state, pending) = status.unwrap();
        assert_eq!(agent_id, id);
        assert_eq!(pending, 0);
    }

    #[test]
    fn test_discover_agents() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("alice".to_string());
        kernel.register_agent("bob".to_string());

        let agents = kernel.discover_agents(None, None);
        assert!(agents.len() >= 2);
    }

    #[test]
    fn test_ensure_agent_registered() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.ensure_agent_registered("lazy-agent");
        let resolved = kernel.resolve_agent("lazy-agent");
        assert!(resolved.is_some());
    }
}
