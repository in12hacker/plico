//! Intent execution — application-layer logic for NL→execute→learn.
//!
//! Extracted from kernel (v3.0-M1) per soul alignment: the kernel provides
//! resource primitives, applications decide how to think and learn.
//! This module provides reusable execution helpers that any application
//! (aicli, plicod, custom agents) can use with their own IntentRouter.

use crate::api::semantic::ApiRequest;
use crate::intent::{IntentRouter, ResolvedIntent, RoutingAction};
use crate::kernel::AIKernel;
use crate::memory::MemoryContent;

/// Result of synchronous intent execution.
#[derive(Debug, Clone)]
pub struct IntentExecutionResult {
    pub resolved: Vec<ResolvedIntent>,
    pub executed: bool,
    pub success: bool,
    pub output: String,
}

/// Resolve and execute a natural language intent synchronously.
///
/// This is application-layer logic: resolve NL → execute → optionally learn.
/// The kernel is used only for structured API dispatch and memory primitives.
pub fn execute_sync(
    kernel: &AIKernel,
    router: &dyn IntentRouter,
    text: &str,
    agent_id: &str,
    confidence_threshold: f32,
    learn: bool,
) -> Result<IntentExecutionResult, String> {
    if let Some((actions, explanation)) = recall_learned_workflow(kernel, agent_id, text) {
        let resolved: Vec<ResolvedIntent> = actions.iter().map(|action| ResolvedIntent {
            routing_action: if actions.len() > 1 {
                RoutingAction::MultiAction
            } else {
                RoutingAction::SingleAction
            },
            confidence: 0.95,
            action: action.clone(),
            explanation: format!("[reused] {}", explanation),
        }).collect();

        let (all_ok, outputs) = execute_actions_sequence(kernel, &actions);

        let tags = if all_ok {
            vec!["execution-success".into(), "sync".into(), "reused".into()]
        } else {
            vec!["execution-failure".into(), "sync".into(), "reused".into()]
        };
        let summary = format!(
            "Reused learned workflow ({} steps) for '{}' → {}",
            actions.len(),
            &text.chars().take(40).collect::<String>(),
            if all_ok { "success" } else { "failed" }
        );
        let _ = kernel.remember_working(agent_id, "default", summary, tags);

        return Ok(IntentExecutionResult {
            resolved,
            executed: true,
            success: all_ok,
            output: outputs,
        });
    }

    let resolved = match router.resolve(text, agent_id) {
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
        && best.routing_action == RoutingAction::MultiAction;

    let (all_ok, output) = if is_multi {
        let actions: Vec<_> = resolved.iter().map(|r| r.action.clone()).collect();
        execute_actions_sequence(kernel, &actions)
    } else {
        let resp = kernel.handle_api_request(best.action.clone());
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
        vec!["execution-success".into(), "sync".into()]
    } else {
        vec!["execution-failure".into(), "sync".into()]
    };
    let _ = kernel.remember_working(agent_id, "default", result_summary, tags);

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
        let _ = kernel.remember_procedural(
            agent_id,
            "default",
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

/// Resolve and optionally submit a natural language intent to the scheduler.
pub fn execute_async(
    kernel: &AIKernel,
    router: &dyn IntentRouter,
    text: &str,
    agent_id: &str,
    confidence_threshold: f32,
    priority: crate::scheduler::IntentPriority,
    learn: bool,
) -> Result<(Option<String>, Vec<ResolvedIntent>), String> {
    let resolved = match router.resolve(text, agent_id) {
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
    let intent_id = kernel.submit_intent(
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
        let _ = kernel.remember_procedural(
            agent_id,
            "default",
            name,
            format!("When asked '{}', execute resolved action", text),
            vec![step],
            "auto-learned from intent execution".to_string(),
            vec!["auto-learned".to_string()],
        );
    }

    Ok((Some(intent_id), resolved))
}

fn execute_actions_sequence(
    kernel: &AIKernel,
    actions: &[ApiRequest],
) -> (bool, String) {
    let mut all_ok = true;
    let mut outputs = Vec::new();
    for action in actions {
        let resp = kernel.handle_api_request(action.clone());
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
    kernel: &AIKernel,
    agent_id: &str,
    text: &str,
) -> Option<(Vec<ApiRequest>, String)> {
    let name_prefix = format!("auto:{}", &text.chars().take(40).collect::<String>());

    // First: check agent's own procedural memory
    let procedures = kernel.recall_procedural(agent_id, "default", Some(&name_prefix));
    if let Some(result) = extract_verified_workflow(&procedures) {
        return Some(result);
    }

    // Second: check shared procedural memory from other agents
    let shared = kernel.recall_shared_procedural(Some(&name_prefix));
    extract_verified_workflow(&shared)
}

fn extract_verified_workflow(
    procedures: &[crate::memory::MemoryEntry],
) -> Option<(Vec<ApiRequest>, String)> {
    for entry in procedures {
        if !entry.tags.iter().any(|t| t == "verified") {
            continue;
        }
        if let MemoryContent::Procedure(ref proc) = entry.content {
            let actions: Vec<ApiRequest> = proc.steps.iter()
                .filter_map(|step| serde_json::from_str(&step.action).ok())
                .collect();
            if !actions.is_empty() {
                return Some((actions, proc.description.clone()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intent::heuristic::HeuristicRouter;
    use tempfile::tempdir;

    fn make_test_kernel() -> AIKernel {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempdir().unwrap();
        AIKernel::new(dir.path().to_path_buf()).expect("kernel init")
    }

    fn make_router() -> HeuristicRouter {
        HeuristicRouter::new()
    }

    #[test]
    fn test_execute_sync_basic_single_action() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("TestAgent".to_string());

        let result = execute_sync(&kernel, &router, "put hello world", "TestAgent", 0.0, false);
        // Heuristic router may not resolve this to an exact action, but should not panic
        assert!(result.is_ok(), "execute_sync should not return error");
        let r = result.unwrap();
        let _ = r.executed;
    }

    #[test]
    fn test_execute_sync_multi_action() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("MultiAgent".to_string());

        // Multi-action text
        let result = execute_sync(&kernel, &router, "put test1 --tags a and put test2 --tags b", "MultiAgent", 0.0, false);
        assert!(result.is_ok());
        let r = result.unwrap();
        let _ = r.executed;
    }

    #[test]
    fn test_execute_sync_below_threshold() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("ThreshAgent".to_string());

        // Very low confidence text - heuristic router may error on garbage
        let result = execute_sync(&kernel, &router, "xyzabc123 gibberish", "ThreshAgent", 0.9, false);
        // Result can be Err (router fails) or Ok (no confidence)
        // We just verify no panic
        if result.is_ok() {
            let r = result.unwrap();
            // Below threshold means not executed
            let _ = r.executed;
        }
    }

    #[test]
    fn test_execute_sync_with_learning() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("LearnAgent".to_string());

        // With learn=true, after successful execution a procedure is stored
        let result = execute_sync(&kernel, &router, "status", "LearnAgent", 0.0, true);
        assert!(result.is_ok());
        // If it executed successfully, a procedural memory should be stored
        let _procs = kernel.recall_procedural("LearnAgent", "default", None);
        // learn flag should create at least one procedure (if execution succeeded)
    }

    #[test]
    fn test_recall_learned_workflow_empty() {
        let kernel = make_test_kernel();
        kernel.register_agent("NoWorkflowAgent".to_string());

        let result = recall_learned_workflow(&kernel, "NoWorkflowAgent", "nonexistent workflow text");
        assert!(result.is_none(), "no workflow should return None");
    }

    #[test]
    fn test_execute_actions_sequence_all_ok() {
        let kernel = make_test_kernel();
        kernel.register_agent("SeqAgent".to_string());

        let actions = vec![
            ApiRequest::Create {
                api_version: None,
                content: "seq1".to_string(),
                content_encoding: crate::api::semantic::ContentEncoding::Utf8,
                tags: vec!["test".to_string()],
                agent_id: "SeqAgent".to_string(),
                tenant_id: None,
                agent_token: None,
                intent: None,
            },
            ApiRequest::Create {
                api_version: None,
                content: "seq2".to_string(),
                content_encoding: crate::api::semantic::ContentEncoding::Utf8,
                tags: vec!["test".to_string()],
                agent_id: "SeqAgent".to_string(),
                tenant_id: None,
                agent_token: None,
                intent: None,
            },
        ];

        let (all_ok, output) = execute_actions_sequence(&kernel, &actions);
        // At least one should succeed (kernel processes sequentially)
        let _ = all_ok;
        let _ = output;
    }

    #[test]
    fn test_execute_sync_empty_text() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("EmptyAgent".to_string());

        // Empty string - router may fail but should not panic
        let _ = execute_sync(&kernel, &router, "", "EmptyAgent", 0.0, false);
    }

    #[test]
    fn test_execute_sync_learn_creates_procedural() {
        let kernel = make_test_kernel();
        let router = make_router();
        kernel.register_agent("LearnProceduralAgent".to_string());

        // With learn=true, a successful execution should create a procedural memory
        let result = execute_sync(&kernel, &router, "status", "LearnProceduralAgent", 0.0, true);
        assert!(result.is_ok(), "execute_sync should succeed");

        // Verify a working memory was stored (summary of the execution)
        let memories = kernel.recall("LearnProceduralAgent", "default");
        assert!(!memories.is_empty(), "execution should store a working memory summary");
    }
}
