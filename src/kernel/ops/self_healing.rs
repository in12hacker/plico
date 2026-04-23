//! Self-Healing Module (Node 23 M2) — Autonomous failure recovery.
//!
//! Provides failure classification and plan adaptation for intent execution.
//! - FailureClassifier: maps error messages to FailureType
//! - PlanAdaptor: tracks failure history and suggests adaptations

use std::collections::HashMap;
use std::sync::RwLock;

/// Failure types for classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureType {
    PermissionDenied,
    ResourceExhausted,
    ToolNotFound,
    ExecutionFailed,
    DependencyBlocked,
}

/// Classifies step execution errors into FailureType.
pub struct FailureClassifier;

impl FailureClassifier {
    /// Classify a step result error into a FailureType.
    pub fn classify(error_msg: &str, _step_operation: &str) -> FailureType {
        let msg_lower = error_msg.to_lowercase();

        if msg_lower.contains("permission")
            || msg_lower.contains("denied")
            || msg_lower.contains("access denied")
        {
            return FailureType::PermissionDenied;
        }

        if msg_lower.contains("resource")
            || msg_lower.contains("exhausted")
            || msg_lower.contains("quota")
            || msg_lower.contains("memory")
        {
            return FailureType::ResourceExhausted;
        }

        if msg_lower.contains("not found")
            || msg_lower.contains("tool")
            || msg_lower.contains("unknown tool")
            || msg_lower.contains("tool not found")
        {
            return FailureType::ToolNotFound;
        }

        if msg_lower.contains("dependency") || msg_lower.contains("blocked") {
            return FailureType::DependencyBlocked;
        }

        FailureType::ExecutionFailed
    }
}

/// Record of a failure event for a step.
#[derive(Debug, Clone)]
pub struct FailureRecord {
    pub step_id: String,
    pub failure_type: FailureType,
    pub timestamp_ms: u64,
    pub count: usize,
}

/// Adaptation suggestions for failed steps.
#[derive(Debug, Clone, PartialEq)]
pub enum Adaptation {
    Skip,
    RetryWithNewParams,
    ReplaceTool { new_tool: String },
    ReduceScope,
}

/// Manages failure history and provides adaptation suggestions.
pub struct PlanAdaptor {
    failure_history: RwLock<HashMap<String, Vec<FailureRecord>>>,
}

impl PlanAdaptor {
    pub fn new() -> Self {
        Self {
            failure_history: RwLock::new(HashMap::new()),
        }
    }

    /// Record a failure and return adaptation suggestion.
    pub fn record_and_adapt(&self, step_id: &str, failure: &FailureType) -> Adaptation {
        let now = crate::scheduler::agent::now_ms();

        {
            let mut history = self.failure_history.write().unwrap();
            let records = history.entry(step_id.to_string()).or_insert_with(Vec::new);

            // Update existing record or create new one
            if let Some(record) = records.iter_mut().find(|r| r.failure_type == *failure) {
                record.count += 1;
                record.timestamp_ms = now;
            } else {
                records.push(FailureRecord {
                    step_id: step_id.to_string(),
                    failure_type: failure.clone(),
                    timestamp_ms: now,
                    count: 1,
                });
            }
        }

        self.get_adaptation(step_id).unwrap_or(Adaptation::RetryWithNewParams)
    }

    /// Get cached adaptation for a step (if same failure happened 3+ times).
    pub fn get_adaptation(&self, step_id: &str) -> Option<Adaptation> {
        let history = self.failure_history.read().unwrap();
        let records = history.get(step_id)?;

        for record in records {
            if record.count > 3 && record.failure_type == FailureType::PermissionDenied {
                return Some(Adaptation::Skip);
            }
            if record.count > 2 && record.failure_type == FailureType::ResourceExhausted {
                return Some(Adaptation::ReduceScope);
            }
            if record.failure_type == FailureType::ToolNotFound {
                return Some(Adaptation::ReplaceTool {
                    new_tool: "fallback_tool".to_string(),
                });
            }
        }

        None
    }
}

impl Default for PlanAdaptor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failure_classifier_permission_denied() {
        let cases = vec![
            "permission denied",
            "access denied",
            "PERMISSION_DENIED",
        ];
        for msg in cases {
            assert_eq!(
                FailureClassifier::classify(msg, "read"),
                FailureType::PermissionDenied,
                "Failed on: {msg}"
            );
        }
    }

    #[test]
    fn test_failure_classifier_resource_exhausted() {
        let cases = vec![
            "resource exhausted",
            "memory quota exceeded",
            "RESOURCE",
        ];
        for msg in cases {
            assert_eq!(
                FailureClassifier::classify(msg, "write"),
                FailureType::ResourceExhausted,
                "Failed on: {msg}"
            );
        }
    }

    #[test]
    fn test_plan_adaptor_skips_repeated_failures() {
        let adaptor = PlanAdaptor::new();

        // Record 3 permission denied failures
        for _ in 0..3 {
            let adaptation = adaptor.record_and_adapt(
                "step-1",
                &FailureType::PermissionDenied,
            );
            assert_eq!(adaptation, Adaptation::RetryWithNewParams);
        }

        // 4th failure should skip
        let adaptation = adaptor.record_and_adapt("step-1", &FailureType::PermissionDenied);
        assert_eq!(adaptation, Adaptation::Skip);
    }

    #[test]
    fn test_plan_adaptor_replaces_tool() {
        let adaptor = PlanAdaptor::new();

        // First tool not found
        let adaptation = adaptor.record_and_adapt("step-2", &FailureType::ToolNotFound);
        assert_eq!(
            adaptation,
            Adaptation::ReplaceTool {
                new_tool: "fallback_tool".to_string()
            }
        );
    }
}