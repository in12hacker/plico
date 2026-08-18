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
