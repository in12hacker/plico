//! Structural publisher (ADR-0008 §5): recomputes every domain binding and
//! drives the bounded object writes plus the atomic pointer publish.
//!
//! Failure semantics: any error before the pointer exchange keeps the old
//! active bytes (`storage_unavailable`); once the exchange happened but
//! durability is unconfirmed the only honest answer is `commit_indeterminate`
//! and the caller's handle is poisoned (set by `store::commit_structural`).

use std::io;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::ledger_store::LedgerStorageError;

use super::super::canonical::to_canonical_vec;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::model::*;
use super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::loader;
use super::slots;
use super::{
    store_debug, store_error, store_info, store_log, store_warn, FixtureStoredEventV1, FixtureStructuralCommitV1,
    FixtureStructuralStateV1, STORED_EVENT_MAX_BYTES,
};

/// Writes the deterministic genesis objects and publishes the genesis
/// pointer. Safe to re-run: identical objects are idempotent under the
/// content-addressed CAS, and the exchange lands the same pointer bytes.
pub(super) fn publish_genesis(storage: &ExecutionObservationFixtureStorage) -> Result<(), ObservationStoreError> {
    let genesis = slots::genesis_materials()?;
    put_object(
        storage,
        &genesis.view_sha256,
        &to_canonical_vec(&genesis.view)?,
        CURRENT_VIEW_MAX_BYTES,
    )?;
    put_object(
        storage,
        &genesis.root_sha256,
        &to_canonical_vec(&genesis.root)?,
        ROOT_MAX_BYTES,
    )?;
    publish_pointer(storage, &genesis.pointer_bytes)?;
    store_info!("phase=open kind=genesis-published generation=0");
    Ok(())
}

/// Commits one structural bundle after recomputing every binding.
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

    // 1. typed validation of the caller bundle (caller-input categories kept)
    match &event {
        FixtureStoredEventV1::Started(event) => event.validate()?,
        FixtureStoredEventV1::Terminal(event) => event.validate()?,
    }
    let event_sha256 = event_sha256(&event)?;
    segment.validate(&event_sha256)?;
    current_view.validate()?;
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
    if segment.event_kind != event_kind(&event) {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition));
    }

    // 2. recompute every binding; caller-supplied digests are never trusted
    if segment.event_sha256 != event_sha256 {
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
    if root.event_segment_head_sha256.as_deref() != Some(segment_sha256.as_str()) {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    let root_sha256 = hash::root_sha256(&root)?;
    root.validate(&view_sha256)?;

    // 3. the new root must be the unique direct child of the active root:
    //    generation and watermark each +1, previous root bound exactly, the
    //    segment numbered next, and its previous head bound to the parent's
    let active_root = loader::load_root(storage, &current.root_sha256)?;
    if root.previous_root_sha256.as_deref() != Some(current.root_sha256.as_str())
        || root.generation != current.generation + 1
        || root.event_watermark != current.event_watermark + 1
        || segment.first_sequence != current.event_watermark + 1
        || segment.previous_segment_sha256.as_deref() != active_root.event_segment_head_sha256.as_deref()
    {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch));
    }

    // 4. bounded content-addressed writes, then the atomic pointer publish
    let event_bytes = match &event {
        FixtureStoredEventV1::Started(event) => to_canonical_vec(event)?,
        FixtureStoredEventV1::Terminal(event) => to_canonical_vec(event)?,
    };
    put_object(storage, &event_sha256, &event_bytes, STORED_EVENT_MAX_BYTES)?;
    store_debug!("phase=put kind=event watermark={}", root.event_watermark);
    put_object(
        storage,
        &segment_sha256,
        &to_canonical_vec(&segment)?,
        SEGMENT_MAX_BYTES,
    )?;
    put_object(
        storage,
        &view_sha256,
        &to_canonical_vec(&current_view)?,
        CURRENT_VIEW_MAX_BYTES,
    )?;
    put_object(storage, &root_sha256, &to_canonical_vec(&root)?, ROOT_MAX_BYTES)?;
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.to_string(),
        root_sha256: root_sha256.clone(),
    };
    publish_pointer(storage, &to_canonical_vec(&pointer)?)?;
    storage.flush().map_err(|_| {
        store_error!("phase=flush category=commit_indeterminate");
        ObservationStoreError::CommitIndeterminate
    })?;
    Ok(FixtureStructuralStateV1 {
        root_sha256,
        generation: root.generation,
        event_watermark: root.event_watermark,
    })
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

/// `Err(Io)` is strictly pre-exchange (old active bytes intact);
/// `Err(PublishedButUnsynced)` is post-exchange indeterminacy.
fn publish_pointer(
    storage: &ExecutionObservationFixtureStorage,
    pointer_bytes: &[u8],
) -> Result<(), ObservationStoreError> {
    match storage.publish_active(pointer_bytes) {
        Ok(()) => Ok(()),
        Err(LedgerStorageError::PublishedButUnsynced(_)) => {
            store_error!("phase=publish category=commit_indeterminate detail=post-exchange-sync");
            Err(ObservationStoreError::CommitIndeterminate)
        }
        Err(LedgerStorageError::Io(_)) => {
            store_warn!("phase=publish category=storage_unavailable detail=pre-exchange");
            Err(ObservationStoreError::StorageUnavailable)
        }
    }
}

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

fn map_put_io(error: io::Error) -> ObservationStoreError {
    match error.kind() {
        io::ErrorKind::AlreadyExists => ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch),
        io::ErrorKind::InvalidInput => ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit),
        _ => ObservationStoreError::StorageUnavailable,
    }
}
