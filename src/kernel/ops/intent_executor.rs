//! Intent Executor (F-3, Node 21) — Autonomous Execution Loop.
//!
//! OS drives execution of IntentPlan steps, Agent only intervenes at exceptions.
//!
//! Soul 2.0 Axiom 2: "意图先于操作" — OS executes on behalf of Agent.

use std::collections::HashMap;
use std::sync::Arc;
use crate::kernel::ops::intent::{
    IntentPlan, IntentStep, IntentOperation,
};
use crate::kernel::AIKernel;
use crate::kernel::ops::skill_discovery::SkillDiscriminator;
use crate::kernel::ops::self_healing::{FailureClassifier, PlanAdaptor};
use crate::kernel::ops::intent_decomposer::IntentDecomposer;

// ── F-2: Execution Statistics ────────────────────────────────────────────────

/// Execution statistics for self-optimization (F-2).
#[derive(Debug, Clone, Default)]
pub struct ExecutionStats {
    /// Average execution time per operation type (ms).
    avg_times: HashMap<String, u64>,
    /// Total count per operation type.
    counts: HashMap<String, u32>,
    /// Total time per operation type (ms).
    total_times: HashMap<String, u64>,
}

impl ExecutionStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an execution.
    pub fn record(&mut self, operation_type: String, duration_ms: u64) {
        let count = self.counts.entry(operation_type.clone()).or_insert(0);
        *count += 1;
        let total = self.total_times.entry(operation_type.clone()).or_insert(0);
        *total += duration_ms;
        // Calculate average
        let avg = *total / (*count as u64);
        self.avg_times.insert(operation_type, avg);
    }

    /// Get average execution time for an operation type.
    pub fn get_avg_time(&self, operation_type: &str) -> Option<u64> {
        self.avg_times.get(operation_type).copied()
    }

    /// Get all operation types.
    pub fn operation_types(&self) -> Vec<String> {
        self.avg_times.keys().cloned().collect()
    }

    /// F-3: Get all average times for optimized_sort.
    pub(crate) fn get_avg_times_map(&self) -> std::collections::HashMap<String, u64> {
        self.avg_times.clone()
    }
}

/// Result of executing a single step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub success: bool,
    pub output_cids: Vec<String>,
    pub tokens_used: usize,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
}

/// Error during step execution.
#[derive(Debug, Clone)]
pub struct StepError {
    pub step_id: String,
    pub error_type: StepErrorType,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum StepErrorType {
    PermissionDenied,
    ResourceExhausted,
    ToolNotFound,
    ExecutionFailed,
    DependencyBlocked,
}

/// Result of executing an entire intent plan.
#[derive(Debug, Clone)]
pub struct IntentExecutionResult {
    pub intent_id: String,
    pub success: bool,
    pub steps_completed: usize,
    pub steps_failed: usize,
    pub tokens_used: usize,
    pub results: HashMap<String, StepResult>,
}

/// Autonomous Executor — drives intent plan execution.
///
/// While loop: execute steps, handle exceptions, track progress.
/// Agent only intervenes at exception points (human-in-the-loop).
pub struct AutonomousExecutor {
    kernel: Arc<AIKernel>,
    /// F-2: Execution statistics for self-optimization.
    stats: ExecutionStats,
    /// F-4 (M1): Skill discriminator for autonomous skill discovery.
    skill_discriminator: SkillDiscriminator,
    /// F-4 (M2): Plan adaptor for self-healing.
    plan_adaptor: PlanAdaptor,
    /// F-4 (M3): Intent decomposer for goal decomposition.
    intent_decomposer: IntentDecomposer,
}

impl AutonomousExecutor {
    pub fn new(kernel: Arc<AIKernel>) -> Self {
        let profile_store = kernel.prefetch.profile_store().clone();
        Self {
            kernel,
            stats: ExecutionStats::new(),
            skill_discriminator: SkillDiscriminator::new(3),
            plan_adaptor: PlanAdaptor::new(),
            intent_decomposer: IntentDecomposer::new(profile_store),
        }
    }

