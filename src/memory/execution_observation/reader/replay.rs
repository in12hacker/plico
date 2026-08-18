//! Independent replay of the authoritative active chain (ADR-0009).
//!
//! Walks the sealed capability's pointer/root/segment/event objects from the
//! active root down to the recomputed exact genesis, re-derives every hash
//! binding, feeds the strictly-ordered events through the single reducer, and
//! then compares the stored current view against the reducer's result
//! field-by-field. The stored view is never trusted on its own.

use crate::cas::ExistingExecutionObservationReadOnly;

use super::super::canonical::parse_canonical;
use super::super::error::{CorruptionCategory, InvalidRequestCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::model::*;
use super::super::validation::{is_lowercase_hex64, CANONICAL_REQUEST_MAX_BYTES};
use super::super::{CURRENT_VIEW_MAX_BYTES, EVENTS_MAX, POINTER_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::reducer::{reduce, ReducibleAttemptV1, ReducibleEventV1, ReducibleKindV1};

const STORED_EVENT_MAX_BYTES: usize = CANONICAL_REQUEST_MAX_BYTES + 4_096;

fn corrupt(category: CorruptionCategory) -> ObservationStoreError {
    ObservationStoreError::corrupt(category)
}

fn map_stored(error: ObservationStoreError) -> ObservationStoreError {
    match error {
        ObservationStoreError::InvalidRequest { category } => corrupt(match category {
            InvalidRequestCategory::UnsupportedSchema | InvalidRequestCategory::InvalidAttestation => {
                CorruptionCategory::UnsupportedStoredSchema
            }
            _ => CorruptionCategory::ObjectHashMismatch,
        }),
        ObservationStoreError::LimitExceeded { .. } => corrupt(CorruptionCategory::StoredResourceLimit),
        ObservationStoreError::TransitionConflict { .. } => corrupt(CorruptionCategory::InvalidTransition),
        stored => stored,
    }
}

fn read_object(
    view: &ExistingExecutionObservationReadOnly,
    sha256: &str,
    maximum_bytes: usize,
    missing: CorruptionCategory,
) -> Result<Vec<u8>, ObservationStoreError> {
    match view.get_immutable_bounded(sha256, maximum_bytes as u64) {
        Ok(bytes) => Ok(bytes),
        Err(error) => Err(match error.kind() {
            std::io::ErrorKind::NotFound => corrupt(missing),
            std::io::ErrorKind::InvalidData => corrupt(CorruptionCategory::StoredResourceLimit),
            _ => ObservationStoreError::StorageUnavailable,
        }),
    }
}

/// Loads and name-verifies one root object.
fn load_root(
    view: &ExistingExecutionObservationReadOnly,
    sha256: &str,
) -> Result<FixtureLedgerRootV1, ObservationStoreError> {
    let bytes = read_object(view, sha256, ROOT_MAX_BYTES, CorruptionCategory::BrokenRootChain)?;
    let root = parse_canonical::<FixtureLedgerRootV1>(&bytes).map_err(map_stored)?;
    if hash::root_sha256(&root).map_err(map_stored)? != sha256 {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let view_sha256 = root.current_view_sha256.clone();
    root.validate(&view_sha256).map_err(map_stored)?;
    Ok(root)
}

/// Byte-tag kind probe before the typed event parse.
fn parse_event(bytes: &[u8]) -> Result<FixtureStoredEvent, ObservationStoreError> {
    let has_tag = |tag: &[u8]| bytes.windows(tag.len()).any(|window| window == tag);
    let started = has_tag(br#""plico.execution-observation.fixture-started/v1""#);
    let terminal = has_tag(br#""plico.execution-observation.fixture-terminal/v1""#);
    match (started, terminal) {
        (true, false) => Ok(FixtureStoredEvent::Started(
            parse_canonical::<StoredStartedEventV1>(bytes).map_err(map_stored)?,
        )),
        (false, true) => Ok(FixtureStoredEvent::Terminal(
            parse_canonical::<StoredTerminalEventV1>(bytes).map_err(map_stored)?,
        )),
        _ => Err(corrupt(CorruptionCategory::UnsupportedStoredSchema)),
    }
}

enum FixtureStoredEvent {
    Started(StoredStartedEventV1),
    Terminal(StoredTerminalEventV1),
}

/// Replays the authoritative chain and returns reducer-built attempts in
/// canonical key order; the stored current view must match them exactly.
pub(super) fn replay(
    view: &ExistingExecutionObservationReadOnly,
) -> Result<Vec<ReducibleAttemptV1>, ObservationStoreError> {
    let active = view
        .read_active_bounded(POINTER_MAX_BYTES as u64)
        .map_err(|_| corrupt(CorruptionCategory::MissingActivePointer))?
        .ok_or_else(|| corrupt(CorruptionCategory::MissingActivePointer))?;
    let pointer = parse_canonical::<FixtureActivePointerV1>(&active)
        .map_err(|_| corrupt(CorruptionCategory::NoncanonicalPointer))?;
    if pointer.schema != POINTER_SCHEMA || !is_lowercase_hex64(&pointer.root_sha256) {
        return Err(corrupt(CorruptionCategory::NoncanonicalPointer));
    }

    let mut events: Vec<ReducibleEventV1> = Vec::new();
    let mut root = load_root(view, &pointer.root_sha256)?;
    let stored_view = verify_generation(view, &root)?;
    let mut segment_previous = collect_generation(view, &root, &mut events)?;
    let mut steps = 0_u64;
    while let Some(previous) = root.previous_root_sha256.clone() {
        // cap counts parent edges (event generations), matching the store
        // loader: a legal ledger of exactly EVENTS_MAX events stays readable
        steps += 1;
        if steps > EVENTS_MAX {
            return Err(corrupt(CorruptionCategory::StoredResourceLimit));
        }
        let parent = load_root(view, &previous)?;
        if segment_previous != parent.event_segment_head_sha256 {
            return Err(corrupt(CorruptionCategory::BrokenSegmentChain));
        }
        root = parent;
        verify_generation(view, &root)?;
        segment_previous = collect_generation(view, &root, &mut events)?;
    }
    if root.generation != 0 || root.event_watermark != 0 || root.previous_root_sha256.is_some() {
        return Err(corrupt(CorruptionCategory::BrokenRootChain));
    }
    if hash::root_sha256(&root).map_err(map_stored)? != genesis_root_sha256()? {
        return Err(corrupt(CorruptionCategory::BrokenRootChain));
    }
    events.reverse();
    let attempts = reduce(events)?;
    verify_view_matches(&stored_view, &attempts)?;
    Ok(attempts)
}

/// Reads, verifies, and validates one generation's view; returns it for the
/// final comparison against the reducer output.
fn verify_generation(
    view: &ExistingExecutionObservationReadOnly,
    root: &FixtureLedgerRootV1,
) -> Result<FixtureCurrentViewV1, ObservationStoreError> {
    let view_bytes = read_object(
        view,
        &root.current_view_sha256,
        CURRENT_VIEW_MAX_BYTES,
        CorruptionCategory::CurrentViewMismatch,
    )?;
    let view = parse_canonical::<FixtureCurrentViewV1>(&view_bytes).map_err(map_stored)?;
    if hash::current_view_sha256(&view).map_err(map_stored)? != root.current_view_sha256
        || view.generation != root.generation
        || view.event_watermark != root.event_watermark
    {
        return Err(corrupt(CorruptionCategory::CurrentViewMismatch));
    }
    view.validate().map_err(map_stored)?;
    Ok(view)
}

/// Flattens one generation's segment and event into a reducible event and
/// returns the segment's previous-head reference for the chain binding. The
/// segment's event reference is digest-validated before dereference.
fn collect_generation(
    view: &ExistingExecutionObservationReadOnly,
    root: &FixtureLedgerRootV1,
    events: &mut Vec<ReducibleEventV1>,
) -> Result<Option<String>, ObservationStoreError> {
    let Some(segment_sha256) = root.event_segment_head_sha256.clone() else {
        if root.generation == 0 {
            return Ok(None);
        }
        return Err(corrupt(CorruptionCategory::BrokenSegmentChain));
    };
    let segment_bytes = read_object(
        view,
        &segment_sha256,
        SEGMENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let segment = parse_canonical::<FixtureEventSegmentV1>(&segment_bytes).map_err(map_stored)?;
    if hash::segment_sha256(&segment).map_err(map_stored)? != segment_sha256 {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    if segment.schema != SEGMENT_SCHEMA || segment.last_sequence != segment.first_sequence {
        return Err(corrupt(CorruptionCategory::UnsupportedStoredSchema));
    }
    if !is_lowercase_hex64(&segment.event_sha256) {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let event_bytes = read_object(
        view,
        &segment.event_sha256,
        STORED_EVENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let event_sha256 = segment.event_sha256.clone();
    let root_sha256 = hash::root_sha256(root).map_err(map_stored)?;
    let (reducible, kind_matches) = match parse_event(&event_bytes)? {
        FixtureStoredEvent::Started(event) => {
            let kind_matches = segment.event_kind == EventKind::Started;
            (started_reducible(event, &event_sha256, &root_sha256)?, kind_matches)
        }
        FixtureStoredEvent::Terminal(event) => {
            let kind_matches = segment.event_kind == EventKind::Terminal;
            (terminal_reducible(event, &event_sha256, &root_sha256)?, kind_matches)
        }
    };
    if !kind_matches {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    // Persisted-stamp integrity: the event's own sequence and root_generation
    // are carried into the reducer unmodified and must bind three ways —
    // sequence == segment ordinal == root watermark, and the stamped
    // generation == the root's generation == the sequence. Overwriting the
    // stamp with root.generation before these checks would hide exactly the
    // tamper these comparisons exist to catch.
    if reducible.sequence != segment.first_sequence
        || segment.first_sequence != root.event_watermark
        || reducible.root_generation != root.generation
        || reducible.root_generation != reducible.sequence
    {
        return Err(corrupt(CorruptionCategory::GenerationMismatch));
    }
    events.push(reducible);
    Ok(segment.previous_segment_sha256.clone())
}

fn started_reducible(
    event: StoredStartedEventV1,
    event_sha256: &str,
    root_sha256: &str,
) -> Result<ReducibleEventV1, ObservationStoreError> {
    event.validate().map_err(map_stored)?;
    if hash::started_event_sha256(&event).map_err(map_stored)? != event_sha256 {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let request = &event.request;
    Ok(ReducibleEventV1 {
        sequence: event.sequence,
        root_generation: event.root_generation,
        root_sha256: root_sha256.to_string(),
        recorded_at_ms: event.recorded_at_ms,
        event_sha256: event_sha256.to_string(),
        request_sha256: hash::started_request_sha256(request).map_err(map_stored)?,
        key: request.key,
        kind: ReducibleKindV1::Started {
            policy_sha256: request.policy_sha256.clone(),
            runtime_sha256: request.runtime_sha256.clone(),
        },
    })
}

fn terminal_reducible(
    event: StoredTerminalEventV1,
    event_sha256: &str,
    root_sha256: &str,
) -> Result<ReducibleEventV1, ObservationStoreError> {
    event.validate().map_err(map_stored)?;
    if hash::terminal_event_sha256(&event).map_err(map_stored)? != event_sha256 {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let request = &event.request;
    Ok(ReducibleEventV1 {
        sequence: event.sequence,
        root_generation: event.root_generation,
        root_sha256: root_sha256.to_string(),
        recorded_at_ms: event.recorded_at_ms,
        event_sha256: event_sha256.to_string(),
        request_sha256: hash::terminal_request_sha256(request).map_err(map_stored)?,
        key: request.key,
        kind: ReducibleKindV1::Terminal {
            policy_sha256: request.policy_sha256.clone(),
            runtime_sha256: request.runtime_sha256.clone(),
        },
    })
}

/// The stored current view must equal the reducer output field-by-field;
/// otherwise the view is tampered and the reader fails closed.
fn verify_view_matches(
    view: &FixtureCurrentViewV1,
    attempts: &[ReducibleAttemptV1],
) -> Result<(), ObservationStoreError> {
    if view.attempts.len() != attempts.len() {
        return Err(corrupt(CorruptionCategory::CurrentViewMismatch));
    }
    for (stored, rebuilt) in view.attempts.iter().zip(attempts) {
        let terminal_matches = match (
            &stored.terminal_request_sha256,
            &stored.terminal_event_sha256,
            &rebuilt.terminal,
        ) {
            (None, None, None) => true,
            (Some(request), Some(event), Some(receipt)) => {
                *request == receipt.request_sha256 && *event == receipt.event_sha256
            }
            _ => false,
        };
        if stored.key != rebuilt.key
            || stored.started_request_sha256 != rebuilt.started.request_sha256
            || stored.started_event_sha256 != rebuilt.started.event_sha256
            || !terminal_matches
        {
            return Err(corrupt(CorruptionCategory::CurrentViewMismatch));
        }
    }
    Ok(())
}

/// Recomputes the exact genesis root SHA from the frozen constants (the
/// acceptable chain tail is only this root, never a hash-self-consistent
/// alternate).
fn genesis_root_sha256() -> Result<String, ObservationStoreError> {
    let view = FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.to_string(),
        attestation_state: ATTESTATION_STATE.to_string(),
        generation: 0,
        event_watermark: 0,
        attempts: Vec::new(),
    };
    let view_sha256 = hash::current_view_sha256(&view).map_err(map_stored)?;
    let root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.to_string(),
        trust_class: TRUST_CLASS.to_string(),
        generation: 0,
        previous_root_sha256: None,
        event_segment_head_sha256: None,
        event_watermark: 0,
        current_view_sha256: view_sha256,
        committed_at_ms: 0,
    };
    hash::root_sha256(&root).map_err(map_stored)
}
