//! Shared deterministic fixtures for execution-observation tests.

use super::super::error::{ObservationStoreError, TransitionConflictCategory};
use super::super::hash;
use super::super::hash::tests::{
    golden_key, golden_started_request, golden_terminal_request, GENESIS_ROOT_SHA, STARTED_EVENT_SHA,
    STARTED_RECORDED_AT_MS, STARTED_REQUEST_SHA, STARTED_ROOT_SHA, STARTED_SEGMENT_SHA, TERMINAL_EVENT_SHA,
    TERMINAL_RECORDED_AT_MS, TERMINAL_REQUEST_SHA, TERMINAL_SEGMENT_SHA,
};
use super::super::ids::EventKind;
use super::super::model::*;
use super::GoldenChain;

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

pub(crate) fn err(category: super::super::error::InvalidRequestCategory) -> ObservationStoreError {
    ObservationStoreError::invalid(category)
}
pub(crate) fn conflict(category: TransitionConflictCategory) -> ObservationStoreError {
    ObservationStoreError::conflict(category)
}
