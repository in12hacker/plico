//! TemporalResolver — trait + implementations
//!
//! The trait is intentionally small so it can be implemented for testing
//! (stub), for fast heuristic-only resolution, or for LLM-powered resolution.

use crate::temporal::rules::Granularity;

/// A resolved time range with confidence metadata.
#[derive(Debug, Clone)]
pub struct TemporalRange {
    /// Lower bound (inclusive), Unix milliseconds.
    pub since: i64,
    /// Upper bound (inclusive), Unix milliseconds.
    pub until: i64,
    /// Confidence in the resolution [0, 1].
    pub confidence: f32,
    /// Resolved granularity.
    pub granularity: Granularity,
    /// Original expression (echoed back).
    pub expression: String,
}

impl TemporalRange {
    /// Expand the range symmetrically by `days` in both directions.
    /// Used for medium-confidence resolutions.
    pub fn expanded(&self, days: i64) -> Self {
        let day_ms = days * 86_400_000;
        Self {
            since: self.since.saturating_sub(day_ms),
            until: self.until.saturating_add(day_ms),
            confidence: self.confidence,
            granularity: Granularity::Fuzzy,
            expression: self.expression.clone(),
        }
    }
}

/// Resolves natural-language time expressions to concrete time ranges.
///
/// Implementations range from fast rule-based (no LLM) to full LLM-powered
/// (handles novel expressions the rules don't cover).
pub trait TemporalResolver: Send + Sync {
    /// Resolve a time expression.
    ///
    /// `reference_ms` — Unix milliseconds of the reference date (default: now).
    /// Returns `None` if resolution fails (expression not understood).
    fn resolve(&self, expression: &str, reference_ms: Option<i64>) -> Option<TemporalRange>;
}

// ─── Stub resolver (for testing / stub embedding) ───────────────────────────

/// Stub resolver that always returns `None` — forces pure semantic search.
pub struct StubTemporalResolver;

impl TemporalResolver for StubTemporalResolver {
    fn resolve(&self, _: &str, _: Option<i64>) -> Option<TemporalRange> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_temporal_range_expanded() {
        let range = TemporalRange {
            since: 1000,
            until: 2000,
            confidence: 0.8,
            granularity: crate::temporal::rules::Granularity::ExactDay,
            expression: "test".to_string(),
        };
        let expanded = range.expanded(1);
        // Should expand by 1 day in both directions
        assert!(expanded.since < range.since);
        assert!(expanded.until > range.until);
        assert_eq!(expanded.confidence, range.confidence);
        assert_eq!(expanded.expression, range.expression);
    }

    #[test]
    fn test_temporal_range_expanded_preserves_granularity() {
        let range = TemporalRange {
            since: 1000,
            until: 2000,
            confidence: 0.5,
            granularity: crate::temporal::rules::Granularity::Week,
            expression: "test".to_string(),
        };
        let expanded = range.expanded(3);
        // Expanded range becomes Fuzzy
        assert_eq!(expanded.granularity, crate::temporal::rules::Granularity::Fuzzy);
    }

    // ─── Stub Temporal Resolver ─────────────────────────────────────────────

    #[test]
    fn test_stub_resolver_always_returns_none() {
        let stub = StubTemporalResolver;
        let result = stub.resolve("last week", None);
        assert!(result.is_none(), "stub resolver should always return None");
    }

    #[test]
    fn test_stub_resolver_with_reference_time() {
        let stub = StubTemporalResolver;
        let result = stub.resolve("yesterday", Some(1700000000000));
        assert!(result.is_none(), "stub resolver ignores reference time");
    }

    #[test]
    fn test_stub_resolver_empty_expression() {
        let stub = StubTemporalResolver;
        let result = stub.resolve("", None);
        assert!(result.is_none(), "stub resolver handles empty expression");
    }
}