    /// Execute an intent plan autonomously.
    ///
    /// Returns when all steps are complete, failed, or blocked.
    /// F-1: Writes execution results back to AgentProfile (learning).
    /// F-2: Records execution times to ExecutionStats for self-optimization.
    pub async fn execute_plan(&mut self, plan: &IntentPlan, agent_id: &str) -> IntentExecutionResult {
        let mut results = HashMap::new();
        let mut completed_steps = std::collections::HashSet::new();
        let mut tokens_used = 0;
        let mut steps_completed = 0;
        let mut steps_failed = 0;

        // F-1: Get profile store for learning
        let profile_store = self.kernel.prefetch.profile_store();

        // F-3: Get topologically sorted steps with execution time optimization
        let sorted_indices = match plan.optimized_sort(&self.stats.get_avg_times_map()) {
            Ok(indices) => indices,
            Err(e) => {
                return IntentExecutionResult {
                    intent_id: plan.intent_id.clone(),
                    success: false,
                    steps_completed: 0,
                    steps_failed: 0,
                    tokens_used: 0,
                    results: HashMap::new(),
                };
            }
        };

        // Execute each step in order
        for &step_idx in &sorted_indices {
            let step = &plan.steps[step_idx];

            // Check if dependencies are satisfied
            if !self.can_execute_step(step, &completed_steps) {
                // Mark as blocked
                results.insert(step.step_id.clone(), StepResult {
                    step_id: step.step_id.clone(),
                    success: false,
                    output_cids: vec![],
                    tokens_used: 0,
                    error: Some("dependency not satisfied".to_string()),
                    duration_ms: Some(0),
                });
                steps_failed += 1;
                continue;
            }

            // Execute the step
            let start = std::time::Instant::now();
            let result = self.execute_step(step).await;
            let duration_ms = start.elapsed().as_millis() as u64;

            // F-1: Record successful CIDs to profile for hot_objects
            if result.success && !result.output_cids.is_empty() {
                profile_store.record_cid_usage(agent_id, &result.output_cids);
            }

            // Record result
            tokens_used += result.tokens_used;
            if result.success {
                steps_completed += 1;
                completed_steps.insert(step.step_id.clone());
            } else {
                steps_failed += 1;
            }

            // Create result with duration
            let result_with_duration = StepResult {
                step_id: result.step_id,
                success: result.success,
                output_cids: result.output_cids,
                tokens_used: result.tokens_used,
                error: result.error,
                duration_ms: Some(duration_ms),
            };
            results.insert(step.step_id.clone(), result_with_duration.clone());

            // F-2: Record execution time for self-optimization
            let op_type = match &step.operation {
                IntentOperation::Read { .. } => "read",
                IntentOperation::Search { .. } => "search",
                IntentOperation::Call { .. } => "call",
                IntentOperation::Create { .. } => "create",
                IntentOperation::ReadBatch { .. } => "read_batch",
            };
            self.stats.record(op_type.to_string(), duration_ms);
        }

        // F-1: Record intent transition if we have steps
        if !results.is_empty() {
            // Extract intent tag from plan intent_id (simplified)
            let intent_tag = plan.intent_id.split(':').next().unwrap_or("unknown");
            profile_store.record_intent_complete(agent_id, Some(intent_tag), None);
        }

        // F-4: Build result before async call
        let exec_result = IntentExecutionResult {
            intent_id: plan.intent_id.clone(),
            success: steps_failed == 0,
            steps_completed,
            steps_failed,
            tokens_used,
            results,
        };

        // F-4: Trigger predictive prefetch based on execution results
        self.trigger_predictive_prefetch(&exec_result, agent_id).await;

        // F-4: Record operation sequences for skill discovery
        self.record_operation_sequences(&exec_result);

        // F-4: Analyze failures for self-healing
        self.analyze_failures(&exec_result);

        // F-4: Check decomposition opportunity
        self.check_decomposition_opportunity(&exec_result, agent_id);

        exec_result
    }

    /// F-4: Trigger predictive prefetch based on execution results.
    ///
    /// After plan execution completes, checks if there's a high-confidence
    /// prediction for the next intent and triggers silent prefetch.
    async fn trigger_predictive_prefetch(&self, result: &IntentExecutionResult, agent_id: &str) {
        // Extract current intent tag
        let current_tag = result.intent_id.split(':').next().unwrap_or("unknown");

        // Get profile and ask for next predicted intent
        let profile = self.kernel.prefetch.profile_store().get_or_create(agent_id);
        if let Some(next_tag) = profile.predict_next(current_tag) {
            // Trigger on_intent_complete which handles the prefetch
            let _ = self.kernel.prefetch.on_intent_complete(
                agent_id,
                current_tag,
                Some(&next_tag),
                &[],
            );
        }
    }

