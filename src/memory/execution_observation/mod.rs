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
pub(crate) mod validation;

#[cfg(test)]
mod tests;

use error::{LimitCategory, ObservationStoreError};

// Frozen pure limits (ADR-0007 §5; r0_spec.json `limits`). Byte caps apply to
// canonical bytes; count caps are pure functions for the WP2 store, which must
// reject oversized input before writing any immutable object.
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

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn execution_observation_limits_attempt_and_event_counts() {
        validate_attempt_count(ATTEMPTS_MAX).expect("boundary attempts");
        validate_event_count(EVENTS_MAX).expect("boundary events");
        assert_eq!(
            validate_attempt_count(ATTEMPTS_MAX + 1),
            Err(ObservationStoreError::limit(LimitCategory::Attempt))
        );
        assert_eq!(
            validate_event_count(EVENTS_MAX + 1),
            Err(ObservationStoreError::limit(LimitCategory::Event))
        );
    }
}

/// Adversarial counterexamples from the independent WP1.1 review: each case
/// kills a specific implementation mutant (three-way key operands, the
/// view↔started digest binding, the transition-internal evidence total, the
/// attempt component of the key, and malformed bodies fed to transitions).
#[cfg(test)]
mod counterexample_tests {
    use std::num::NonZeroU32;

    use super::error::{
        CorruptionCategory, InvalidRequestCategory, LimitCategory, ObservationStoreError, TransitionConflictCategory,
    };
    use super::hash;
    use super::hash::tests::{flow, golden_started_request, golden_terminal_request, hex64, uuid};
    use super::ids::ExecutionAttemptKeyV1;
    use super::model::STARTED_REQUEST_SCHEMA;
    use super::tests::attempt_view;
    use super::validation::{validate_started_transition, validate_terminal_transition, EVIDENCE_ITEMS_PER_LIST_MAX};

    fn unique_cids(from: u64, count: usize) -> Vec<String> {
        (from..from + count as u64)
            .map(|value| format!("{value:064x}"))
            .collect()
    }

    fn other_execution_key(attempt: u32) -> ExecutionAttemptKeyV1 {
        ExecutionAttemptKeyV1 {
            execution_id: uuid("123e4567-e89b-42d3-a456-426614174099"),
            attempt: NonZeroU32::new(attempt).expect("nonzero"),
        }
    }

    #[test]
    fn execution_observation_counterexample_three_way_key_binding() {
        let request = golden_terminal_request();
        let view = attempt_view(false);
        let mut started = golden_started_request();
        started.key = other_execution_key(3);
        super::validation::validate_started_request(&started).expect("started request itself is valid");
        assert_eq!(
            validate_terminal_transition(&request, Some(&view), Some(&started)),
            Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
        );

        let mut other_view = attempt_view(false);
        other_view.key = other_execution_key(4);
        assert_eq!(
            validate_terminal_transition(&request, Some(&other_view), Some(&golden_started_request())),
            Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
        );

        assert_eq!(
            validate_terminal_transition(&request, Some(&view), None),
            Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
        );

        let mut runtime_rebound = golden_terminal_request();
        runtime_rebound.runtime_sha256 = hex64('e');
        assert_eq!(
            validate_terminal_transition(
                &runtime_rebound,
                Some(&attempt_view(true)),
                Some(&golden_started_request())
            ),
            Err(ObservationStoreError::conflict(
                TransitionConflictCategory::TerminalRuntimeRebind
            ))
        );
        flow("counterexample three-way key operands + missing bound_started + runtime rebind pre-idempotency");
    }

    #[test]
    fn execution_observation_counterexample_view_started_hash_binding() {
        let mut view = attempt_view(false);
        view.started_request_sha256 = hex64('f');
        assert_eq!(
            validate_terminal_transition(&golden_terminal_request(), Some(&view), Some(&golden_started_request())),
            Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch))
        );
        flow("counterexample view/started digest binding mismatch -> corrupt_store/object_hash_mismatch");
    }

    #[test]
    fn execution_observation_counterexample_transition_evidence_total() {
        let mut started = golden_started_request();
        started.input_evidence_cids = unique_cids(1, EVIDENCE_ITEMS_PER_LIST_MAX);
        started.context_evidence_cids = unique_cids(257, EVIDENCE_ITEMS_PER_LIST_MAX);
        let mut view = attempt_view(false);
        view.started_request_sha256 = hash::started_request_sha256(&started).expect("hash");
        assert_eq!(
            validate_terminal_transition(&golden_terminal_request(), Some(&view), Some(&started)),
            Err(ObservationStoreError::limit(LimitCategory::EvidenceTotal))
        );
        flow("counterexample transition-internal evidence total 256+256+1 -> limit/evidence_total_limit");
    }

    #[test]
    fn execution_observation_counterexample_same_execution_different_attempt() {
        let mut started = golden_started_request();
        started.key.attempt = NonZeroU32::new(2).expect("nonzero");
        assert_eq!(
            validate_started_transition(&started, Some(&attempt_view(false))),
            Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
        );

        let mut malformed = golden_started_request();
        malformed.schema = format!("{STARTED_REQUEST_SCHEMA}-v2");
        assert_eq!(
            validate_started_transition(&malformed, None),
            Err(ObservationStoreError::invalid(
                InvalidRequestCategory::UnsupportedSchema
            ))
        );
        flow("counterexample attempt-component isolation + malformed body rejected inside transition");
    }
}
