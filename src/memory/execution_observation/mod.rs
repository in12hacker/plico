//! Execution Observation Ledger v1 — Phase 1A pure type and validation core
//! (ADR-0007, milestone v53 WP1).
//!
//! Crate-private, in-memory only: typed identifiers, fixed wire schemas,
//! RFC 8785/JCS canonicalization, domain-separated SHA-256, and strict
//! field/boundary/transition validators. No I/O, no CAS namespace, no store,
//! no kernel/scheduler wiring, no public API surface.

#![allow(dead_code)] // WP1 has no production caller by contract; wiring starts at the WP2 store (ADR-0007 §10)

pub(crate) mod canonical;
pub(crate) mod error;
pub(crate) mod hash;
pub(crate) mod ids;
pub(crate) mod model;
mod store;
pub(crate) mod validation;

#[cfg(test)]
mod tests;

use error::{LimitCategory, ObservationStoreError};

// Frozen pure limits (ADR-0007 §5; r0_spec.json `limits`). Byte caps and the
// attempt/event count caps are enforced by the stored-object validators and by
// these pure functions; the count call convention (existing count vs count
// including a new request) is frozen by the WP2 store writer.
pub(crate) const ATTEMPTS_MAX: usize = 10_000;
pub(crate) const EVENTS_MAX: u64 = 20_000;
pub(crate) const POINTER_MAX_BYTES: usize = 4_096;
pub(crate) const ROOT_MAX_BYTES: usize = 65_536;
pub(crate) const SEGMENT_MAX_BYTES: usize = 65_536;
pub(crate) const CURRENT_VIEW_MAX_BYTES: usize = 8 * 1_024 * 1_024;

/// Ledger capacity: at most 10,000 attempts; the cap rejects new Started while
/// existing Open attempts may still take a Terminal (ADR-0007 §5).
pub(crate) fn validate_attempt_count(count: usize) -> Result<(), ObservationStoreError> {
    if count > ATTEMPTS_MAX {
        Err(ObservationStoreError::limit(LimitCategory::Attempt))
    } else {
        Ok(())
    }
}

/// Ledger capacity: at most 20,000 stored events (ADR-0007 §5).
pub(crate) fn validate_event_count(count: u64) -> Result<(), ObservationStoreError> {
    if count > EVENTS_MAX {
        Err(ObservationStoreError::limit(LimitCategory::Event))
    } else {
        Ok(())
    }
}

/// Adversarial counterexamples; each kills a specific implementation mutant
/// (key operands, digest bindings, evidence totals, caps, malformed bodies).
#[cfg(test)]
mod counterexample_tests;

/// Field-level typed rejects (F13): frozen categories, never a panic.
#[cfg(test)]
mod field_reject_tests;
