//! Bounded loader and structural chain verification (ADR-0008 §4/§5).
//!
//! Every physical read is bounded by the object-kind cap BEFORE any parse;
//! semantic and limit errors observed on stored bytes are remapped to stable
//! `CorruptStore` categories at this boundary and never leak through.

use std::io;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;

use super::super::canonical::parse_canonical;
use super::super::error::{CorruptionCategory, InvalidRequestCategory, LimitCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::model::*;
use super::super::{CURRENT_VIEW_MAX_BYTES, EVENTS_MAX, POINTER_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::{store_debug, store_log, FixtureStoredEventV1, STORED_EVENT_MAX_BYTES};

/// Reads one object by content hash; `missing` is the chain category used
/// when the object is absent (e.g. `broken_root_chain`, `broken_segment_chain`).
pub(super) fn read_object(
    storage: &ExecutionObservationFixtureStorage,
    object_sha256: &str,
    maximum_bytes: usize,
    missing: CorruptionCategory,
) -> Result<Vec<u8>, ObservationStoreError> {
    let bytes = storage
        .get_immutable_bounded(object_sha256, maximum_bytes as u64)
        .map_err(|error| map_read_io(error, missing))?;
    store_debug!("phase=load kind=object bytes={}", bytes.len());
    Ok(bytes)
}

/// Reads one pointer slot; `None` means the slot exists but is empty.
pub(super) fn read_slot(
    storage: &ExecutionObservationFixtureStorage,
    active: bool,
) -> Result<Option<Vec<u8>>, ObservationStoreError> {
    let read = if active {
        storage.read_active_bounded(POINTER_MAX_BYTES as u64)
    } else {
        storage.read_candidate_bounded(POINTER_MAX_BYTES as u64)
    };
    read.map_err(|error| map_read_io(error, CorruptionCategory::MissingActivePointer))
}

pub(super) fn parse_pointer(bytes: &[u8]) -> Result<FixtureActivePointerV1, ObservationStoreError> {
    parse_canonical::<FixtureActivePointerV1>(bytes).map_err(map_stored)
}

pub(super) fn parse_root(bytes: &[u8]) -> Result<FixtureLedgerRootV1, ObservationStoreError> {
    parse_canonical::<FixtureLedgerRootV1>(bytes).map_err(map_stored)
}

/// Parses a stored event of either kind by matching the frozen top-level
/// schema literal at the byte level before the typed parse.
pub(super) fn parse_event(bytes: &[u8]) -> Result<FixtureStoredEventV1, ObservationStoreError> {
    let started_tag = br#""plico.execution-observation.fixture-started/v1""#;
    let terminal_tag = br#""plico.execution-observation.fixture-terminal/v1""#;
    let started = bytes.windows(started_tag.len()).any(|window| window == started_tag);
    let terminal = bytes.windows(terminal_tag.len()).any(|window| window == terminal_tag);
    match (started, terminal) {
        (true, false) => Ok(FixtureStoredEventV1::Started(
            parse_canonical::<StoredStartedEventV1>(bytes).map_err(map_stored)?,
        )),
        (false, true) => Ok(FixtureStoredEventV1::Terminal(
            parse_canonical::<StoredTerminalEventV1>(bytes).map_err(map_stored)?,
        )),
        _ => Err(ObservationStoreError::corrupt(
            CorruptionCategory::UnsupportedStoredSchema,
        )),
    }
}

/// Loads and fully validates the active chain: pointer→root→previous roots,
/// each root's view binding, and each commit's segment→event binding. Orphan
/// objects outside this chain are tolerated (ADR-0008 §5).
pub(super) fn verify_active_chain(
    storage: &ExecutionObservationFixtureStorage,
    pointer: &FixtureActivePointerV1,
) -> Result<FixtureLedgerRootV1, ObservationStoreError> {
    let active = load_root(storage, &pointer.root_sha256)?;
    verify_root_structure(storage, &active)?;
    let mut child = active.clone();
    let mut steps = 0_u64;
    while let Some(previous_sha256) = child.previous_root_sha256.clone() {
        steps += 1;
        if steps > EVENTS_MAX {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit));
        }
        let parent = load_root(storage, &previous_sha256)?;
        if parent.generation + 1 != child.generation || parent.event_watermark + 1 != child.event_watermark {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch));
        }
        if parent.event_segment_head_sha256.is_none() != (parent.generation == 0) {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain));
        }
        verify_root_structure(storage, &parent)?;
        let child_segment = load_segment(storage, child.event_segment_head_sha256.as_deref())?;
        if child_segment.previous_segment_sha256.as_deref() != parent.event_segment_head_sha256.as_deref() {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenSegmentChain));
        }
        child = parent;
    }
    if child.generation != 0 || child.event_watermark != 0 || child.previous_root_sha256.is_some() {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain));
    }
    store_debug!("phase=load kind=chain generations={}", active.generation);
    Ok(active)
}

