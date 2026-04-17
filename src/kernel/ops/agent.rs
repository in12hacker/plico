//! Agent lifecycle operations — register, suspend, resume, terminate.

use crate::scheduler::{Agent, AgentHandle, AgentId, AgentState, AgentResources, Intent, IntentPriority};

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
    ) -> String {
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
        id
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
        if agent.state().is_terminal() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Agent {} is in terminal state {:?}", agent_id, agent.state()),
            ));
        }

        let state_before = format!("{:?}", agent.state());
        let memories = self.memory.get_all(agent_id);
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .count();
        let last_intent = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .last()
            .map(|i| i.description.clone());

        let snapshot = crate::memory::context_snapshot::ContextSnapshot {
            agent_id: agent_id.to_string(),
            timestamp_ms: crate::memory::layered::now_ms(),
            state_before_suspend: state_before,
            pending_intents: pending,
            active_memory_count: memories.len(),
            last_intent_description: last_intent,
        };

        self.memory.store(snapshot.to_memory_entry());

        self.scheduler.update_state(&aid, AgentState::Suspended);
        self.persist_agents();
        Ok(())
    }

    pub fn agent_resume(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        if !matches!(agent.state(), AgentState::Suspended) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Agent {} is not suspended (state={:?})", agent_id, agent.state()),
            ));
        }
        self.scheduler.update_state(&aid, AgentState::Running);
        self.persist_agents();
        Ok(())
    }

    pub fn agent_terminate(&self, agent_id: &str) -> std::io::Result<()> {
        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        if agent.state().is_terminal() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("Agent {} already terminated", agent_id),
            ));
        }
        self.scheduler.update_state(&aid, AgentState::Terminated);
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
}
