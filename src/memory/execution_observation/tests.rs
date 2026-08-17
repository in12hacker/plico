//! WP1 self-tests: F-matrix F10/F13 strict rejects, pure transitions, and key
//! counterexamples (field-level F13 and stale-hash counterexamples live in
//! `canonical.rs`; limits/boundaries in `validation.rs`; golden digests in
//! `hash.rs`).

use super::canonical::{parse_canonical, to_canonical_vec};
use super::error::InvalidRequestCategory::{DuplicateCid, InvalidCid, JcsCanonicalizationFailed};
use super::error::{ObservationStoreError, TransitionConflictCategory};
use super::hash;
use super::hash::tests::{
    flow, golden_key, golden_started_request, golden_terminal_request, hex64, uuid, GENESIS_ROOT_SHA,
    ORIGIN_REQUEST_ID, STARTED_EVENT_SHA, STARTED_RECORDED_AT_MS, STARTED_REQUEST_SHA, STARTED_ROOT_SHA,
    STARTED_SEGMENT_SHA, TERMINAL_EVENT_SHA, TERMINAL_RECORDED_AT_MS, TERMINAL_REQUEST_SHA, TERMINAL_SEGMENT_SHA,
};
use super::ids::{EventKind, ExecutionAttemptKeyV1, FixtureOriginV1, TerminalOutcomeV1};
use super::model::*;
use super::validation::*;

pub(crate) fn attempt_view(terminal: bool) -> FixtureAttemptViewV1 {
    FixtureAttemptViewV1 {
        key: golden_key(),
        attestation_state: ATTESTATION_STATE.into(),
        started_request_sha256: STARTED_REQUEST_SHA.into(),
        started_event_sha256: STARTED_EVENT_SHA.into(),
        terminal_request_sha256: terminal.then(|| TERMINAL_REQUEST_SHA.to_string()),
        terminal_event_sha256: terminal.then(|| TERMINAL_EVENT_SHA.to_string()),
    }
}

fn current_view(generation: u64, attempt: FixtureAttemptViewV1) -> FixtureCurrentViewV1 {
    FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.into(),
        attestation_state: ATTESTATION_STATE.into(),
        generation,
        event_watermark: generation,
        attempts: vec![attempt],
    }
}

fn root(
    generation: u64,
    previous_root_sha256: Option<&str>,
    event_segment_head_sha256: Option<&str>,
    current_view_sha256: String,
    committed_at_ms: u64,
) -> FixtureLedgerRootV1 {
    FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.into(),
        trust_class: TRUST_CLASS.into(),
        generation,
        previous_root_sha256: previous_root_sha256.map(str::to_string),
        event_segment_head_sha256: event_segment_head_sha256.map(str::to_string),
        event_watermark: generation,
        current_view_sha256,
        committed_at_ms,
    }
}

fn segment(sequence: u64, previous: Option<&str>, kind: EventKind, event_sha256: &str) -> FixtureEventSegmentV1 {
    FixtureEventSegmentV1 {
        schema: SEGMENT_SCHEMA.into(),
        first_sequence: sequence,
        last_sequence: sequence,
        previous_segment_sha256: previous.map(str::to_string),
        event_kind: kind,
        event_sha256: event_sha256.to_string(),
    }
}

pub(crate) struct GoldenChain {
    pub(crate) started_event: StoredStartedEventV1,
    pub(crate) started_segment: FixtureEventSegmentV1,
    pub(crate) open_view: FixtureCurrentViewV1,
    pub(crate) started_root: FixtureLedgerRootV1,
    pub(crate) terminal_event: StoredTerminalEventV1,
    pub(crate) terminal_segment: FixtureEventSegmentV1,
    pub(crate) terminal_view: FixtureCurrentViewV1,
    pub(crate) terminal_root: FixtureLedgerRootV1,
}

