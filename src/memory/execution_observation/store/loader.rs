//! Bounded loader with validated-before-dereference typestates (R2-R03):
//! only `Validated*` exposes references for the next CAS lookup; the active
//! chain is verified down to the recomputed exact genesis root (R2-R04).

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;

use super::super::canonical::parse_canonical;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::model::*;
use super::super::validation::is_lowercase_hex64;
use super::super::{CURRENT_VIEW_MAX_BYTES, EVENTS_MAX, POINTER_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::{FixtureStoredEventV1, STORED_EVENT_MAX_BYTES};

pub(super) struct ParsedPointerV1(FixtureActivePointerV1);

pub(super) struct ValidatedPointerV1 {
    root_sha256: String,
}

impl ParsedPointerV1 {
    pub(super) fn validate(self) -> Result<ValidatedPointerV1, ObservationStoreError> {
        if self.0.schema != POINTER_SCHEMA {
            return Err(super::corrupt(CorruptionCategory::UnsupportedStoredSchema));
        }
        if !is_lowercase_hex64(&self.0.root_sha256) {
            return Err(super::corrupt(CorruptionCategory::NoncanonicalPointer));
        }
        Ok(ValidatedPointerV1 {
            root_sha256: self.0.root_sha256,
        })
    }
}

impl ValidatedPointerV1 {
    pub(super) fn root_sha256(&self) -> &str {
        &self.root_sha256
    }
}

/// Schema, caps, ordinals, and name↔hash binding all hold.
pub(super) struct ValidatedRootV1 {
    root: FixtureLedgerRootV1,
    root_sha256: String,
}

impl ValidatedRootV1 {
    pub(super) fn ordinals(&self) -> (u64, u64) {
        (self.root.generation, self.root.event_watermark)
    }
    pub(super) fn sha256(&self) -> &str {
        &self.root_sha256
    }
    pub(super) fn previous_sha256(&self) -> Option<String> {
        self.root.previous_root_sha256.clone()
    }
    pub(super) fn head_sha256(&self) -> Option<String> {
        self.root.event_segment_head_sha256.clone()
    }
}

pub(super) struct ValidatedSegmentV1 {
    segment: FixtureEventSegmentV1,
}

impl ValidatedSegmentV1 {
    /// Only a validated segment gives up its event reference (R2-R03).
    pub(super) fn event_sha256(&self) -> &str {
        &self.segment.event_sha256
    }
    pub(super) fn previous_sha256(&self) -> Option<String> {
        self.segment.previous_segment_sha256.clone()
    }
    fn first_sequence(&self) -> u64 {
        self.segment.first_sequence
    }
    fn kind(&self) -> EventKind {
        self.segment.event_kind
    }
}

/// Reads one slot; `None` means present-but-empty. A NotFound here is
/// unreachable in practice (the sealed opener's topology validation requires
/// both slot files), so the shared missing-pointer category is used.
pub(super) fn read_slot(
    storage: &ExecutionObservationFixtureStorage,
    active: bool,
) -> Result<Option<Vec<u8>>, ObservationStoreError> {
    let read = if active {
        storage.read_active_bounded(POINTER_MAX_BYTES as u64)
    } else {
        storage.read_candidate_bounded(POINTER_MAX_BYTES as u64)
    };
    read.map_err(|error| super::map_read_io(error, CorruptionCategory::MissingActivePointer))
}

pub(super) fn parse_slot_pointer(bytes: &[u8]) -> Result<ValidatedPointerV1, ObservationStoreError> {
    let parsed = parse_canonical::<FixtureActivePointerV1>(bytes)
        .map_err(|_| super::corrupt(CorruptionCategory::NoncanonicalPointer))?;
    ParsedPointerV1(parsed).validate()
}

pub(super) fn load_root(
    storage: &ExecutionObservationFixtureStorage,
    root_sha256: &str,
) -> Result<ValidatedRootV1, ObservationStoreError> {
    let bytes = read_object(
        storage,
        root_sha256,
        ROOT_MAX_BYTES,
        CorruptionCategory::BrokenRootChain,
    )?;
    let root = parse_canonical::<FixtureLedgerRootV1>(&bytes).map_err(super::map_stored_parse)?;
    let computed = hash::root_sha256(&root).map_err(super::map_stored_parse)?;
    if computed != root_sha256 {
        return Err(super::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    Ok(ValidatedRootV1 {
        root,
        root_sha256: computed,
    })
}

pub(super) fn verify_active_chain(
    storage: &ExecutionObservationFixtureStorage,
    pointer: &ValidatedPointerV1,
) -> Result<ValidatedRootV1, ObservationStoreError> {
    let mut child = load_root(storage, pointer.root_sha256())?;
    verify_generation(storage, &child)?;
    let mut steps = 0_u64;
    while let Some(previous_sha256) = child.previous_sha256() {
        steps += 1;
        if steps > EVENTS_MAX {
            return Err(super::corrupt(CorruptionCategory::StoredResourceLimit));
        }
        let parent = load_root(storage, &previous_sha256)?;
        let (child_gen, child_wm) = child.ordinals();
        let (parent_gen, parent_wm) = parent.ordinals();
        if parent_gen.saturating_add(1) != child_gen || parent_wm.saturating_add(1) != child_wm {
            return Err(super::corrupt(CorruptionCategory::GenerationMismatch));
        }
        verify_generation(storage, &parent)?;
        let child_segment = load_segment(storage, child.head_sha256())?;
        if child_segment.previous_sha256() != parent.head_sha256() {
            return Err(super::corrupt(CorruptionCategory::BrokenSegmentChain));
        }
        child = parent;
    }
    let (child_gen, child_wm) = child.ordinals();
    if child_gen != 0
        || child_wm != 0
        || child.previous_sha256().is_some()
        || child.sha256() != super::slots::genesis_materials()?.root_sha256
    {
        return Err(super::corrupt(CorruptionCategory::BrokenRootChain));
    }
    load_root(storage, pointer.root_sha256())
}

/// One generation's structure plus view and segment/event bindings.
fn verify_generation(
    storage: &ExecutionObservationFixtureStorage,
    root: &ValidatedRootV1,
) -> Result<(), ObservationStoreError> {
    let view_sha256 = root.root.current_view_sha256.clone();
    root.root.validate(&view_sha256).map_err(super::map_stored_parse)?;
    let view_bytes = read_object(
        storage,
        &view_sha256,
        CURRENT_VIEW_MAX_BYTES,
        CorruptionCategory::CurrentViewMismatch,
    )?;
    let view = parse_canonical::<FixtureCurrentViewV1>(&view_bytes).map_err(super::map_stored_parse)?;
    let (generation, watermark) = root.ordinals();
    if hash::current_view_sha256(&view).map_err(super::map_stored_parse)? != view_sha256
        || view.generation != generation
        || view.event_watermark != watermark
    {
        return Err(super::corrupt(CorruptionCategory::CurrentViewMismatch));
    }
    view.validate().map_err(super::map_stored_parse)?;
    match root.head_sha256() {
        None if generation == 0 => Ok(()),
        None => Err(super::corrupt(CorruptionCategory::BrokenRootChain)),
        Some(segment_sha256) => {
            let segment = load_segment(storage, Some(segment_sha256))?;
            verify_event_binding(storage, root, &segment)
        }
    }
}

/// Segment schema/self-consistency/digest form proven before dereference.
fn load_segment(
    storage: &ExecutionObservationFixtureStorage,
    segment_sha256: Option<String>,
) -> Result<ValidatedSegmentV1, ObservationStoreError> {
    let segment_sha256 = segment_sha256.ok_or_else(|| super::corrupt(CorruptionCategory::BrokenSegmentChain))?;
    let bytes = read_object(
        storage,
        &segment_sha256,
        SEGMENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let segment = parse_canonical::<FixtureEventSegmentV1>(&bytes).map_err(super::map_stored_parse)?;
    if hash::segment_sha256(&segment).map_err(super::map_stored_parse)? != segment_sha256
        || segment.schema != SEGMENT_SCHEMA
        || segment.last_sequence != segment.first_sequence
        || !is_lowercase_hex64(&segment.event_sha256)
    {
        return Err(super::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    Ok(ValidatedSegmentV1 { segment })
}

fn verify_event_binding(
    storage: &ExecutionObservationFixtureStorage,
    root: &ValidatedRootV1,
    segment: &ValidatedSegmentV1,
) -> Result<(), ObservationStoreError> {
    let event_sha256 = segment.event_sha256().to_string();
    let event_bytes = read_object(
        storage,
        &event_sha256,
        STORED_EVENT_MAX_BYTES,
        CorruptionCategory::BrokenSegmentChain,
    )?;
    let event = parse_event(&event_bytes)?;
    let (computed, sequence, generation, kind) = match &event {
        FixtureStoredEventV1::Started(event) => {
            event.validate().map_err(super::map_stored_parse)?;
            (
                hash::started_event_sha256(event).map_err(super::map_stored_parse)?,
                event.sequence,
                event.root_generation,
                EventKind::Started,
            )
        }
        FixtureStoredEventV1::Terminal(event) => {
            event.validate().map_err(super::map_stored_parse)?;
            (
                hash::terminal_event_sha256(event).map_err(super::map_stored_parse)?,
                event.sequence,
                event.root_generation,
                EventKind::Terminal,
            )
        }
    };
    let (root_gen, root_wm) = root.ordinals();
    let first = segment.first_sequence();
    if computed != event_sha256
        || segment.kind() != kind
        || sequence != first
        || generation != root_gen
        || first != root_wm
    {
        return Err(super::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    Ok(())
}

fn parse_event(bytes: &[u8]) -> Result<FixtureStoredEventV1, ObservationStoreError> {
    let has_tag = |tag: &[u8]| bytes.windows(tag.len()).any(|window| window == tag);
    let started = has_tag(br#""plico.execution-observation.fixture-started/v1""#);
    let terminal = has_tag(br#""plico.execution-observation.fixture-terminal/v1""#);
    match (started, terminal) {
        (true, false) => Ok(FixtureStoredEventV1::Started(
            parse_canonical::<StoredStartedEventV1>(bytes).map_err(super::map_stored_parse)?,
        )),
        (false, true) => Ok(FixtureStoredEventV1::Terminal(
            parse_canonical::<StoredTerminalEventV1>(bytes).map_err(super::map_stored_parse)?,
        )),
        _ => Err(super::corrupt(CorruptionCategory::UnsupportedStoredSchema)),
    }
}

fn read_object(
    storage: &ExecutionObservationFixtureStorage,
    object_sha256: &str,
    maximum_bytes: usize,
    missing: CorruptionCategory,
) -> Result<Vec<u8>, ObservationStoreError> {
    storage
        .get_immutable_bounded(object_sha256, maximum_bytes as u64)
        .map_err(|error| super::map_read_io(error, missing))
}
