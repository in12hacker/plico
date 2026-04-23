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

/// Result of executing a single step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_id: String,
    pub success: bool,
    pub output_cids: Vec<String>,
    pub tokens_used: usize,
    pub error: Option<String>,
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
}

impl AutonomousExecutor {
    pub fn new(kernel: Arc<AIKernel>) -> Self {
        Self { kernel }
    }

    /// Execute an intent plan autonomously.
    ///
    /// Returns when all steps are complete, failed, or blocked.
    pub async fn execute_plan(&self, plan: &IntentPlan) -> IntentExecutionResult {
        let mut results = HashMap::new();
        let mut completed_steps = std::collections::HashSet::new();
        let mut tokens_used = 0;
        let mut steps_completed = 0;
        let mut steps_failed = 0;

        // Get topologically sorted steps
        let sorted_indices = match plan.topological_sort() {
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
                });
                steps_failed += 1;
                continue;
            }

            // Execute the step
            let result = self.execute_step(step).await;

            // Record result
            tokens_used += result.tokens_used;
            if result.success {
                steps_completed += 1;
                completed_steps.insert(step.step_id.clone());
            } else {
                steps_failed += 1;
            }
            results.insert(step.step_id.clone(), result);
        }

        IntentExecutionResult {
            intent_id: plan.intent_id.clone(),
            success: steps_failed == 0,
            steps_completed,
            steps_failed,
            tokens_used,
            results,
        }
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
            },
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
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
                }
            }
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
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
            },
            Ok(Err(e)) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some(e.to_string()),
            },
            Err(_) => StepResult {
                step_id: step_id_owned,
                success: false,
                output_cids: vec![],
                tokens_used: 0,
                error: Some("task join error".to_string()),
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
