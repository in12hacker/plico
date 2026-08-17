//! Structural publisher (R2-R06/R07 consumption; ADR-0008 §5).
//!
//! Every domain binding is recomputed, writes go through the CAS
//! single-step atomic NOREPLACE primitive, and the only fault windows are
//! pre-exchange (`storage_unavailable`, active bytes intact) and
//! post-exchange sync (`commit_indeterminate`, handle poison). There is no
//! final flush after a successful publish.

use std::fmt::Display;
use std::io::{Error, ErrorKind};

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;

use super::super::canonical::to_canonical_vec;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::model::*;
use super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::loader::ValidatedRootV1;
use super::slots;
use super::{FixtureStoredEventV1, FixtureStructuralCommitV1, FixtureStructuralStateV1, STORED_EVENT_MAX_BYTES};

#[cfg(test)]
use super::super::hash::tests::{TERMINAL_RECORDED_AT_MS, TERMINAL_ROOT_SHA, TERMINAL_SEGMENT_SHA};
#[cfg(test)]
use super::super::tests::GoldenChain;

/// Serializes any model value; used for pointer bytes and object writes.
pub(super) fn canonical_bytes<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, ObservationStoreError> {
    to_canonical_vec(value)
}

/// Writes the deterministic genesis objects and publishes the genesis
/// pointer. Re-running is safe: identical objects are idempotent under the
/// content-addressed CAS and the exchange lands the same pointer bytes.
pub(super) fn publish_genesis(storage: &ExecutionObservationFixtureStorage) -> Result<(), ObservationStoreError> {
    let genesis = slots::genesis_materials()?;
    put_object(
        storage,
        &genesis.view_sha256,
        &canonical_bytes(&genesis.view)?,
        CURRENT_VIEW_MAX_BYTES,
    )?;
    put_object(
        storage,
        &genesis.root_sha256,
        &canonical_bytes(&genesis.root)?,
        ROOT_MAX_BYTES,
    )?;
    publish_pointer(storage, &genesis.pointer_bytes)
}

/// Commits one structural bundle after recomputing every binding and proving
/// the unique-direct-child relationship against the current active head.
pub(super) fn commit(
    storage: &ExecutionObservationFixtureStorage,
    current: &FixtureStructuralStateV1,
    commit: FixtureStructuralCommitV1,
) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    let FixtureStructuralCommitV1 {
        event,
        segment,
        current_view,
        root,
    } = commit;

    // 1. typed validation of the caller bundle (caller categories preserved)
    match &event {
        FixtureStoredEventV1::Started(event) => event.validate()?,
        FixtureStoredEventV1::Terminal(event) => event.validate()?,
    }
    let event_sha256 = event_sha256(&event)?;
    segment.validate(&event_sha256)?;
    current_view.validate()?;

    // 2. recompute every binding; caller digests are never trusted
    if segment.event_sha256 != event_sha256 || segment.event_kind != event_kind(&event) {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let segment_sha256 = hash::segment_sha256(&segment)?;
    let view_sha256 = hash::current_view_sha256(&current_view)?;
    if root.current_view_sha256 != view_sha256
        || current_view.generation != root.generation
        || current_view.event_watermark != root.event_watermark
    {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    match &event {
        FixtureStoredEventV1::Started(event) => {
            if event.sequence != segment.first_sequence || event.root_generation != root.generation {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::SequenceGap));
            }
        }
        FixtureStoredEventV1::Terminal(event) => {
            if event.sequence != segment.first_sequence || event.root_generation != root.generation {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::SequenceGap));
            }
        }
    }
    let root_sha256 = hash::root_sha256(&root)?;
    root.validate(&view_sha256)?;

    // 3. the new root must be the unique direct child of the active head
    let active_root = super::loader::load_root(storage, &current.root_sha256)?;
    if root.previous_root_sha256.as_deref() != Some(current.root_sha256.as_str())
        || root.generation != current.generation + 1
        || root.event_watermark != current.event_watermark + 1
        || segment.first_sequence != current.event_watermark + 1
        || segment.previous_segment_sha256.as_deref() != active_head(&active_root).as_deref()
    {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch));
    }

    // 4. bounded content-addressed writes, then the atomic pointer publish
    let event_bytes = match &event {
        FixtureStoredEventV1::Started(event) => canonical_bytes(event)?,
        FixtureStoredEventV1::Terminal(event) => canonical_bytes(event)?,
    };
    put_object(storage, &event_sha256, &event_bytes, STORED_EVENT_MAX_BYTES)?;
    put_object(storage, &segment_sha256, &canonical_bytes(&segment)?, SEGMENT_MAX_BYTES)?;
    put_object(
        storage,
        &view_sha256,
        &canonical_bytes(&current_view)?,
        CURRENT_VIEW_MAX_BYTES,
    )?;
    put_object(storage, &root_sha256, &canonical_bytes(&root)?, ROOT_MAX_BYTES)?;
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.to_string(),
        root_sha256: root_sha256.clone(),
    };
    publish_pointer(storage, &canonical_bytes(&pointer)?)?;
    Ok(FixtureStructuralStateV1 {
        root_sha256,
        generation: root.generation,
        event_watermark: root.event_watermark,
    })
}

