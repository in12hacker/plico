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
    ///
    /// For MultiAction intents (conjunctive NL like "create X and then search Y"),
    /// all resolved actions are executed in sequence.
    pub fn intent_execute_sync(
        &self,
        text: &str,
        agent_id: &str,
        confidence_threshold: f32,
        learn: bool,
    ) -> Result<IntentExecutionResult, String> {
        // Check procedural memory for previously learned workflows (reuse loop).
        if let Some((actions, explanation)) = self.recall_learned_workflow(agent_id, text) {
            let resolved: Vec<ResolvedIntent> = actions.iter().map(|action| ResolvedIntent {
                routing_action: if actions.len() > 1 {
                    crate::intent::RoutingAction::MultiAction
                } else {
                    crate::intent::RoutingAction::SingleAction
                },
                confidence: 0.95,
                action: action.clone(),
                explanation: format!("[reused] {}", explanation),
            }).collect();

            let (all_ok, outputs) = self.execute_actions_sequence(&actions);

            let tags = if all_ok {
                vec!["execution-success".to_string(), "sync".to_string(), "reused".to_string()]
            } else {
                vec!["execution-failure".to_string(), "sync".to_string(), "reused".to_string()]
            };
            let summary = format!(
                "Reused learned workflow ({} steps) for '{}' → {}",
                actions.len(),
                &text.chars().take(40).collect::<String>(),
                if all_ok { "success" } else { "failed" }
            );
            let _ = self.remember_working(agent_id, summary, tags);

            return Ok(IntentExecutionResult {
                resolved,
                executed: true,
                success: all_ok,
                output: outputs,
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

        let is_multi = resolved.len() > 1
            && best.routing_action == crate::intent::RoutingAction::MultiAction;

        let (all_ok, output) = if is_multi {
            let actions: Vec<_> = resolved.iter().map(|r| r.action.clone()).collect();
            self.execute_actions_sequence(&actions)
        } else {
            let resp = self.handle_api_request(best.action.clone());
            let out = serde_json::to_string(&resp).unwrap_or_default();
            (resp.ok, out)
        };

        let step_count = if is_multi { resolved.len() } else { 1 };
        let result_summary = if all_ok {
            let preview: String = text.chars().take(60).collect();
            format!("Sync executed ({} steps): '{}' → success", step_count, preview)
        } else {
            format!(
                "Sync executed ({} steps): '{}' → failed",
                step_count,
                &text.chars().take(40).collect::<String>()
            )
        };

        let tags = if all_ok {
            vec!["execution-success".to_string(), "sync".to_string()]
        } else {
            vec!["execution-failure".to_string(), "sync".to_string()]
        };
        let _ = self.remember_working(agent_id, result_summary, tags);

        if learn && all_ok {
            let steps: Vec<crate::memory::layered::ProcedureStep> = if is_multi {
                resolved.iter().enumerate().map(|(i, ri)| crate::memory::layered::ProcedureStep {
                    step_number: (i + 1) as u32,
                    description: ri.explanation.clone(),
                    action: serde_json::to_string(&ri.action).unwrap_or_default(),
                    expected_outcome: "success (verified by execution)".to_string(),
                }).collect()
            } else {
                vec![crate::memory::layered::ProcedureStep {
                    step_number: 1,
                    description: best.explanation.clone(),
                    action: serde_json::to_string(&best.action).unwrap_or_default(),
                    expected_outcome: "success (verified by execution)".to_string(),
                }]
            };

            let name = format!("auto:{}", &text.chars().take(40).collect::<String>());
            let _ = self.remember_procedural(
                agent_id,
                name,
                format!("Verified: when asked '{}', execute {} step(s)", text, steps.len()),
                steps,
                "auto-learned from successful sync execution".to_string(),
                vec!["auto-learned".to_string(), "verified".to_string()],
            );
        }

        Ok(IntentExecutionResult {
            resolved: resolved.clone(),
            executed: true,
            success: all_ok,
            output,
        })
    }

    fn execute_actions_sequence(
        &self,
        actions: &[crate::api::semantic::ApiRequest],
    ) -> (bool, String) {
        let mut all_ok = true;
        let mut outputs = Vec::new();
        for action in actions {
            let resp = self.handle_api_request(action.clone());
            if !resp.ok {
                all_ok = false;
            }
            outputs.push(serde_json::to_string(&resp).unwrap_or_default());
        }
        let combined = if outputs.len() == 1 {
            outputs.into_iter().next().unwrap_or_default()
        } else {
            format!("[{}]", outputs.join(","))
        };
        (all_ok, combined)
    }

    fn recall_learned_workflow(
        &self,
        agent_id: &str,
        text: &str,
    ) -> Option<(Vec<crate::api::semantic::ApiRequest>, String)> {
        let name_prefix = format!("auto:{}", &text.chars().take(40).collect::<String>());
        let procedures = self.recall_procedural(agent_id, Some(&name_prefix));

        for entry in procedures {
            if !entry.tags.iter().any(|t| t == "verified") {
                continue;
            }
            if let crate::memory::MemoryContent::Procedure(ref proc) = entry.content {
                let actions: Vec<crate::api::semantic::ApiRequest> = proc.steps.iter()
                    .filter_map(|step| serde_json::from_str(&step.action).ok())
                    .collect();
                if !actions.is_empty() {
                    return Some((actions, proc.description.clone()));
                }
            }
        }
        None
    }
}