pub(crate) fn golden_chain() -> GoldenChain {
    let started_event = StoredStartedEventV1 {
        schema: STARTED_EVENT_SCHEMA.into(),
        request: golden_started_request(),
        request_sha256: STARTED_REQUEST_SHA.into(),
        sequence: 1,
        root_generation: 1,
        recorded_at_ms: STARTED_RECORDED_AT_MS,
    };
    let started_segment = segment(1, None, EventKind::Started, STARTED_EVENT_SHA);
    let open_view = current_view(1, attempt_view(false));
    let started_root = root(
        1,
        Some(GENESIS_ROOT_SHA),
        Some(STARTED_SEGMENT_SHA),
        hash::current_view_sha256(&open_view).expect("hash"),
        STARTED_RECORDED_AT_MS,
    );
    let terminal_event = StoredTerminalEventV1 {
        schema: TERMINAL_EVENT_SCHEMA.into(),
        request: golden_terminal_request(),
        request_sha256: TERMINAL_REQUEST_SHA.into(),
        sequence: 2,
        root_generation: 2,
        recorded_at_ms: TERMINAL_RECORDED_AT_MS,
    };
    let terminal_segment = segment(2, Some(STARTED_SEGMENT_SHA), EventKind::Terminal, TERMINAL_EVENT_SHA);
    let terminal_view = current_view(2, attempt_view(true));
    let terminal_root = root(
        2,
        Some(STARTED_ROOT_SHA),
        Some(TERMINAL_SEGMENT_SHA),
        hash::current_view_sha256(&terminal_view).expect("hash"),
        TERMINAL_RECORDED_AT_MS,
    );
    GoldenChain {
        started_event,
        started_segment,
        open_view,
        started_root,
        terminal_event,
        terminal_segment,
        terminal_view,
        terminal_root,
    }
}

pub(crate) fn err(category: super::error::InvalidRequestCategory) -> ObservationStoreError {
    ObservationStoreError::invalid(category)
}
pub(crate) fn conflict(category: TransitionConflictCategory) -> ObservationStoreError {
    ObservationStoreError::conflict(category)
}

#[test]
fn execution_observation_f10_malformed_and_inline_cid_rejected() {
    let mut request = golden_started_request();
    validate_started_request(&request).expect("golden request is valid");
    let malformed = [
        String::new(),
        "abc".to_string(),
        "0".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
        "inline bytes are not cid references".to_string(),
        format!("{}\n", "0".repeat(63)),
    ];
    for (index, cid) in malformed.into_iter().enumerate() {
        request.input_evidence_cids = vec![cid];
        assert_eq!(validate_started_request(&request), Err(err(InvalidCid)));
        flow(format!("logic.f10 cid-case={index} -> invalid_request/invalid_cid"));
    }
    request = golden_started_request();
    request.input_evidence_cids = vec![hex64('0'), hex64('0')];
    assert_eq!(validate_started_request(&request), Err(err(DuplicateCid)));
    flow("logic.f10 duplicate-within-list -> invalid_request/duplicate_cid");
    request.input_evidence_cids = vec![hex64('0')];
    request.context_evidence_cids = vec![hex64('0')];
    validate_started_request(&request).expect("same cid across lists is allowed");
    flow("logic.f10 same-cid-across-lists -> ok");
}

#[test]
fn execution_observation_f13_wire_level_strict_rejects() {
    let canonical = to_canonical_vec(&golden_started_request()).unwrap();
    let declaration_order = serde_json::to_vec(&golden_started_request()).unwrap();
    assert_ne!(declaration_order, canonical);
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(&declaration_order),
        Err(err(JcsCanonicalizationFailed))
    );
    let text = std::str::from_utf8(&canonical).expect("ascii");
    let unknown_field = format!("{{\"zz\":0,{}", &text[1..]);
    assert!(parse_canonical::<AppendStartedRequestV1>(unknown_field.as_bytes()).is_err());
    let whitespace = format!(" {text}");
    assert!(parse_canonical::<AppendStartedRequestV1>(whitespace.as_bytes()).is_err());
    let escaped_unicode = text.replacen("\"policy_sha256\":\"bbbb", "\"policy_sha256\":\"\\u0062bbb", 1);
    assert!(parse_canonical::<AppendStartedRequestV1>(escaped_unicode.as_bytes()).is_err());
    flow("logic.f13 wire rejects declaration-order|unknown-field|whitespace|escaped-unicode -> jcs_canonicalization_failed");
}