fn active_head(active_root: &ValidatedRootV1) -> Option<String> {
    active_root.head_sha256()
}

fn event_kind(event: &FixtureStoredEventV1) -> EventKind {
    match event {
        FixtureStoredEventV1::Started(_) => EventKind::Started,
        FixtureStoredEventV1::Terminal(_) => EventKind::Terminal,
    }
}

fn event_sha256(event: &FixtureStoredEventV1) -> Result<String, ObservationStoreError> {
    Ok(match event {
        FixtureStoredEventV1::Started(event) => hash::started_event_sha256(event)?,
        FixtureStoredEventV1::Terminal(event) => hash::terminal_event_sha256(event)?,
    })
}

/// The two frozen fault windows. The `LedgerStorageError` variants are not
/// importable under the WP2 allowlist, so the post-exchange window is told
/// by the frozen message prefix of `PublishedButUnsynced` (A3 ledger bytes).
fn publish_pointer(
    storage: &ExecutionObservationFixtureStorage,
    pointer_bytes: &[u8],
) -> Result<(), ObservationStoreError> {
    match storage.publish_active(pointer_bytes) {
        Ok(()) => Ok(()),
        Err(error) if post_exchange_uncertainty(&error) => Err(ObservationStoreError::CommitIndeterminate),
        Err(_) => Err(ObservationStoreError::StorageUnavailable),
    }
}

fn post_exchange_uncertainty(error: &impl Display) -> bool {
    error
        .to_string()
        .starts_with("active ledger pointer was exchanged but directory sync failed")
}

/// Bounded single-step atomic write; collision rereads stay bounded in CAS.
pub(super) fn put_object(
    storage: &ExecutionObservationFixtureStorage,
    sha256: &str,
    bytes: &[u8],
    maximum_bytes: usize,
) -> Result<(), ObservationStoreError> {
    storage
        .put_immutable_bounded(sha256, bytes, maximum_bytes as u64)
        .map_err(map_put_io)
}

fn map_put_io(error: Error) -> ObservationStoreError {
    match error.kind() {
        ErrorKind::AlreadyExists => ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch),
        ErrorKind::InvalidInput => ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit),
        ErrorKind::InvalidData => ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit),
        _ => ObservationStoreError::StorageUnavailable,
    }
}

/// Test fixture: the golden chain's Started commit bundle.
#[cfg(test)]
pub(super) fn started_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Started(chain.started_event.clone()),
        segment: chain.started_segment.clone(),
        current_view: chain.open_view.clone(),
        root: chain.started_root.clone(),
    }
}

/// Test fixture: the golden chain's Terminal commit bundle.
#[cfg(test)]
pub(super) fn terminal_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Terminal(chain.terminal_event.clone()),
        segment: chain.terminal_segment.clone(),
        current_view: chain.terminal_view.clone(),
        root: chain.terminal_root.clone(),
    }
}

/// Test fixture: the terminal bundle advanced to generation 3 (rehashes
/// every binding exactly like the publisher does).
#[cfg(test)]
pub(super) fn third_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    let mut event = chain.terminal_event.clone();
    event.sequence = 3;
    event.root_generation = 3;
    event.recorded_at_ms = TERMINAL_RECORDED_AT_MS + 1;
    let event_sha256 = hash::terminal_event_sha256(&event).expect("event hash");
    let mut segment = chain.terminal_segment.clone();
    segment.first_sequence = 3;
    segment.last_sequence = 3;
    segment.previous_segment_sha256 = Some(TERMINAL_SEGMENT_SHA.into());
    segment.event_sha256 = event_sha256;
    let segment_sha256 = hash::segment_sha256(&segment).expect("segment hash");
    let mut view = chain.terminal_view.clone();
    view.generation = 3;
    view.event_watermark = 3;
    let view_sha256 = hash::current_view_sha256(&view).expect("view hash");
    let mut root = chain.terminal_root.clone();
    root.generation = 3;
    root.event_watermark = 3;
    root.previous_root_sha256 = Some(TERMINAL_ROOT_SHA.into());
    root.event_segment_head_sha256 = Some(segment_sha256);
    root.current_view_sha256 = view_sha256;
    root.committed_at_ms = TERMINAL_RECORDED_AT_MS + 1;
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Terminal(event),
        segment,
        current_view: view,
        root,
    }
}
