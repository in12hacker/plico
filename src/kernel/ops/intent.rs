//! Intent module (F-1, Node 21) — Structured Intent Declaration and Progress Tracking.
//!
//! Provides:
//! - `IntentDeclaration`: structured intent with keywords, CIDs, budget, expected outcome
//! - `IntentTracker`: tracks active intent execution progress
//!
//! Soul 2.0 Axiom 2: "意图先于操作" — OS accepts structured intent declaration
//! and handles assembly, not Agent.

use std::collections::HashMap;
use std::sync::RwLock;
use serde::{Deserialize, Serialize};

/// Structured intent declaration (F-1).
///
/// Replaces raw string intent with structured data:
/// - keywords: semantic tags for matching
/// - related_cids: context CIDs for assembly
/// - budget_tokens: token budget for this intent
/// - expected_outcome: description of expected result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentDeclaration {
    /// Intent keywords for semantic matching.
    pub keywords: Vec<String>,
    /// Related CIDs for context assembly.
    pub related_cids: Vec<String>,
    /// Token budget for this intent.
    pub budget_tokens: usize,
    /// Expected outcome description.
    pub expected_outcome: String,
    /// Agent that declared this intent.
    pub agent_id: String,
    /// Session this intent belongs to.
    pub session_id: String,
    /// Intent text for backward compatibility.
    pub intent_text: String,
    /// When this intent was declared.
    pub declared_at_ms: u64,
}

impl IntentDeclaration {
    pub fn new(
        keywords: Vec<String>,
        related_cids: Vec<String>,
        budget_tokens: usize,
        expected_outcome: String,
        agent_id: String,
        session_id: String,
        intent_text: String,
    ) -> Self {
        Self {
            keywords,
            related_cids,
            budget_tokens,
            expected_outcome,
            agent_id,
            session_id,
            intent_text,
            declared_at_ms: now_ms(),
        }
    }

    /// Validate that required fields are present.
    pub fn validate(&self) -> Result<(), IntentValidationError> {
        if self.keywords.is_empty() && self.intent_text.trim().is_empty() {
            return Err(IntentValidationError::EmptyIntent);
        }
        if self.budget_tokens == 0 {
            return Err(IntentValidationError::ZeroBudget);
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum IntentValidationError {
    EmptyIntent,
    ZeroBudget,
}

impl std::fmt::Display for IntentValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentValidationError::EmptyIntent => write!(f, "intent must have keywords or text"),
            IntentValidationError::ZeroBudget => write!(f, "budget_tokens must be > 0"),
        }
    }
}

// ── F-2: Intent Plan ─────────────────────────────────────────────────────────

/// Intent operation types for plan steps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentOperation {
    /// Read a CID.
    Read { cid: String },
    /// Search with query and tags.
    Search { query: String, tags: Vec<String> },
    /// Call a tool with parameters.
    Call { tool: String, params: serde_json::Value },
    /// Create an object.
    Create { content: Vec<u8>, tags: Vec<String> },
    /// Read multiple CIDs.
    ReadBatch { cids: Vec<String> },
}

/// State of an individual step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntentStepState {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
}

/// An individual step in an intent plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentStep {
    pub step_id: String,
    pub operation: IntentOperation,
    pub dependencies: Vec<String>,
    pub estimated_tokens: usize,
    pub state: IntentStepState,
}

impl IntentStep {
    pub fn new(step_id: String, operation: IntentOperation, estimated_tokens: usize) -> Self {
        Self {
            step_id,
            operation,
            dependencies: Vec::new(),
            estimated_tokens,
            state: IntentStepState::Pending,
        }
    }

    pub fn with_dependency(mut self, dep: String) -> Self {
        self.dependencies.push(dep);
        self
    }
}

/// Intent plan — decomposed intent into executable steps (F-2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentPlan {
    pub intent_id: String,
    pub steps: Vec<IntentStep>,
    pub total_estimated_tokens: usize,
    pub created_at_ms: u64,
}