#[test]
fn execution_observation_transition_state_machine() {
    let chain = golden_chain();
    let open_view = &chain.open_view.attempts[0];
    let terminal_view = &chain.terminal_view.attempts[0];
    let started = golden_started_request();
    let terminal = golden_terminal_request();

    validate_started_transition(&started, None).expect("absent accepts started");
    validate_started_transition(&started, Some(open_view)).expect("same started is idempotent");
    validate_started_transition(&started, Some(terminal_view)).expect("same started is idempotent");
    flow("logic.transition absent+started -> ok; open+same-started -> ok-idempotent; terminal+same -> ok-idempotent");

    let mut rebound = started.clone();
    rebound.input_evidence_cids = vec![hex64('7')];
    assert_eq!(
        validate_started_transition(&rebound, Some(open_view)),
        Err(conflict(TransitionConflictCategory::StartedAlreadyBound))
    );
    let mut origin_rebound = started.clone();
    let origin_id = uuid(ORIGIN_REQUEST_ID);
    origin_rebound.fixture_origin = FixtureOriginV1::IntentDispatch { intent_id: origin_id };
    assert_eq!(
        validate_started_transition(&origin_rebound, Some(terminal_view)),
        Err(conflict(TransitionConflictCategory::StartedAlreadyBound))
    );
    flow("logic.transition open/terminal + different-started (evidence|origin rebind) -> started_already_bound");

    assert_eq!(
        validate_terminal_transition(&terminal, None, None),
        Err(conflict(TransitionConflictCategory::TerminalWithoutStarted))
    );
    validate_terminal_transition(&terminal, Some(open_view), Some(&started))
        .expect("open accepts first terminal with matching policy/runtime");
    validate_terminal_transition(&terminal, Some(terminal_view), Some(&started)).expect("same terminal is idempotent");
    flow("logic.transition absent+terminal -> terminal_without_started; open+first-terminal -> ok; terminal+same -> ok-idempotent");

    let mut policy_rebind = terminal.clone();
    policy_rebind.policy_sha256 = hex64('d');
    assert_eq!(
        validate_terminal_transition(&policy_rebind, Some(open_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalPolicyRebind))
    );
    let mut runtime_rebind = terminal.clone();
    runtime_rebind.runtime_sha256 = hex64('e');
    assert_eq!(
        validate_terminal_transition(&runtime_rebind, Some(open_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalRuntimeRebind))
    );
    let mut second_terminal = terminal.clone();
    second_terminal.outcome = TerminalOutcomeV1::Success;
    assert_eq!(
        validate_terminal_transition(&second_terminal, Some(terminal_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalAlreadyBound))
    );
    flow("logic.transition open+policy/runtime-mismatch -> rebind conflicts; terminal+different-terminal -> terminal_already_bound");
}

#[test]
fn execution_observation_counterexample_terminal_cross_attempt_key() {
    use std::num::NonZeroU32;

    use super::error::CorruptionCategory;

    let view = attempt_view(false);
    let started = golden_started_request();
    let mut terminal = golden_terminal_request();
    terminal.key = ExecutionAttemptKeyV1 {
        execution_id: uuid("123e4567-e89b-42d3-a456-426614174099"),
        attempt: NonZeroU32::new(2).expect("nonzero"),
    };
    validate_terminal_request(&terminal).expect("request itself is valid");
    assert_eq!(
        validate_terminal_transition(&terminal, Some(&view), Some(&started)),
        Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
    );
    flow("counterexample terminal cross-attempt key -> corrupt_store/invalid_transition");
}

#[test]
fn execution_observation_counterexample_started_retry_unrelated_view() {
    use std::num::NonZeroU32;

    use super::error::CorruptionCategory;

    let view = attempt_view(false);
    let mut started = golden_started_request();
    started.key = ExecutionAttemptKeyV1 {
        execution_id: uuid("123e4567-e89b-42d3-a456-426614174099"),
        attempt: NonZeroU32::new(7).expect("nonzero"),
    };
    validate_started_request(&started).expect("request itself is valid");
    assert_eq!(
        validate_started_transition(&started, Some(&view)),
        Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
    );
    flow("counterexample started retry vs unrelated view -> corrupt_store/invalid_transition");
}
