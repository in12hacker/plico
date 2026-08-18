//! Writer clock (ADR-0010 §5).
//!
//! The only time field is the event/root `recorded_at_ms`:
//! `max(system_now_ms, previous_accepted_recorded_at_ms)`, never silently
//! truncated — a value beyond the JSON-safe integer ceiling is a typed
//! limit rejection. Same-millisecond monotonicity comes from
//! sequence/generation, clock rollback is absorbed by the max, and
//! idempotent hits never reach this module at all.

use std::time::{SystemTime, UNIX_EPOCH};

use super::super::error::{LimitCategory, ObservationStoreError};

/// 2^53 - 1: the JSON-safe integer ceiling the contract pins for stamps.
pub(crate) const JSON_SAFE_MAX_MS: u64 = 9_007_199_254_740_991;

/// Wall-clock milliseconds since the Unix epoch; a pre-epoch clock reads 0.
pub(super) fn system_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// The frozen clock rule. The facade applies it after the idempotency
/// decision and before any object is constructed; callers never supply
/// timestamps.
pub(super) fn advance(system_now_ms: u64, previous_accepted_ms: u64) -> Result<u64, ObservationStoreError> {
    let value = system_now_ms.max(previous_accepted_ms);
    if value > JSON_SAFE_MAX_MS {
        return Err(ObservationStoreError::limit(LimitCategory::Event));
    }
    Ok(value)
}
