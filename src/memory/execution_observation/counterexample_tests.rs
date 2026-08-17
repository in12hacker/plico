use std::num::NonZeroU32;

use super::error::{
    CorruptionCategory, InvalidRequestCategory, LimitCategory, ObservationStoreError, TransitionConflictCategory,
};
use super::hash;
use super::hash::tests::{flow, golden_started_request, golden_terminal_request, hex64, uuid, STARTED_EVENT_SHA};
use super::ids::ExecutionAttemptKeyV1;
use super::model::{FixtureCurrentViewV1, ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, STARTED_REQUEST_SCHEMA};
use super::tests::{attempt_view, golden_chain};
use super::validation::{validate_started_transition, validate_terminal_transition, EVIDENCE_ITEMS_PER_LIST_MAX};
use super::{ATTEMPTS_MAX, EVENTS_MAX};

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

#[test]
fn execution_observation_counterexample_capacity_and_ordinal_caps() {
    let event_limit = || ObservationStoreError::limit(LimitCategory::Event);
    // attempts: 10,000 accepted, 10,001 rejected (byte cap does not bind)
    sized_view(ATTEMPTS_MAX).validate().expect("10,000 attempts accepted");
    assert_eq!(
        sized_view(ATTEMPTS_MAX + 1).validate(),
        Err(ObservationStoreError::limit(LimitCategory::Attempt))
    );
    // watermark and generation: 20,001 rejected on view and root
    let mut view = sized_view(1);
    view.event_watermark = EVENTS_MAX + 1;
    assert_eq!(view.validate(), Err(event_limit()));
    view.event_watermark = EVENTS_MAX;
    view.generation = EVENTS_MAX + 1;
    assert_eq!(view.validate(), Err(event_limit()));
    view.generation = EVENTS_MAX;
    view.validate().expect("view 20,000/20,000 accepted");
    let chain = golden_chain();
    let mut root = chain.terminal_root;
    let expected_root_view = root.current_view_sha256.clone();
    root.event_watermark = EVENTS_MAX + 1;
    assert_eq!(root.validate(&expected_root_view), Err(event_limit()));
    root.event_watermark = EVENTS_MAX;
    root.generation = EVENTS_MAX + 1;
    assert_eq!(root.validate(&expected_root_view), Err(event_limit()));
    root.generation = EVENTS_MAX;
    root.validate(&expected_root_view).expect("root 20,000/20,000 accepted");
    // stored events: sequence and root_generation each capped at 20,000
    let mut started_event = chain.started_event;
    started_event.sequence = EVENTS_MAX + 1;
    assert_eq!(started_event.validate(), Err(event_limit()));
    started_event.sequence = EVENTS_MAX;
    started_event.root_generation = EVENTS_MAX + 1;
    assert_eq!(started_event.validate(), Err(event_limit()));
    started_event.root_generation = EVENTS_MAX;
    started_event.validate().expect("started event 20,000/20,000 accepted");
    let mut terminal_event = chain.terminal_event;
    terminal_event.sequence = EVENTS_MAX + 1;
    assert_eq!(terminal_event.validate(), Err(event_limit()));
    terminal_event.sequence = EVENTS_MAX;
    terminal_event.root_generation = EVENTS_MAX + 1;
    assert_eq!(terminal_event.validate(), Err(event_limit()));
    terminal_event.root_generation = EVENTS_MAX;
    terminal_event
        .validate()
        .expect("terminal event 20,000/20,000 accepted");
    // segment: first and last sequence capped; last != first is corrupt
    let mut segment = chain.started_segment;
    segment.first_sequence = EVENTS_MAX + 1;
    segment.last_sequence = EVENTS_MAX + 1;
    assert_eq!(segment.validate(STARTED_EVENT_SHA), Err(event_limit()));
    segment.first_sequence = EVENTS_MAX;
    segment.last_sequence = EVENTS_MAX;
    segment.validate(STARTED_EVENT_SHA).expect("segment 20,000 accepted");
    segment.last_sequence = EVENTS_MAX + 1;
    assert_eq!(
        segment.validate(STARTED_EVENT_SHA),
        Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
    );
    flow(
        "counterexample caps: attempts 10k/10k+1; ordinal+generation+watermark 20,001 -> event_limit; 20,000 accepted",
    );
}

/// Valid ascending view with `count` attempts (~3.6 MiB at the cap).
fn sized_view(count: usize) -> FixtureCurrentViewV1 {
    let attempts = (1..=count)
        .map(|index| {
            let mut view = attempt_view(false);
            view.key.execution_id = uuid(&format!("123e4567-e89b-42d3-a456-{index:012x}"));
            view
        })
        .collect::<Vec<_>>();
    FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.into(),
        attestation_state: ATTESTATION_STATE.into(),
        generation: count as u64,
        event_watermark: count as u64,
        attempts,
    }
}