impl IntentPlan {
    pub fn new(intent_id: String) -> Self {
        Self {
            intent_id,
            steps: Vec::new(),
            total_estimated_tokens: 0,
            created_at_ms: now_ms(),
        }
    }

    pub fn add_step(&mut self, step: IntentStep) {
        self.total_estimated_tokens += step.estimated_tokens;
        self.steps.push(step);
    }

    /// Topological sort of steps by dependencies.
    /// Returns step indices in execution order (dependencies first).
    pub fn topological_sort(&self) -> Result<Vec<usize>, PlanError> {
        let n = self.steps.len();
        let mut visited = vec![false; n];
        let mut in_progress = vec![false; n];
        let mut sorted = Vec::new();

        // Build step_id to index map
        let step_id_to_idx: std::collections::HashMap<_, _> = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| (&s.step_id, i))
            .collect();

        // Validate dependencies exist
        for step in &self.steps {
            for dep in &step.dependencies {
                if !step_id_to_idx.contains_key(dep) {
                    return Err(PlanError::MissingDependency(step.step_id.clone(), dep.clone()));
                }
            }
        }

        fn visit(
            idx: usize,
            steps: &[IntentStep],
            visited: &mut [bool],
            in_progress: &mut [bool],
            sorted: &mut Vec<usize>,
            step_id_to_idx: &std::collections::HashMap<&String, usize>,
        ) -> Result<(), PlanError> {
            if visited[idx] {
                return Ok(());
            }
            if in_progress[idx] {
                return Err(PlanError::CircularDependency(steps[idx].step_id.clone()));
            }

            in_progress[idx] = true;

            for dep_id in &steps[idx].dependencies {
                if let Some(&dep_idx) = step_id_to_idx.get(dep_id) {
                    visit(dep_idx, steps, visited, in_progress, sorted, step_id_to_idx)?;
                }
            }

            in_progress[idx] = false;
            visited[idx] = true;
            sorted.push(idx);
            Ok(())
        }

        for i in 0..n {
            if !visited[i] {
                visit(i, &self.steps, &mut visited, &mut in_progress, &mut sorted, &step_id_to_idx)?;
            }
        }

        Ok(sorted)
    }

    /// Get sorted steps (convenience method).
    pub fn sorted_steps(&self) -> Result<Vec<&IntentStep>, PlanError> {
        let indices = self.topological_sort()?;
        Ok(indices.into_iter().map(|i| &self.steps[i]).collect())
    }

    /// Get steps that have no dependencies (can run immediately).
    pub fn entry_steps(&self) -> Vec<&IntentStep> {
        self.steps.iter().filter(|s| s.dependencies.is_empty()).collect()
    }

    /// Get step by ID.
    pub fn get_step(&self, step_id: &str) -> Option<&IntentStep> {
        self.steps.iter().find(|s| s.step_id == step_id)
    }
}

#[derive(Debug)]
pub enum PlanError {
    MissingDependency(String, String),
    CircularDependency(String),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::MissingDependency(step, dep) => {
                write!(f, "step '{}' depends on missing '{}'", step, dep)
            }
            PlanError::CircularDependency(step) => {
                write!(f, "circular dependency detected at step '{}'", step)
            }
        }
    }
}

// ── F-4: IntentTree for Multi-Agent Coordination ──────────────────────────────

/// IntentTree — shared intent plan for multi-agent coordination (F-4).
///
/// Allows multiple sub-agents to work on different steps of the same intent plan.
/// Each sub-agent can claim steps, execute them, and report results back to the tree.
#[derive(Debug)]
pub struct IntentTree {
    /// Root intent ID.
    pub root_intent_id: String,
    /// Shared plan (immutable after creation).
    pub plan: IntentPlan,
    /// Which agent owns which steps.
    assigned_agents: RwLock<HashMap<String, Vec<String>>>,
    /// Results from each step.
    results: RwLock<HashMap<String, StepResult>>,
    /// Conflict detection log.
    conflicts: RwLock<Vec<ConflictRecord>>,
}