/// Loads a commit's segment by its head reference (genesis has none).
fn load_segment(
    storage: &ExecutionObservationFixtureStorage,
    segment_sha256: Option<&str>,
) -> Result<FixtureEventSegmentV1, ObservationStoreError> {
    match segment_sha256 {
        None => Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenSegmentChain)),
        Some(segment_sha256) => {
            let bytes = read_object(
                storage,
                segment_sha256,
                SEGMENT_MAX_BYTES,
                CorruptionCategory::BrokenSegmentChain,
            )?;
            let segment = parse_canonical::<FixtureEventSegmentV1>(&bytes).map_err(map_stored)?;
            if hash::segment_sha256(&segment)? != segment_sha256 {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
            }
            Ok(segment)
        }
    }
}

/// Loads one root object and proves its content hash equals its object name.
pub(super) fn load_root(
    storage: &ExecutionObservationFixtureStorage,
    root_sha256: &str,
) -> Result<FixtureLedgerRootV1, ObservationStoreError> {
    let bytes = read_object(
        storage,
        root_sha256,
        ROOT_MAX_BYTES,
        CorruptionCategory::BrokenRootChain,
    )?;
    let root = parse_root(&bytes)?;
    if hash::root_sha256(&root)? != root_sha256 {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    Ok(root)
}

/// Validates one root's own structure plus its view and segment bindings.
/// `full` additionally parses and validates the view body (the active root).
fn verify_root_structure(
    storage: &ExecutionObservationFixtureStorage,
    root: &FixtureLedgerRootV1,
) -> Result<(), ObservationStoreError> {
    root.validate(&root.current_view_sha256).map_err(map_stored)?;
    let view_bytes = read_object(
        storage,
        &root.current_view_sha256,
        CURRENT_VIEW_MAX_BYTES,
        CorruptionCategory::CurrentViewMismatch,
    )?;
    let view = parse_canonical::<FixtureCurrentViewV1>(&view_bytes).map_err(map_stored)?;
    if hash::current_view_sha256(&view)? != root.current_view_sha256
        || view.generation != root.generation
        || view.event_watermark != root.event_watermark
    {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::CurrentViewMismatch));
    }
    view.validate().map_err(map_stored)?;
    match &root.event_segment_head_sha256 {
        None if root.generation == 0 => Ok(()),
        None => Err(ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain)),
        Some(segment_sha256) => {
            let segment_bytes = read_object(
                storage,
                segment_sha256,
                SEGMENT_MAX_BYTES,
                CorruptionCategory::BrokenSegmentChain,
            )?;
            let segment = parse_canonical::<FixtureEventSegmentV1>(&segment_bytes).map_err(map_stored)?;
            if hash::segment_sha256(&segment)? != *segment_sha256 {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
            }
            let event_bytes = read_object(
                storage,
                &segment.event_sha256,
                STORED_EVENT_MAX_BYTES,
                CorruptionCategory::BrokenSegmentChain,
            )?;
            let event = parse_event(&event_bytes)?;
            let (event_sha256, event_sequence, event_generation, event_kind) = match &event {
                FixtureStoredEventV1::Started(event) => {
                    event.validate().map_err(map_stored)?;
                    (
                        hash::started_event_sha256(event)?,
                        event.sequence,
                        event.root_generation,
                        EventKind::Started,
                    )
                }
                FixtureStoredEventV1::Terminal(event) => {
                    event.validate().map_err(map_stored)?;
                    (
                        hash::terminal_event_sha256(event)?,
                        event.sequence,
                        event.root_generation,
                        EventKind::Terminal,
                    )
                }
            };
            if event_sha256 != segment.event_sha256 {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
            }
            segment.validate(&event_sha256).map_err(map_stored)?;
            if segment.first_sequence != root.event_watermark
                || segment.event_kind != event_kind
                || event_sequence != segment.first_sequence
                || event_generation != root.generation
            {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::SequenceGap));
            }
            Ok(())
        }
    }
}

/// io → typed mapping for reads: absent objects are chain corruption,
/// over-cap payloads are `stored_resource_limit`, everything else is
/// `storage_unavailable`. No OS message is ever surfaced.
pub(super) fn map_read_io(error: io::Error, missing: CorruptionCategory) -> ObservationStoreError {
    match error.kind() {
        io::ErrorKind::NotFound => ObservationStoreError::corrupt(missing),
        io::ErrorKind::InvalidData => ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit),
        _ => ObservationStoreError::StorageUnavailable,
    }
}

/// Loader-boundary remap of semantic/limit errors observed on stored bytes.
pub(super) fn map_stored(error: ObservationStoreError) -> ObservationStoreError {
    match error {
        ObservationStoreError::InvalidRequest { category } => ObservationStoreError::corrupt(match category {
            InvalidRequestCategory::UnsupportedSchema | InvalidRequestCategory::InvalidAttestation => {
                CorruptionCategory::UnsupportedStoredSchema
            }
            _ => CorruptionCategory::ObjectHashMismatch,
        }),
        ObservationStoreError::LimitExceeded { category } => ObservationStoreError::corrupt(match category {
            LimitCategory::ObjectBytes => CorruptionCategory::StoredResourceLimit,
            other => {
                let _ = &other; // referenced only by the debug-gated log below
                store_debug!("phase=load category=limit/{:?}", other);
                CorruptionCategory::StoredResourceLimit
            }
        }),
        ObservationStoreError::TransitionConflict { .. } => {
            ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition)
        }
        stored => stored,
    }
}
