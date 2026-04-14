//! Agent definition and lifecycle states.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique agent identifier (UUID v4).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Agent lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentState {
    /// Agent is initialized but not yet scheduled.
    Created,
    /// Agent is queued and waiting for execution.
    Waiting,
    /// Agent is actively running.
    Running,
    /// Agent is paused (context swapped out).
    Suspended,
    /// Agent has completed its current intent.
    Completed,
    /// Agent failed (error occurred).
    Failed,
    /// Agent is terminated (cannot be resumed).
    Terminated,
}

impl AgentState {
    pub fn is_terminal(&self) -> bool {
        matches!(self, AgentState::Completed | AgentState::Failed | AgentState::Terminated)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, AgentState::Running | AgentState::Waiting)
    }
}

/// An AI agent — the fundamental unit of execution in Plico.
#[derive(Debug, Clone)]
pub struct Agent {
    id: AgentId,
    pub name: String,
    state: AgentState,
    /// Current intent being executed.
    current_intent: Option<Intent>,
    /// Resources allocated to this agent.
    resources: AgentResources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResources {
    /// Memory quota (bytes).
    pub memory_quota: u64,
    /// CPU time quota (ms).
    pub cpu_time_quota: u64,
    /// Tools available to this agent.
    pub allowed_tools: Vec<String>,
}

impl Agent {
    pub fn new(name: String) -> Self {
        Self {
            id: AgentId::new(),
            name,
            state: AgentState::Created,
            current_intent: None,
            resources: AgentResources::default(),
        }
    }

    pub fn id(&self) -> &AgentId {
        &self.id
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    pub fn set_state(&mut self, state: AgentState) {
        self.state = state;
    }

    pub fn assign_intent(&mut self, intent: Intent) {
        self.current_intent = Some(intent);
        if self.state == AgentState::Created || self.state == AgentState::Waiting {
            self.state = AgentState::Waiting;
        }
    }
}

impl Default for AgentResources {
    fn default() -> Self {
        Self {
            memory_quota: 1_073_741_824, // 1 GB
            cpu_time_quota: 60_000,       // 60 seconds
            allowed_tools: Vec::new(),
        }
    }
}

/// An intent — a task or goal submitted to the scheduler.
#[derive(Debug, Clone)]
pub struct Intent {
    pub id: IntentId,
    pub priority: IntentPriority,
    pub description: String,
    /// Which agent owns this intent (if any).
    pub agent_id: Option<AgentId>,
    /// Timestamp (ms) when submitted.
    pub submitted_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentPriority {
    Critical = 4,
    High = 3,
    Medium = 2,
    Low = 1,
}

impl PartialOrd for IntentPriority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IntentPriority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

impl Intent {
    pub fn new(priority: IntentPriority, description: String) -> Self {
        Self {
            id: IntentId::new(),
            priority,
            description,
            agent_id: None,
            submitted_at: now_ms(),
        }
    }

    pub fn with_agent(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IntentId(pub String);

impl IntentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for IntentId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for IntentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
