//! Agent lifecycle operations — register, suspend, resume, terminate, checkpoint.

use crate::scheduler::{Agent, AgentHandle, AgentId, AgentState, AgentResources, Intent, IntentPriority, TransitionError};

fn transition_err(e: TransitionError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string())
}

impl crate::kernel::AIKernel {
    pub fn register_agent(&self, name: String) -> String {
        let agent = Agent::new(name);
        let id = agent.id().to_string();
        self.scheduler.register(agent);
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
        if let Some(aid) = agent_id {
            intent = intent.with_agent(AgentId(aid));
        }
        let id = intent.id.0.clone();
        self.scheduler.submit(intent);
        self.persist_intents();
        Ok(id)
    }

    pub fn agent_status(&self, agent_id: &str) -> Option<(String, String, usize)> {
        let agent = self.scheduler.get(&AgentId(agent_id.to_string()))?;
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .count();
        Some((agent.id().to_string(), format!("{:?}", agent.state()), pending))
    }

    pub fn agent_suspend(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;

        // Auto-checkpoint to CAS before suspend (best-effort)
        let checkpoint_cid = self.checkpoint_agent(agent_id).ok();

        let state_before = format!("{:?}", agent.state());
        let memories = self.memory.get_all(agent_id);
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .count();
        let last_intent = self.scheduler.snapshot_intents()
            .iter().rfind(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .map(|i| i.description.clone());

        let snapshot = crate::memory::context_snapshot::ContextSnapshot {
            agent_id: agent_id.to_string(),
            timestamp_ms: crate::memory::layered::now_ms(),
            state_before_suspend: state_before,
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
        self.persist_agents();
        Ok(())
    }

    pub fn agent_resume(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let _agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;

        let memories = self.memory.get_all(agent_id);
        if let Some(snapshot) = crate::memory::context_snapshot::find_latest_snapshot(&memories) {
            let ctx_text = snapshot.to_context_string();
            let entry = crate::memory::MemoryEntry::ephemeral(agent_id, ctx_text);
            self.memory.store(entry);
        }

        self.scheduler.update_state(&aid, AgentState::Waiting).map_err(transition_err)?;
        self.persist_agents();
        Ok(())
    }

    pub fn agent_terminate(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let _agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        self.scheduler.update_state(&aid, AgentState::Terminated).map_err(transition_err)?;
        self.persist_agents();
        Ok(())
    }

    pub fn agent_complete(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let _agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        self.scheduler.update_state(&aid, AgentState::Completed).map_err(transition_err)?;
        self.persist_agents();
        Ok(())
    }

    pub fn agent_fail(&self, agent_id: &str, reason: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let _agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        tracing::info!("Agent {} explicitly failed: {}", agent_id, reason);
        self.scheduler.update_state(&aid, AgentState::Failed).map_err(transition_err)?;
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

        let obj = self.get_object(checkpoint_cid, agent_id)
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
}
