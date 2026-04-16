//! Agent Scheduler
//!
//! Manages the lifecycle of AI agents: creation, scheduling, resource allocation,
//! suspension, resumption, and destruction.
//!
//! # Core Concepts
//!
//! - **Agent**: An independent AI entity with its own memory, tools, and objectives.
//! - **Intent**: A task or goal submitted by or assigned to an agent.
//! - **Priority**: Intent urgency — determines scheduling order.
//! - **Resource allocation**: CPU time, memory quota, tool access permissions.
//!
//! # Scheduling Model
//!
//! Plico uses a priority-based queue with round-robin for equal priority.
//! Two intent categories:
//! - **Inference-intent** (latency-sensitive): High priority, preemptive.
//! - **Training-intent** (throughput-sensitive): Lower priority, batched.

pub mod agent;
pub mod queue;
pub mod dispatch;
pub mod messaging;

pub use agent::{Agent, AgentId, AgentState, AgentResources, Intent, IntentPriority, IntentId};
pub use queue::{SchedulerQueue, SchedulerError};
pub use dispatch::{DispatchHandle, AgentExecutor, LocalExecutor, KernelExecutor, TokioDispatchLoop, DispatchError, ExecutionResult};

use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// The scheduler — global agent lifecycle manager.
pub struct AgentScheduler {
    /// Active agents.
    agents: RwLock<HashMap<AgentId, Agent>>,

    /// Pending intents queue (priority-sorted).
    queue: RwLock<SchedulerQueue>,
}

/// Lightweight agent reference for cross-module communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub id: String,
    pub name: String,
    pub state: AgentState,
}

impl AgentHandle {
    pub fn from_agent(agent: &Agent) -> Self {
        Self {
            id: agent.id().to_string(),
            name: agent.name.clone(),
            state: agent.state(),
        }
    }
}

use std::collections::HashMap;

impl AgentScheduler {
    pub fn new() -> Self {
        Self {
            agents: RwLock::new(HashMap::new()),
            queue: RwLock::new(SchedulerQueue::new()),
        }
    }

    /// Register a new agent with the scheduler.
    pub fn register(&self, agent: Agent) -> AgentId {
        let id = agent.id().clone();
        let mut agents = self.agents.write().unwrap();
        agents.insert(id.clone(), agent);
        id
    }

    /// Submit an intent to the scheduler queue.
    pub fn submit(&self, intent: Intent) {
        self.queue.write().unwrap().push(intent);
    }

    /// Get the next ready intent (highest priority, oldest timestamp).
    pub fn dequeue(&self) -> Option<Intent> {
        self.queue.write().unwrap().pop()
    }

    /// Update agent state.
    pub fn update_state(&self, agent_id: &AgentId, state: AgentState) {
        if let Ok(mut agents) = self.agents.write() {
            if let Some(agent) = agents.get_mut(agent_id) {
                agent.set_state(state);
            }
        }
    }

    /// List all active agents.
    pub fn list_agents(&self) -> Vec<AgentHandle> {
        let agents = self.agents.read().unwrap();
        agents.values().map(AgentHandle::from_agent).collect()
    }

    /// Get agent by ID.
    pub fn get(&self, agent_id: &AgentId) -> Option<Agent> {
        self.agents.read().unwrap().get(agent_id).cloned()
    }

    /// Remove an agent.
    pub fn remove(&self, agent_id: &AgentId) {
        self.agents.write().unwrap().remove(agent_id);
    }

    /// Snapshot all agents for persistence (serializable copies).
    pub fn snapshot_agents(&self) -> Vec<Agent> {
        self.agents.read().unwrap().values().cloned().collect()
    }

    /// Restore agents from a persisted snapshot.
    /// Replaces any existing agents with the same ID.
    pub fn restore_agents(&self, agents: Vec<Agent>) {
        let mut map = self.agents.write().unwrap();
        for agent in agents {
            map.insert(agent.id().clone(), agent);
        }
    }

    /// Re-register an already-constructed Agent (e.g. deserialized from CAS).
    pub fn register_existing(&self, agent: Agent) {
        let mut agents = self.agents.write().unwrap();
        agents.insert(agent.id().clone(), agent);
    }

    /// Get an agent's resource limits.
    pub fn get_resources(&self, agent_id: &AgentId) -> Option<AgentResources> {
        self.agents.read().unwrap().get(agent_id).map(|a| a.resources().clone())
    }

    /// Update an agent's resource limits.
    pub fn set_resources(&self, agent_id: &AgentId, resources: AgentResources) -> bool {
        if let Ok(mut agents) = self.agents.write() {
            if let Some(agent) = agents.get_mut(agent_id) {
                agent.set_resources(resources);
                return true;
            }
        }
        false
    }

    /// Drain all pending intents for persistence snapshot.
    /// Returns a copy of all intents currently in the queue.
    pub fn snapshot_intents(&self) -> Vec<Intent> {
        let mut queue = self.queue.write().unwrap();
        let mut intents = Vec::new();
        while let Some(intent) = queue.pop() {
            intents.push(intent);
        }
        for intent in &intents {
            queue.push(intent.clone());
        }
        intents
    }

    /// Restore intents from a persisted snapshot.
    pub fn restore_intents(&self, intents: Vec<Intent>) {
        let mut queue = self.queue.write().unwrap();
        for intent in intents {
            queue.push(intent);
        }
    }
}

impl Default for AgentScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_list() {
        let scheduler = AgentScheduler::new();
        let agent = Agent::new("TestAgent".into());
        let id = agent.id().to_string();

        scheduler.register(agent);
        let agents = scheduler.list_agents();

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, id);
    }

    #[test]
    fn test_priority_ordering() {
        let scheduler = AgentScheduler::new();

        scheduler.submit(Intent::new(IntentPriority::Low, "low".into()));
        scheduler.submit(Intent::new(IntentPriority::High, "high".into()));
        scheduler.submit(Intent::new(IntentPriority::Medium, "medium".into()));
        scheduler.submit(Intent::new(IntentPriority::Critical, "critical".into()));

        let mut intents = Vec::new();
        while let Some(i) = scheduler.dequeue() {
            intents.push(i);
        }

        assert_eq!(intents[0].description, "critical");
        assert_eq!(intents[1].description, "high");
        assert_eq!(intents[2].description, "medium");
        assert_eq!(intents[3].description, "low");
    }
}
