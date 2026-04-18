//! Intent resolution and execution operations.

use crate::intent::ResolvedIntent;
use crate::scheduler::IntentPriority;

/// Result of synchronous intent execution.
#[derive(Debug, Clone)]
pub struct IntentExecutionResult {
    pub resolved: Vec<ResolvedIntent>,
    pub executed: bool,
    pub success: bool,
    pub output: String,
}

impl crate::kernel::AIKernel {
    /// Resolve natural language text into structured API requests.
    pub fn intent_resolve(&self, text: &str, agent_id: &str) -> Vec<ResolvedIntent> {
        match self.intent_router.resolve(text, agent_id) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Intent resolution failed: {}", e);
                vec![]
            }
        }
    }

    /// Resolve and optionally execute a natural language intent.
    ///
    /// If the best resolved intent has confidence >= threshold, it is serialized
    /// and submitted to the scheduler for execution. If `learn` is true,
    /// the NL→action mapping is captured as procedural memory.
    pub fn intent_execute(
        &self,
        text: &str,
        agent_id: &str,
        confidence_threshold: f32,
        priority: IntentPriority,
        learn: bool,
    ) -> Result<(Option<String>, Vec<ResolvedIntent>), String> {
        let resolved = match self.intent_router.resolve(text, agent_id) {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => return Ok((None, vec![])),
            Err(e) => return Err(format!("Intent resolution failed: {}", e)),
        };

        let best = &resolved[0];
        if best.confidence < confidence_threshold {
            return Ok((None, resolved));
        }

        let action_json = serde_json::to_string(&best.action)
            .map_err(|e| format!("Failed to serialize action: {}", e))?;
        let intent_id = self.submit_intent(
            priority,
            best.explanation.clone(),
            Some(action_json),
            Some(agent_id.to_string()),
        )?;

        if learn {
            let step = crate::memory::layered::ProcedureStep {
                step_number: 1,
                description: best.explanation.clone(),
                action: serde_json::to_string(&best.action).unwrap_or_default(),
                expected_outcome: "success".to_string(),
            };
            let name = format!("auto:{}", &text.chars().take(40).collect::<String>());
            let _ = self.remember_procedural(
                agent_id,
                name,
                format!("When asked '{}', execute resolved action", text),
                vec![step],
                "auto-learned from intent execution".to_string(),
                vec!["auto-learned".to_string()],
            );
        }

        Ok((Some(intent_id), resolved))
    }

    /// Resolve and execute a natural language intent synchronously.
    ///
    /// Unlike `intent_execute` which submits to the scheduler queue,
    /// this executes the resolved action inline via `handle_api_request`
    /// and returns the result immediately. Captures outcomes in memory.
    pub fn intent_execute_sync(
        &self,
        text: &str,
        agent_id: &str,
        confidence_threshold: f32,
        learn: bool,
    ) -> Result<IntentExecutionResult, String> {
        // Check procedural memory for previously learned mappings (reuse loop).
        if let Some((action, explanation)) = self.recall_learned_action(agent_id, text) {
            let resolved = vec![ResolvedIntent {
                routing_action: crate::intent::RoutingAction::SingleAction,
                confidence: 0.95,
                action: action.clone(),
                explanation: format!("[reused] {}", explanation),
            }];

            let resp = self.handle_api_request(action);
            let output = serde_json::to_string(&resp).unwrap_or_default();

            let tags = if resp.ok {
                vec!["execution-success".to_string(), "sync".to_string(), "reused".to_string()]
            } else {
                vec!["execution-failure".to_string(), "sync".to_string(), "reused".to_string()]
            };
            let summary = format!("Reused learned action for '{}' → {}", &text.chars().take(40).collect::<String>(), if resp.ok { "success" } else { "failed" });
            let _ = self.remember_working(agent_id, summary, tags);

            return Ok(IntentExecutionResult {
                resolved,
                executed: true,
                success: resp.ok,
                output,
            });
        }

        let resolved = match self.intent_router.resolve(text, agent_id) {
            Ok(r) if !r.is_empty() => r,
            Ok(_) => return Ok(IntentExecutionResult {
                resolved: vec![], executed: false, success: false,
                output: "No intents resolved".to_string(),
            }),
            Err(e) => return Err(format!("Intent resolution failed: {}", e)),
        };

        let best = &resolved[0];
        if best.confidence < confidence_threshold {
            return Ok(IntentExecutionResult {
                resolved: resolved.clone(), executed: false, success: false,
                output: format!(
                    "Confidence {:.2} below threshold {:.2}",
                    best.confidence, confidence_threshold
                ),
            });
        }

        let action_req = best.action.clone();

        let resp = self.handle_api_request(action_req);
        let output = serde_json::to_string(&resp).unwrap_or_default();

        let result_summary = if resp.ok {
            let preview: String = text.chars().take(60).collect();
            format!("Sync executed: '{}' → success", preview)
        } else {
            let err = resp.error.as_deref().unwrap_or("unknown error");
            format!("Sync executed: '{}' → failed: {}", &text.chars().take(40).collect::<String>(), err)
        };

        let tags = if resp.ok {
            vec!["execution-success".to_string(), "sync".to_string()]
        } else {
            vec!["execution-failure".to_string(), "sync".to_string()]
        };
        let _ = self.remember_working(agent_id, result_summary, tags);

        if learn && resp.ok {
            let step = crate::memory::layered::ProcedureStep {
                step_number: 1,
                description: best.explanation.clone(),
                action: serde_json::to_string(&best.action).unwrap_or_default(),
                expected_outcome: "success (verified by execution)".to_string(),
            };
            let name = format!("auto:{}", &text.chars().take(40).collect::<String>());
            let _ = self.remember_procedural(
                agent_id,
                name,
                format!("Verified: when asked '{}', this action succeeds", text),
                vec![step],
                "auto-learned from successful sync execution".to_string(),
                vec!["auto-learned".to_string(), "verified".to_string()],
            );
        }

        Ok(IntentExecutionResult {
            resolved: resolved.clone(),
            executed: true,
            success: resp.ok,
            output,
        })
    }

    /// Check procedural memory for a previously learned action matching the input text.
    ///
    /// Auto-learned procedures are named "auto:<text prefix>". If found with a
    /// "verified" tag, the stored action is deserialized and returned for reuse.
    fn recall_learned_action(
        &self,
        agent_id: &str,
        text: &str,
    ) -> Option<(crate::api::semantic::ApiRequest, String)> {
        let name_prefix = format!("auto:{}", &text.chars().take(40).collect::<String>());
        let procedures = self.recall_procedural(agent_id, Some(&name_prefix));

        for entry in procedures {
            if !entry.tags.iter().any(|t| t == "verified") {
                continue;
            }
            if let crate::memory::MemoryContent::Procedure(ref proc) = entry.content {
                if let Some(step) = proc.steps.first() {
                    if let Ok(action) = serde_json::from_str::<crate::api::semantic::ApiRequest>(&step.action) {
                        return Some((action, proc.description.clone()));
                    }
                }
            }
        }
        None
    }
}
