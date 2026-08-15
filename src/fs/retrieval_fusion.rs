//! Per-role retrieval tuning weights learned by `AgentProfile`.
//!
//! The retired inline-memory fusion engine no longer consumes these values;
//! the profile keeps them as learning state for a future projection design.

use serde::{Deserialize, Serialize};

/// Tunable weights for each signal dimension. Defaults sum to 1.0.
///
/// Serializable for persistence and runtime configuration.
/// Agents can self-derive optimal weights via EMA learning (AgentProfile).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionWeights {
    pub semantic: f32,
    pub causal: f32,
    pub access: f32,
    pub tag: f32,
    pub temporal: f32,
    pub type_match: f32,
    pub lexical_keyword: f32,
}

impl Default for FusionWeights {
    fn default() -> Self {
        Self {
            semantic: 0.333,
            causal: 0.111,
            access: 0.089,
            tag: 0.111,
            temporal: 0.078,
            type_match: 0.111,
            lexical_keyword: 0.167,
        }
    }
}

impl FusionWeights {
    /// Sum of all weights (should be ~1.0 after normalization).
    pub fn total(&self) -> f32 {
        self.semantic + self.causal + self.access + self.tag + self.temporal + self.type_match + self.lexical_keyword
    }

    /// Normalize weights so they sum to 1.0, preserving ratios.
    pub fn normalize(&mut self) {
        let t = self.total();
        if t > 0.0 {
            self.semantic /= t;
            self.causal /= t;
            self.access /= t;
            self.tag /= t;
            self.temporal /= t;
            self.type_match /= t;
            self.lexical_keyword /= t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one() {
        let weights = FusionWeights::default();
        assert!((weights.total() - 1.0).abs() < 0.01);
    }

    #[test]
    fn weights_roundtrip() {
        let weights = FusionWeights::default();
        let encoded = serde_json::to_string(&weights).unwrap();
        let decoded: FusionWeights = serde_json::from_str(&encoded).unwrap();
        assert!((weights.semantic - decoded.semantic).abs() < 1e-6);
        assert!((weights.lexical_keyword - decoded.lexical_keyword).abs() < 1e-6);
    }

    #[test]
    fn normalization_preserves_ratios() {
        let mut weights = FusionWeights {
            semantic: 2.0,
            causal: 1.0,
            access: 1.0,
            tag: 1.0,
            temporal: 1.0,
            type_match: 1.0,
            lexical_keyword: 1.0,
        };
        weights.normalize();
        assert!((weights.total() - 1.0).abs() < 0.01);
        assert!(weights.semantic > weights.causal);
    }
}
