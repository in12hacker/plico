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

/// Intent execution state (F-5).
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
}