#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub success: bool,
    pub output_cids: Vec<String>,
    pub tokens_used: usize,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ConflictRecord {
    pub step_id: String,
    pub agent_id: String,
    pub conflict_type: ConflictType,
    pub detected_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    /// Two agents claimed the same step.
    DoubleClaim,
    /// Agent attempted to claim a step assigned to another agent.
    AlreadyClaimed,
    /// Result doesn't match expected output.
    ResultMismatch,
}

impl IntentTree {
    /// Create a new IntentTree from a plan.
    pub fn new(root_intent_id: String, plan: IntentPlan) -> Self {
        Self {
            root_intent_id,
            plan,
            assigned_agents: RwLock::new(HashMap::new()),
            results: RwLock::new(HashMap::new()),
            conflicts: RwLock::new(Vec::new()),
        }
    }

    /// Assign a step to an agent.
    /// Returns Ok if assignment succeeded, Err if step is already claimed.
    pub fn assign_step(&self, step_id: &str, agent_id: &str) -> Result<(), TreeError> {
        // Check if step exists
        let step_exists = self.plan.steps.iter().any(|s| s.step_id == step_id);
        if !step_exists {
            return Err(TreeError::StepNotFound(step_id.to_string()));
        }

        // Check if step is already assigned
        {
            let assigned = self.assigned_agents.read().unwrap();
            for (_agent, steps) in assigned.iter() {
                if steps.contains(&step_id.to_string()) {
                    return Err(TreeError::StepAlreadyAssigned(step_id.to_string()));
                }
            }
        }

        // Assign step to agent
        let mut assigned = self.assigned_agents.write().unwrap();
        assigned
            .entry(agent_id.to_string())
            .or_default()
            .push(step_id.to_string());
        Ok(())
    }

    /// Get all steps assigned to an agent.
    pub fn get_agent_steps(&self, agent_id: &str) -> Vec<String> {
        let assigned = self.assigned_agents.read().unwrap();
        assigned.get(agent_id).cloned().unwrap_or_default()
    }

    /// Record a step result from an agent.
    pub fn record_result(&self, step_id: &str, agent_id: &str, result: StepResult) -> Result<(), TreeError> {
        // Verify the agent owns this step
        {
            let assigned = self.assigned_agents.read().unwrap();
            if let Some(steps) = assigned.get(agent_id) {
                if !steps.contains(&step_id.to_string()) {
                    return Err(TreeError::UnauthorizedAgent(step_id.to_string(), agent_id.to_string()));
                }
            } else {
                return Err(TreeError::UnauthorizedAgent(step_id.to_string(), agent_id.to_string()));
            }
        }

        // Record result
        let mut results = self.results.write().unwrap();
        results.insert(step_id.to_string(), result);
        Ok(())
    }

    /// Get result for a step.
    pub fn get_result(&self, step_id: &str) -> Option<StepResult> {
        let results = self.results.read().unwrap();
        results.get(step_id).cloned()
    }

    /// Check for conflicts (double-claims detected at assignment time, but can check here too).
    pub fn detect_conflicts(&self) -> Vec<ConflictRecord> {
        let conflicts = self.conflicts.read().unwrap();
        conflicts.clone()
    }

    /// Record a conflict.
    fn add_conflict(&self, step_id: &str, agent_id: &str, conflict_type: ConflictType) {
        let mut conflicts = self.conflicts.write().unwrap();
        conflicts.push(ConflictRecord {
            step_id: step_id.to_string(),
            agent_id: agent_id.to_string(),
            conflict_type,
            detected_at_ms: now_ms(),
        });
    }

    /// Get unclaimed steps.
    pub fn unclaimed_steps(&self) -> Vec<String> {
        let assigned = self.assigned_agents.read().unwrap();
        let mut claimed: std::collections::HashSet<_> = std::collections::HashSet::new();
        for steps in assigned.values() {
            for step in steps {
                claimed.insert(step);
            }
        }
        self.plan
            .steps
            .iter()
            .filter(|s| !claimed.contains(&s.step_id))
            .map(|s| s.step_id.clone())
            .collect()
    }

