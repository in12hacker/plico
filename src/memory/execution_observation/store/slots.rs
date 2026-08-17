//! Dual-slot startup classifier (ADR-0008 §3; ADR-0007 §7.1).
//!
//! `active` is the only authoritative slot. The candidate slot is never
//! auto-promoted; the single recovery exception is fresh `E/P(G0)`, which
//! recomputes the exact genesis and re-runs the same publish. Fresh `E/E`
//! accepts only a deterministic subset of the genesis object set.

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;

use super::super::canonical::to_canonical_vec;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::model::*;
use super::loader;
use super::publisher;
use super::{store_debug, store_info, store_log, store_warn, FixtureStructuralStateV1};

/// Deterministic genesis object set size (one view + one root).
const GENESIS_OBJECT_COUNT: usize = 2;

/// Recomputed genesis materials: view/root objects, their content hashes,
/// and the canonical active-pointer bytes.
pub(super) struct GenesisMaterials {
    pub(super) view: FixtureCurrentViewV1,
    pub(super) root: FixtureLedgerRootV1,
    pub(super) view_sha256: String,
    pub(super) root_sha256: String,
    pub(super) pointer_bytes: Vec<u8>,
}

pub(super) fn genesis_materials() -> Result<GenesisMaterials, ObservationStoreError> {
    let view = FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.to_string(),
        attestation_state: ATTESTATION_STATE.to_string(),
        generation: 0,
        event_watermark: 0,
        attempts: Vec::new(),
    };
    let view_sha256 = hash::current_view_sha256(&view)?;
    let root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.to_string(),
        trust_class: TRUST_CLASS.to_string(),
        generation: 0,
        previous_root_sha256: None,
        event_segment_head_sha256: None,
        event_watermark: 0,
        current_view_sha256: view_sha256.clone(),
        committed_at_ms: 0,
    };
    let root_sha256 = hash::root_sha256(&root)?;
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.to_string(),
        root_sha256: root_sha256.clone(),
    };
    Ok(GenesisMaterials {
        view,
        root,
        view_sha256,
        root_sha256,
        pointer_bytes: to_canonical_vec(&pointer)?,
    })
}

/// Classifies the two pointer slots and drives the startup behavior, ending
/// with a fully verified active chain.
pub(super) fn startup(
    storage: &ExecutionObservationFixtureStorage,
) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    let active = loader::read_slot(storage, true)?;
    let candidate = loader::read_slot(storage, false)?;
    match (active, candidate) {
        (None, None) => {
            store_debug!("phase=classify slots=E/E");
            require_fresh_genesis_inventory(storage)?;
            publisher::publish_genesis(storage)?;
            state_after_genesis()
        }
        (None, Some(candidate_bytes)) => {
            store_debug!("phase=classify slots=E/P");
            let genesis = genesis_materials()?;
            if candidate_bytes != genesis.pointer_bytes {
                store_warn!("phase=classify category=invalid_candidate_state detail=candidate-not-genesis");
                return Err(invalid_candidate_state());
            }
            require_fresh_genesis_inventory(storage)?;
            publisher::publish_genesis(storage)?;
            state_after_genesis()
        }
        (Some(active_bytes), None) => {
            store_debug!("phase=classify slots=P/E");
            let pointer = checked_pointer(loader::parse_pointer(&active_bytes)?)?;
            let root = loader::verify_active_chain(storage, &pointer)?;
            if root.generation == 0 {
                // accepted genesis: candidate slot simply lost the old empty
                // pointer after the exchange, which is the normal end state
                state_of(&root)
            } else {
                store_warn!("phase=classify category=invalid_candidate_state detail=active-gen-without-candidate");
                Err(invalid_candidate_state())
            }
        }
        (Some(active_bytes), Some(candidate_bytes)) => {
            store_debug!("phase=classify slots=P/P");
            let pointer = checked_pointer(loader::parse_pointer(&active_bytes)?)?;
            let active_root = loader::verify_active_chain(storage, &pointer)?;
            let candidate_pointer = checked_pointer(loader::parse_pointer(&candidate_bytes)?)?;
            let candidate_root = loader::load_root(storage, &candidate_pointer.root_sha256)?;
            if candidate_pointer.root_sha256 == pointer.root_sha256 {
                store_warn!("phase=classify category=invalid_candidate_state detail=same-root");
                return Err(invalid_candidate_state());
            }
            let child_of_active = candidate_root.generation == active_root.generation + 1
                && candidate_root.event_watermark == active_root.event_watermark + 1
                && candidate_root.previous_root_sha256.as_deref() == Some(pointer.root_sha256.as_str());
            let active_child_of_candidate = active_root.generation == candidate_root.generation + 1
                && active_root.event_watermark == candidate_root.event_watermark + 1
                && active_root.previous_root_sha256.as_deref() == Some(candidate_pointer.root_sha256.as_str());
            if child_of_active {
                // prepared child from a pre-exchange failure: verified, never promoted
                loader::verify_active_chain(storage, &candidate_pointer)?;
                state_of(&active_root)
            } else if active_child_of_candidate {
                // candidate still holds the previous active root: normal post-publish
                state_of(&active_root)
            } else {
                store_warn!("phase=classify category=invalid_candidate_state detail=not-direct-parent-child");
                Err(invalid_candidate_state())
            }
        }
    }
}

/// Fresh directories may only hold a subset of the deterministic genesis
/// object set; anything else fails closed (constant memory: list cap is
/// GENESIS_OBJECT_COUNT + 1).
fn require_fresh_genesis_inventory(storage: &ExecutionObservationFixtureStorage) -> Result<(), ObservationStoreError> {
    let genesis = genesis_materials()?;
    let hashes = storage
        .list_immutable_hashes_bounded(GENESIS_OBJECT_COUNT + 1)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::InvalidData => invalid_candidate_state(),
            _ => ObservationStoreError::StorageUnavailable,
        })?;
    if hashes
        .iter()
        .all(|hash| *hash == genesis.view_sha256 || *hash == genesis.root_sha256)
    {
        store_info!("phase=classify kind=fresh-inventory objects={}", hashes.len());
        Ok(())
    } else {
        store_warn!("phase=classify category=invalid_candidate_state detail=non-genesis-object");
        Err(invalid_candidate_state())
    }
}

fn state_after_genesis() -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    Ok(FixtureStructuralStateV1 {
        root_sha256: genesis_materials()?.root_sha256,
        generation: 0,
        event_watermark: 0,
    })
}

fn state_of(root: &FixtureLedgerRootV1) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    Ok(FixtureStructuralStateV1 {
        root_sha256: hash::root_sha256(root)?,
        generation: root.generation,
        event_watermark: root.event_watermark,
    })
}

/// A pointer read from a slot must carry the frozen schema and a canonical
/// digest form; anything else is a noncanonical pointer, never an I/O issue.
fn checked_pointer(pointer: FixtureActivePointerV1) -> Result<FixtureActivePointerV1, ObservationStoreError> {
    if pointer.schema != POINTER_SCHEMA || !crate_pointer_digest_ok(&pointer.root_sha256) {
        return Err(ObservationStoreError::corrupt(CorruptionCategory::NoncanonicalPointer));
    }
    Ok(pointer)
}

fn crate_pointer_digest_ok(value: &str) -> bool {
    super::super::validation::is_lowercase_hex64(value)
}

fn invalid_candidate_state() -> ObservationStoreError {
    ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
}