    /// F-4 (M1): Record operation sequences for skill discovery.
    fn record_operation_sequences(&self, result: &IntentExecutionResult) {
        for (_, step_result) in &result.results {
            let ops = vec![step_result.step_id.clone()];
            self.skill_discriminator.record_sequence(
                "default",
                ops,
                step_result.success,
                step_result.duration_ms.unwrap_or(0),
            );
        }
    }

    /// F-4 (M2): Analyze failures and update adaptation strategies.
    fn analyze_failures(&self, result: &IntentExecutionResult) {
        for (_, step_result) in &result.results {
            if !step_result.success {
                if let Some(ref error) = step_result.error {
                    let failure_type = FailureClassifier::classify(error, &step_result.step_id);
                    let _ = self.plan_adaptor.record_and_adapt(&step_result.step_id, &failure_type);
                }
            }
        }
    }

    /// F-4 (M3): Check if intent decomposition is needed for future.
    fn check_decomposition_opportunity(&self, result: &IntentExecutionResult, _agent_id: &str) {
        let candidates = self.skill_discriminator.get_skill_candidates("default");
        if !candidates.is_empty() {
            let _ = candidates;
        }
    }

    /// F-2: Get execution statistics for self-optimization.
    pub fn get_stats(&self) -> ExecutionStats {
        self.stats.clone()
    }

    /// Execute a single step.
    async fn execute_step(&self, step: &IntentStep) -> StepResult {
        match &step.operation {
            IntentOperation::Read { cid } => {
                self.execute_read(cid, &step.step_id).await
            }
            IntentOperation::Search { query, tags } => {
                self.execute_search(query, tags, &step.step_id).await
            }
            IntentOperation::Call { tool, params } => {
                self.execute_call(tool, params, &step.step_id).await
            }
            IntentOperation::Create { content, tags } => {
                self.execute_create(content, tags, &step.step_id).await
            }
            IntentOperation::ReadBatch { cids } => {
                self.execute_read_batch(cids, &step.step_id).await
            }
        }
    }

    /// Execute a read operation.
    async fn execute_read(&self, cid: &str, step_id: &str) -> StepResult {
        // Use blocking read in a blocking context
        let cid_owned = cid.to_string();
        let step_id_owned = step_id.to_string();
        let kernel = self.kernel.clone();
        let cid_for_result = cid.to_string();

        let result = tokio::task::spawn_blocking(move || {
            kernel.get_object(&cid_owned, "system", "default")
        })
        .await;

        match result {
            Ok(Ok(obj)) => StepResult {
                step_id: step_id_owned,
                success: true,
                output_cids: vec![cid_for_result],
                tokens_used: obj.data.len(),
                error: None,
                duration_ms: None,
            },
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
                duration_ms: None,
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
                duration_ms: None,
            },
        }
    }