    /// Get steps that have all dependencies satisfied (ready to execute).
    pub fn ready_steps(&self) -> Vec<String> {
        let results = self.results.read().unwrap();
        let mut completed: std::collections::HashSet<_> = std::collections::HashSet::new();
        for (step_id, result) in results.iter() {
            if result.success {
                completed.insert(step_id.clone());
            }
        }

        self.plan
            .steps
            .iter()
            .filter(|s| {
                // Not already completed
                if results.contains_key(&s.step_id) {
                    return false;
                }
                // All dependencies are satisfied
                s.dependencies.iter().all(|dep| completed.contains(dep))
            })
            .map(|s| s.step_id.clone())
            .collect()
    }

    /// Check if all steps are completed.
    pub fn is_complete(&self) -> bool {
        let results = self.results.read().unwrap();
        let all_done = self.plan.steps.iter().all(|s| results.contains_key(&s.step_id));
        if !all_done {
            return false;
        }
        // All must be successful
        results.values().all(|r| r.success)
    }

    /// Aggregate results from all steps.
    pub fn aggregate_results(&self) -> AggregatedResults {
        let results = self.results.read().unwrap();
        let mut total_tokens = 0;
        let mut successful_cids = Vec::new();
        let mut failed_steps = Vec::new();

        for (step_id, result) in results.iter() {
            total_tokens += result.tokens_used;
            if result.success {
                successful_cids.extend(result.output_cids.clone());
            } else {
                failed_steps.push(step_id.clone());
            }
        }

        let success = failed_steps.is_empty();
        AggregatedResults {
            total_tokens,
            output_cids: successful_cids,
            failed_steps,
            success,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AggregatedResults {
    pub total_tokens: usize,
    pub output_cids: Vec<String>,
    pub failed_steps: Vec<String>,
    pub success: bool,
}

#[derive(Debug)]
pub enum TreeError {
    StepNotFound(String),
    StepAlreadyAssigned(String),
    UnauthorizedAgent(String, String),
    ResultNotFound(String),
}

impl std::fmt::Display for TreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TreeError::StepNotFound(id) => write!(f, "step not found: {}", id),
            TreeError::StepAlreadyAssigned(id) => write!(f, "step already assigned: {}", id),
            TreeError::UnauthorizedAgent(step, agent) => {
                write!(f, "agent '{}' is not authorized for step '{}'", agent, step)
            }
            TreeError::ResultNotFound(id) => write!(f, "result not found for step: {}", id),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentExecutionState {
    /// Intent declared but not yet planned.
    Declared,
    /// Intent plan created, awaiting execution.
    Planned,
    /// Intent is being executed.
    Executing,
    /// Intent completed successfully.
    Completed,
    /// Intent failed.
    Failed(String),
    /// Intent cancelled.
    Cancelled,
}

/// Intent progress tracking (F-5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentProgress {
    pub intent_id: String,
    pub state: IntentExecutionState,
    pub steps_total: usize,
    pub steps_completed: usize,
    pub tokens_used: usize,
    pub budget_tokens: usize,
    pub started_at_ms: Option<u64>,
    pub completed_at_ms: Option<u64>,
}

impl IntentProgress {
    pub fn completion_ratio(&self) -> f32 {
        if self.steps_total == 0 {
            return 0.0;
        }
        self.steps_completed as f32 / self.steps_total as f32
    }

    pub fn token_ratio(&self) -> f32 {
        if self.budget_tokens == 0 {
            return 0.0;
        }
        self.tokens_used as f32 / self.budget_tokens as f32
    }
}

/// Intent Tracker — tracks active intent execution progress (F-5).
///
/// Thread-safe store of active intents and their execution state.
pub struct IntentTracker {
    /// Active intents keyed by intent_id.
    intents: RwLock<HashMap<String, ActiveIntent>>,
    /// Completed/cancelled intents for history.
    history: RwLock<HashMap<String, IntentProgress>>,
}

struct ActiveIntent {
    declaration: IntentDeclaration,
    state: IntentExecutionState,
    steps_completed: usize,
    steps_total: usize,
    tokens_used: usize,
    started_at_ms: Option<u64>,
}

impl IntentTracker {
    pub fn new() -> Self {
        Self {
            intents: RwLock::new(HashMap::new()),
            history: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new intent declaration.
    pub fn declare(&self, intent_id: String, declaration: IntentDeclaration) -> Result<(), IntentError> {
        declaration.validate()?;
        let mut intents = self.intents.write().unwrap();
        intents.insert(intent_id.clone(), ActiveIntent {
            declaration,
            state: IntentExecutionState::Declared,
            steps_completed: 0,
            steps_total: 0,
            tokens_used: 0,
            started_at_ms: None,
        });
        Ok(())
    }

    /// Update intent state to Planned.
    pub fn plan(&self, intent_id: &str, steps_total: usize) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.get_mut(intent_id) {
            intent.state = IntentExecutionState::Planned;
            intent.steps_total = steps_total;
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Start executing an intent.
    pub fn start(&self, intent_id: &str) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.get_mut(intent_id) {
            intent.state = IntentExecutionState::Executing;
            intent.started_at_ms = Some(now_ms());
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Record step completion.
    pub fn complete_step(&self, intent_id: &str, tokens_used: usize) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.get_mut(intent_id) {
            intent.steps_completed += 1;
            intent.tokens_used += tokens_used;
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Mark intent as completed.
    pub fn complete(&self, intent_id: &str) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.remove(intent_id) {
            let progress = IntentProgress {
                intent_id: intent_id.to_string(),
                state: IntentExecutionState::Completed,
                steps_total: intent.steps_total,
                steps_completed: intent.steps_completed,
                tokens_used: intent.tokens_used,
                budget_tokens: intent.declaration.budget_tokens,
                started_at_ms: intent.started_at_ms,
                completed_at_ms: Some(now_ms()),
            };
            let mut history = self.history.write().unwrap();
            history.insert(intent_id.to_string(), progress);
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Mark intent as failed.
    pub fn fail(&self, intent_id: &str, reason: String) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.remove(intent_id) {
            let progress = IntentProgress {
                intent_id: intent_id.to_string(),
                state: IntentExecutionState::Failed(reason),
                steps_total: intent.steps_total,
                steps_completed: intent.steps_completed,
                tokens_used: intent.tokens_used,
                budget_tokens: intent.declaration.budget_tokens,
                started_at_ms: intent.started_at_ms,
                completed_at_ms: Some(now_ms()),
            };
            let mut history = self.history.write().unwrap();
            history.insert(intent_id.to_string(), progress);
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Cancel an intent.
    pub fn cancel(&self, intent_id: &str) -> Result<(), IntentError> {
        let mut intents = self.intents.write().unwrap();
        if let Some(intent) = intents.remove(intent_id) {
            let progress = IntentProgress {
                intent_id: intent_id.to_string(),
                state: IntentExecutionState::Cancelled,
                steps_total: intent.steps_total,
                steps_completed: intent.steps_completed,
                tokens_used: intent.tokens_used,
                budget_tokens: intent.declaration.budget_tokens,
                started_at_ms: intent.started_at_ms,
                completed_at_ms: Some(now_ms()),
            };
            let mut history = self.history.write().unwrap();
            history.insert(intent_id.to_string(), progress);
            Ok(())
        } else {
            Err(IntentError::IntentNotFound(intent_id.to_string()))
        }
    }

    /// Get progress for an intent.
    pub fn get_progress(&self, intent_id: &str) -> Option<IntentProgress> {
        // Check active intents first
        let intents = self.intents.read().unwrap();
        if let Some(intent) = intents.get(intent_id) {
            return Some(IntentProgress {
                intent_id: intent_id.to_string(),
                state: intent.state.clone(),
                steps_total: intent.steps_total,
                steps_completed: intent.steps_completed,
                tokens_used: intent.tokens_used,
                budget_tokens: intent.declaration.budget_tokens,
                started_at_ms: intent.started_at_ms,
                completed_at_ms: None,
            });
        }
        drop(intents);

        // Check history
        let history = self.history.read().unwrap();
        history.get(intent_id).cloned()
    }

    /// List all active intents for an agent.
    pub fn list_active(&self, agent_id: &str) -> Vec<IntentProgress> {
        let intents = self.intents.read().unwrap();
        intents.values()
            .filter(|i| i.declaration.agent_id == agent_id)
            .map(|i| IntentProgress {
                intent_id: i.declaration.keywords.first().cloned().unwrap_or_default(),
                state: i.state.clone(),
                steps_total: i.steps_total,
                steps_completed: i.steps_completed,
                tokens_used: i.tokens_used,
                budget_tokens: i.declaration.budget_tokens,
                started_at_ms: i.started_at_ms,
                completed_at_ms: None,
            })
            .collect()
    }

    /// Get number of active intents.
    pub fn active_count(&self) -> usize {
        self.intents.read().unwrap().len()
    }
}

impl Default for IntentTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub enum IntentError {
    IntentNotFound(String),
    ValidationFailed(String),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntentError::IntentNotFound(id) => write!(f, "intent not found: {}", id),
            IntentError::ValidationFailed(msg) => write!(f, "validation failed: {}", msg),
        }
    }
}

impl From<IntentValidationError> for IntentError {
    fn from(e: IntentValidationError) -> Self {
        IntentError::ValidationFailed(e.to_string())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── F-1: IntentDeclaration tests ─────────────────────────────────────

    #[test]
    fn test_intent_declaration_creation() {
        let decl = IntentDeclaration::new(
            vec!["fix".to_string(), "auth".to_string()],
            vec!["cid1".to_string(), "cid2".to_string()],
            1000,
            "Fix authentication bug".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix auth bug".to_string(),
        );
        assert_eq!(decl.keywords, vec!["fix", "auth"]);
        assert_eq!(decl.related_cids, vec!["cid1", "cid2"]);
        assert_eq!(decl.budget_tokens, 1000);
    }

    #[test]
    fn test_intent_declaration_validation_empty() {
        let decl = IntentDeclaration::new(
            vec![],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "".to_string(),
        );
        let result = decl.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_declaration_validation_zero_budget() {
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            0,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        let result = decl.validate();
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_declaration_validation_ok() {
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        assert!(decl.validate().is_ok());
    }

    #[test]
    fn test_intent_declaration_serialization() {
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec!["cid1".to_string()],
            1000,
            "Fix bug".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        let json = serde_json::to_string(&decl).unwrap();
        let restored: IntentDeclaration = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.keywords, decl.keywords);
        assert_eq!(restored.budget_tokens, decl.budget_tokens);
    }

    // ── F-5: IntentTracker tests ────────────────────────────────────────

    #[test]
    fn test_intent_tracker_declare_and_progress() {
        let tracker = IntentTracker::new();
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        tracker.declare("intent-1".to_string(), decl).unwrap();

        let progress = tracker.get_progress("intent-1").unwrap();
        assert!(matches!(progress.state, IntentExecutionState::Declared));
        assert_eq!(progress.steps_total, 0);
    }

    #[test]
    fn test_intent_tracker_plan_and_start() {
        let tracker = IntentTracker::new();
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        tracker.declare("intent-1".to_string(), decl).unwrap();
        tracker.plan("intent-1", 5).unwrap();
        tracker.start("intent-1").unwrap();

        let progress = tracker.get_progress("intent-1").unwrap();
        assert!(matches!(progress.state, IntentExecutionState::Executing));
        assert_eq!(progress.steps_total, 5);
    }

    #[test]
    fn test_intent_tracker_complete() {
        let tracker = IntentTracker::new();
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        tracker.declare("intent-1".to_string(), decl).unwrap();
        tracker.plan("intent-1", 3).unwrap();
        tracker.start("intent-1").unwrap();
        tracker.complete_step("intent-1", 100).unwrap();
        tracker.complete_step("intent-1", 150).unwrap();
        tracker.complete_step("intent-1", 200).unwrap();
        tracker.complete("intent-1").unwrap();

        let progress = tracker.get_progress("intent-1").unwrap();
        assert!(matches!(progress.state, IntentExecutionState::Completed));
        assert_eq!(progress.steps_completed, 3);
        assert_eq!(progress.tokens_used, 450);
    }

    #[test]
    fn test_intent_tracker_cancel() {
        let tracker = IntentTracker::new();
        let decl = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        tracker.declare("intent-1".to_string(), decl).unwrap();
        tracker.plan("intent-1", 5).unwrap();
        tracker.start("intent-1").unwrap();
        tracker.cancel("intent-1").unwrap();

        let progress = tracker.get_progress("intent-1").unwrap();
        assert!(matches!(progress.state, IntentExecutionState::Cancelled));
    }

    #[test]
    fn test_intent_tracker_not_found() {
        let tracker = IntentTracker::new();
        let result = tracker.start("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_progress_completion_ratio() {
        let progress = IntentProgress {
            intent_id: "test".to_string(),
            state: IntentExecutionState::Executing,
            steps_total: 4,
            steps_completed: 2,
            tokens_used: 500,
            budget_tokens: 1000,
            started_at_ms: Some(1000),
            completed_at_ms: None,
        };
        assert!((progress.completion_ratio() - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_intent_tracker_list_active() {
        let tracker = IntentTracker::new();
        let decl1 = IntentDeclaration::new(
            vec!["fix".to_string()],
            vec![],
            1000,
            "Fix".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "fix bug".to_string(),
        );
        let decl2 = IntentDeclaration::new(
            vec!["test".to_string()],
            vec![],
            500,
            "Test".to_string(),
            "agent-1".to_string(),
            "session-1".to_string(),
            "run tests".to_string(),
        );
        tracker.declare("intent-1".to_string(), decl1).unwrap();
        tracker.declare("intent-2".to_string(), decl2).unwrap();

        let active = tracker.list_active("agent-1");
        assert_eq!(active.len(), 2);
    }

    // ── F-2: IntentPlan tests ────────────────────────────────────────────

    #[test]
    fn test_intent_plan_creation() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new(
            "step-1".to_string(),
            IntentOperation::Read { cid: "cid1".to_string() },
            100,
        ));
        plan.add_step(IntentStep::new(
            "step-2".to_string(),
            IntentOperation::Search { query: "test".to_string(), tags: vec![] },
            200,
        ));
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.total_estimated_tokens, 300);
    }

    #[test]
    fn test_intent_plan_topological_sort_no_deps() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100));

        let sorted = plan.topological_sort().unwrap();
        assert_eq!(sorted.len(), 2);
    }

    #[test]
    fn test_intent_plan_topological_sort_with_deps() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        // s2 depends on s1
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(
            IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100)
                .with_dependency("s1".to_string()),
        );

        let sorted = plan.topological_sort().unwrap();
        // s1 should come before s2 since s2 depends on s1
        let s1_idx = sorted.iter().find(|&&i| plan.steps[i].step_id == "s1").unwrap();
        let s2_idx = sorted.iter().find(|&&i| plan.steps[i].step_id == "s2").unwrap();
        assert!(s1_idx < s2_idx);
    }

    #[test]
    fn test_intent_plan_missing_dependency() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(
            IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100)
                .with_dependency("nonexistent".to_string()),
        );

        let result = plan.topological_sort();
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_plan_entry_steps() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        // s1 has no deps, s2 depends on s1
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(
            IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100)
                .with_dependency("s1".to_string()),
        );

        let entries = plan.entry_steps();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].step_id, "s1");
    }

