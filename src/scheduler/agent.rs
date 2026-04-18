//! Agent Definition and Lifecycle States
//!
//! Defines `Agent`, `AgentId`, `AgentState`, `Intent`, and `IntentPriority` —
//! the core types for agent lifecycle management in the scheduler.

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

    pub fn can_transition(&self, to: AgentState) -> bool {
        use AgentState::*;
        matches!(
            (self, to),
            (Created, Waiting) | (Created, Running) | (Created, Terminated)
            | (Waiting, Running) | (Waiting, Suspended) | (Waiting, Completed)
            | (Waiting, Failed) | (Waiting, Terminated)
            | (Running, Waiting) | (Running, Suspended) | (Running, Completed)
            | (Running, Failed) | (Running, Terminated)
            | (Suspended, Waiting) | (Suspended, Terminated)
        )
    }
}

#[derive(Debug, Clone)]
pub struct TransitionError {
    pub from: AgentState,
    pub to: AgentState,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "illegal state transition: {:?} → {:?}", self.from, self.to)
    }
}

impl std::error::Error for TransitionError {}

/// An AI agent — the fundamental unit of execution in Plico.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    id: AgentId,
    pub name: String,
    state: AgentState,
    /// Current intent being executed (not serialized — transient).
    #[serde(skip)]
    current_intent: Option<Intent>,
    /// Resources allocated to this agent.
    resources: AgentResources,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct AgentResources {
    /// Max number of memory entries this agent can store. 0 = unlimited.
    pub memory_quota: u64,
    /// Max CPU time per intent execution (ms). 0 = unlimited.
    pub cpu_time_quota: u64,
    /// Tools available to this agent. Empty = all tools allowed.
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentUsage {
    pub tool_call_count: u64,
    pub last_active_ms: u64,
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

    pub fn set_state(&mut self, state: AgentState) -> Result<(), TransitionError> {
        if !self.state.can_transition(state) {
            return Err(TransitionError { from: self.state, to: state });
        }
        self.state = state;
        Ok(())
    }

    pub fn assign_intent(&mut self, intent: Intent) {
        self.current_intent = Some(intent);
        if self.state == AgentState::Created || self.state == AgentState::Waiting {
            let _ = self.set_state(AgentState::Waiting);
        }
    }

    pub fn resources(&self) -> &AgentResources {
        &self.resources
    }

    pub fn set_resources(&mut self, resources: AgentResources) {
        self.resources = resources;
    }
}


/// An intent — a task or goal submitted to the scheduler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    pub id: IntentId,
    pub priority: IntentPriority,
    pub description: String,
    /// JSON-encoded action payload (typically a serialized `ApiRequest`).
    /// When present, the executor deserializes and dispatches this action
    /// through the kernel. When None, the intent is descriptive only.
    pub action: Option<String>,
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
            action: None,
            agent_id: None,
            submitted_at: now_ms(),
        }
    }

    pub fn with_action(mut self, action: String) -> Self {
        self.action = Some(action);
        self
    }

    pub fn with_agent(mut self, agent_id: AgentId) -> Self {
        self.agent_id = Some(agent_id);
        self
    }

    pub fn description(&self) -> &str {
        &self.description
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

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_transition_legal() {
        use AgentState::*;
        let legal = [
            (Created, Waiting), (Created, Running), (Created, Terminated),
            (Waiting, Running), (Waiting, Suspended), (Waiting, Completed),
            (Waiting, Failed), (Waiting, Terminated),
            (Running, Waiting), (Running, Suspended), (Running, Completed),
            (Running, Failed), (Running, Terminated),
            (Suspended, Waiting), (Suspended, Terminated),
        ];
        for (from, to) in legal {
            assert!(from.can_transition(to), "{:?} → {:?} should be legal", from, to);
        }
    }

    #[test]
    fn test_terminal_state_immutable() {
        use AgentState::*;
        let terminals = [Completed, Failed, Terminated];
        let all_states = [Created, Waiting, Running, Suspended, Completed, Failed, Terminated];
        for from in terminals {
            for to in all_states {
                assert!(!from.can_transition(to), "{:?} → {:?} should be illegal", from, to);
            }
        }
    }

    #[test]
    fn test_set_state_returns_err_on_invalid() {
        let mut agent = Agent::new("test".into());
        assert_eq!(agent.state(), AgentState::Created);
        let result = agent.set_state(AgentState::Completed);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.from, AgentState::Created);
        assert_eq!(err.to, AgentState::Completed);
        assert_eq!(agent.state(), AgentState::Created);
    }

    #[test]
    fn test_set_state_returns_ok_on_valid() {
        let mut agent = Agent::new("test".into());
        assert!(agent.set_state(AgentState::Waiting).is_ok());
        assert_eq!(agent.state(), AgentState::Waiting);
        assert!(agent.set_state(AgentState::Running).is_ok());
        assert_eq!(agent.state(), AgentState::Running);
        assert!(agent.set_state(AgentState::Completed).is_ok());
        assert_eq!(agent.state(), AgentState::Completed);
    }

    #[test]
    fn test_transition_error_display() {
        let err = TransitionError {
            from: AgentState::Completed,
            to: AgentState::Running,
        };
        let msg = err.to_string();
        assert!(msg.contains("Completed"));
        assert!(msg.contains("Running"));
    }
}