    /// Execute a search operation.
    async fn execute_search(&self, query: &str, tags: &[String], step_id: &str) -> StepResult {
        let query_owned = query.to_string();
        let tags_owned = tags.to_vec();
        let step_id_owned = step_id.to_string();
        let kernel = self.kernel.clone();

        let result = tokio::task::spawn_blocking(move || {
            kernel.semantic_search(
                &query_owned,
                "system",
                "default",
                10,
                tags_owned,
                vec![],
            )
        })
        .await;

        match result {
            Ok(Ok(results)) => {
                let cids: Vec<_> = results.into_iter().map(|r| r.cid).collect();
                StepResult {
                    step_id: step_id_owned,
                    success: true,
                    output_cids: cids,
                    tokens_used: 0,
                    error: None,
                    duration_ms: None,
                }
            }
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
                duration_ms: None,
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
                duration_ms: None,
            },
        }
    }

    /// Execute a tool call.
    async fn execute_call(&self, tool: &str, params: &serde_json::Value, step_id: &str) -> StepResult {
        // For now, tool calls are handled via API request
        // This is a placeholder for the full implementation
        StepResult {
            step_id: step_id.to_string(),
            success: false,
            output_cids: vec![],
            tokens_used: 0,
            error: Some("tool call not yet implemented in executor".to_string()),
            duration_ms: None,
        }
    }

    /// Execute a create operation.
    async fn execute_create(&self, content: &[u8], tags: &[String], step_id: &str) -> StepResult {
        let content_owned = content.to_vec();
        let tags_owned = tags.to_vec();
        let step_id_owned = step_id.to_string();
        let content_len = content.len();
        let kernel = self.kernel.clone();

        let result = tokio::task::spawn_blocking(move || {
            kernel.semantic_create(
                content_owned,
                tags_owned,
                "system",
                None,
            )
        })
        .await;

        match result {
            Ok(Ok(cid)) => StepResult {
                step_id: step_id_owned,
                success: true,
                output_cids: vec![cid],
                tokens_used: content_len,
                error: None,
                duration_ms: None,
            },
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
                duration_ms: None,
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
                duration_ms: None,
            },
        }
    }

    /// Execute a batch read.
    async fn execute_read_batch(&self, cids: &[String], step_id: &str) -> StepResult {
        let cids_vec = cids.to_vec();
        let kernel = self.kernel.clone();

        // Single blocking call for all reads
        let (oks, tokens) = tokio::task::spawn_blocking(move || {
            let mut ok_cids = Vec::new();
            let mut used = 0;
            for cid in &cids_vec {
                if let Ok(obj) = kernel.get_object(cid, "system", "default") {
                    ok_cids.push(cid.clone());
                    used += obj.data.len();
                }
            }
            (ok_cids, used)
        })
        .await
        .unwrap_or((vec![], 0));

        StepResult {
            step_id: step_id.to_string(),
            success: !oks.is_empty(),
            output_cids: oks,
            tokens_used: tokens,
            error: None,
            duration_ms: None,
        }
    }

    /// Check if step can execute (dependencies satisfied).
    fn can_execute_step(&self, step: &IntentStep, completed: &std::collections::HashSet<String>) -> bool {
        step.dependencies.iter().all(|dep| completed.contains(dep))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::ops::intent::{IntentPlan, IntentStep, IntentOperation};

    #[test]
    fn test_step_result_creation() {
        let result = StepResult {
            step_id: "step-1".to_string(),
            success: true,
            output_cids: vec!["cid1".to_string()],
            tokens_used: 100,
            error: None,
            duration_ms: None,
        };
        assert!(result.success);
        assert_eq!(result.output_cids.len(), 1);
    }

    #[test]
    fn test_step_error_creation() {
        let error = StepError {
            step_id: "step-1".to_string(),
            error_type: StepErrorType::PermissionDenied,
            message: "access denied".to_string(),
        };
        assert!(matches!(error.error_type, StepErrorType::PermissionDenied));
    }

    #[test]
    fn test_intent_execution_result() {
        let result = IntentExecutionResult {
            intent_id: "intent-1".to_string(),
            success: true,
            steps_completed: 3,
            steps_failed: 0,
            tokens_used: 500,
            results: HashMap::new(),
        };
        assert!(result.success);
        assert_eq!(result.steps_completed, 3);
    }

    #[test]
    fn test_can_execute_step_no_deps() {
        let step = IntentStep::new(
            "s1".to_string(),
            IntentOperation::Read { cid: "c1".to_string() },
            100,
        );
        let completed = std::collections::HashSet::<String>::new();

        // Can't test the executor directly without kernel, but we can test the logic
        let can_exec = step.dependencies.is_empty();
        assert!(can_exec);
    }

    #[test]
    fn test_can_execute_step_with_deps() {
        let step = IntentStep::new(
            "s2".to_string(),
            IntentOperation::Read { cid: "c2".to_string() },
            100,
        )
        .with_dependency("s1".to_string());

        let mut completed = std::collections::HashSet::<String>::new();
        completed.insert("s1".to_string());

        let can_exec = step.dependencies.iter().all(|dep| completed.contains(dep));
        assert!(can_exec);
    }

    #[test]
    fn test_cannot_execute_step_missing_dep() {
        let step = IntentStep::new(
            "s2".to_string(),
            IntentOperation::Read { cid: "c2".to_string() },
            100,
        )
        .with_dependency("s1".to_string());

        let completed = std::collections::HashSet::<String>::new();

        let can_exec = step.dependencies.iter().all(|dep| completed.contains(dep));
        assert!(!can_exec);
    }
}
