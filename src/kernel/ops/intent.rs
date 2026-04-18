//! Intent resolution and execution operations.

use crate::intent::ResolvedIntent;
use crate::scheduler::IntentPriority;

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
}
