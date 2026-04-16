//! Intent Router — Natural Language → Structured API Request
//!
//! Bridges the largest "soul gap": system.md says "自然语言是主要接口" but the
//! system was purely structured JSON. This module translates free-form text
//! into one or more `ApiRequest` actions.
//!
//! # Architecture
//!
//! ```text
//! IntentRouter (trait)
//! ├── HeuristicRouter  — keyword/pattern matching (always available)
//! ├── LlmRouter        — LLM-powered resolution (requires Ollama)
//! └── ChainRouter      — tries Heuristic first, falls back to LLM
//! ```

pub mod heuristic;
pub mod llm;

use serde::{Deserialize, Serialize};
use crate::api::semantic::ApiRequest;

/// A resolved intent — a structured action derived from natural language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedIntent {
    pub confidence: f32,
    pub action: ApiRequest,
    pub explanation: String,
}

/// Errors from intent resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentError {
    Ambiguous(Vec<ResolvedIntent>),
    Unresolvable(String),
    LlmUnavailable(String),
}

impl std::fmt::Display for IntentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ambiguous(intents) => write!(f, "Ambiguous: {} candidates", intents.len()),
            Self::Unresolvable(msg) => write!(f, "Unresolvable: {}", msg),
            Self::LlmUnavailable(msg) => write!(f, "LLM unavailable: {}", msg),
        }
    }
}

impl std::error::Error for IntentError {}

/// Trait for NL → ApiRequest translation.
pub trait IntentRouter: Send + Sync {
    fn resolve(&self, text: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError>;
}

/// Chain router — tries heuristic first, falls back to LLM if confidence is low.
pub struct ChainRouter {
    heuristic: heuristic::HeuristicRouter,
    llm: Option<llm::LlmRouter>,
    confidence_threshold: f32,
}

impl ChainRouter {
    pub fn new(llm: Option<llm::LlmRouter>) -> Self {
        Self {
            heuristic: heuristic::HeuristicRouter::new(),
            llm,
            confidence_threshold: 0.7,
        }
    }

    pub fn with_threshold(mut self, threshold: f32) -> Self {
        self.confidence_threshold = threshold;
        self
    }
}

impl IntentRouter for ChainRouter {
    fn resolve(&self, text: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError> {
        let heuristic_results = self.heuristic.resolve(text, agent_id);

        match heuristic_results {
            Ok(ref results) if !results.is_empty()
                && results[0].confidence >= self.confidence_threshold =>
            {
                heuristic_results
            }
            _ => {
                if let Some(ref llm) = self.llm {
                    match llm.resolve(text, agent_id) {
                        Ok(results) if !results.is_empty() => Ok(results),
                        Ok(_) => heuristic_results,
                        Err(IntentError::LlmUnavailable(_)) => {
                            // LLM failed, return heuristic results even if low confidence
                            match heuristic_results {
                                Ok(r) if !r.is_empty() => Ok(r),
                                _ => Err(IntentError::Unresolvable(
                                    format!("Could not resolve: '{}'", text),
                                )),
                            }
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    match heuristic_results {
                        Ok(r) if !r.is_empty() => Ok(r),
                        _ => Err(IntentError::Unresolvable(
                            format!("Could not resolve: '{}' (no LLM available)", text),
                        )),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_router_uses_heuristic_when_confident() {
        let router = ChainRouter::new(None);
        let results = router.resolve("search for agent scheduling documents", "test-agent");
        assert!(results.is_ok());
        let results = results.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].confidence >= 0.7);
    }

    #[test]
    fn chain_router_returns_low_confidence_for_gibberish() {
        let router = ChainRouter::new(None);
        let results = router.resolve("xyzzy plugh", "test-agent");
        assert!(results.is_ok());
        let results = results.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].confidence < 0.5, "gibberish should have low confidence");
    }

    #[test]
    fn resolved_intent_serializable() {
        let ri = ResolvedIntent {
            confidence: 0.9,
            action: crate::api::semantic::ApiRequest::ListAgents,
            explanation: "test".into(),
        };
        let json = serde_json::to_string(&ri).unwrap();
        assert!(json.contains("0.9"));
    }
}