    #[test]
    fn test_intent_step_with_dependencies() {
        let step = IntentStep::new(
            "step1".to_string(),
            IntentOperation::Call { tool: "test".to_string(), params: serde_json::json!({}) },
            50,
        )
        .with_dependency("dep1".to_string())
        .with_dependency("dep2".to_string());

        assert_eq!(step.dependencies, vec!["dep1", "dep2"]);
    }

    // ── F-4: IntentTree tests ────────────────────────────────────────────

    #[test]
    fn test_intent_tree_creation() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);
        assert_eq!(tree.root_intent_id, "intent-1");
        assert_eq!(tree.unclaimed_steps(), vec!["s1", "s2"]);
    }

    #[test]
    fn test_intent_tree_assign_step() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);

        tree.assign_step("s1", "agent-a").unwrap();
        assert_eq!(tree.get_agent_steps("agent-a"), vec!["s1"]);
        assert_eq!(tree.unclaimed_steps(), vec!["s2"]);
    }

    #[test]
    fn test_intent_tree_assign_same_step_fails() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);

        tree.assign_step("s1", "agent-a").unwrap();
        let result = tree.assign_step("s1", "agent-b");
        assert!(result.is_err());
    }

    #[test]
    fn test_intent_tree_record_result() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);
        tree.assign_step("s1", "agent-a").unwrap();

        let result = StepResult {
            step_id: "s1".to_string(),
            success: true,
            output_cids: vec!["c1".to_string()],
            tokens_used: 50,
            error: None,
        };
        tree.record_result("s1", "agent-a", result).unwrap();

        let stored = tree.get_result("s1").unwrap();
        assert!(stored.success);
        assert_eq!(stored.output_cids, vec!["c1"]);
    }

    #[test]
    fn test_intent_tree_unauthorized_agent() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);
        tree.assign_step("s1", "agent-a").unwrap();

        let result = StepResult {
            step_id: "s1".to_string(),
            success: true,
            output_cids: vec![],
            tokens_used: 0,
            error: None,
        };
        // agent-b trying to record result for s1 (owned by agent-a)
        let err = tree.record_result("s1", "agent-b", result);
        assert!(err.is_err());
    }

    #[test]
    fn test_intent_tree_ready_steps() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        // s1 has no deps, s2 depends on s1
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(
            IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100)
                .with_dependency("s1".to_string()),
        );

        let tree = IntentTree::new("intent-1".to_string(), plan);

        // Initially, only s1 is ready (no deps)
        let ready = tree.ready_steps();
        assert_eq!(ready, vec!["s1"]);
    }

    #[test]
    fn test_intent_tree_aggregate_results() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));
        plan.add_step(IntentStep::new("s2".to_string(), IntentOperation::Read { cid: "c2".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);
        tree.assign_step("s1", "agent-a").unwrap();
        tree.assign_step("s2", "agent-b").unwrap();

        tree.record_result("s1", "agent-a", StepResult {
            step_id: "s1".to_string(),
            success: true,
            output_cids: vec!["c1".to_string()],
            tokens_used: 50,
            error: None,
        }).unwrap();

        tree.record_result("s2", "agent-b", StepResult {
            step_id: "s2".to_string(),
            success: true,
            output_cids: vec!["c2".to_string()],
            tokens_used: 60,
            error: None,
        }).unwrap();

        let agg = tree.aggregate_results();
        assert!(agg.success);
        assert_eq!(agg.total_tokens, 110);
        let mut sorted_cids = agg.output_cids.clone();
        sorted_cids.sort();
        assert_eq!(sorted_cids, vec!["c1", "c2"]);
    }

    #[test]
    fn test_intent_tree_is_complete() {
        let mut plan = IntentPlan::new("intent-1".to_string());
        plan.add_step(IntentStep::new("s1".to_string(), IntentOperation::Read { cid: "c1".to_string() }, 100));

        let tree = IntentTree::new("intent-1".to_string(), plan);
        tree.assign_step("s1", "agent-a").unwrap();

        assert!(!tree.is_complete());

        tree.record_result("s1", "agent-a", StepResult {
            step_id: "s1".to_string(),
            success: true,
            output_cids: vec!["c1".to_string()],
            tokens_used: 50,
            error: None,
        }).unwrap();

        assert!(tree.is_complete());
    }
}
