//! Dual-slot startup classifier (R2-R04/R05; ADR-0008 §3). Each slot is
//! validated on its own before any relation judgement; active stays the only
//! authority and candidate is never promoted, except the fresh `E/P(G0)`
//! byte-exact genesis republish.

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;

use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::model::*;
use super::loader;
use super::publisher;
use super::FixtureStructuralStateV1;

#[cfg(test)]
use super::super::canonical::to_canonical_vec;
#[cfg(test)]
use super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES};

/// Deterministic genesis object set size (one view + one root).
const GENESIS_OBJECT_COUNT: usize = 2;
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
        pointer_bytes: publisher::canonical_bytes(&pointer)?,
    })
}

/// Classifies the slots; ends with a fully verified active chain.
pub(super) fn startup(
    storage: &ExecutionObservationFixtureStorage,
) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    let active = loader::read_slot(storage, true)?;
    let candidate = loader::read_slot(storage, false)?;
    match (active, candidate) {
        (None, None) => {
            require_fresh_genesis_inventory(storage)?;
            publisher::publish_genesis(storage)?;
            genesis_state()
        }
        (None, Some(candidate_bytes)) => {
            // pointer-shape errors classify before any slot relation (R2-R05)
            let candidate = loader::parse_slot_pointer(&candidate_bytes)?;
            let genesis = genesis_materials()?;
            if candidate.root_sha256() != genesis.root_sha256.as_str() {
                return Err(invalid_candidate_state());
            }
            require_fresh_genesis_inventory(storage)?;
            publisher::publish_genesis(storage)?;
            genesis_state()
        }
        (Some(active_bytes), None) => {
            let pointer = loader::parse_slot_pointer(&active_bytes)?;
            let root = loader::verify_active_chain(storage, &pointer)?;
            if root.ordinals() == (0, 0) && root.previous_sha256().is_none() {
                state_of(&root)
            } else {
                Err(invalid_candidate_state())
            }
        }
        (Some(active_bytes), Some(candidate_bytes)) => {
            let pointer = loader::parse_slot_pointer(&active_bytes)?;
            let active_root = loader::verify_active_chain(storage, &pointer)?;
            let candidate_pointer = loader::parse_slot_pointer(&candidate_bytes)?;
            if candidate_pointer.root_sha256() == pointer.root_sha256() {
                return Err(invalid_candidate_state());
            }
            let candidate_root = loader::load_root(storage, candidate_pointer.root_sha256())?;
            let active_ordinals = active_root.ordinals();
            let candidate_ordinals = candidate_root.ordinals();
            let one_step = |child: (u64, u64), parent: (u64, u64)| {
                child.0 == parent.0.saturating_add(1) && child.1 == parent.1.saturating_add(1) && child.0 > 0
            };
            let child_of_active = candidate_root.previous_sha256().as_deref() == Some(pointer.root_sha256())
                && one_step(candidate_ordinals, active_ordinals);
            let active_child_of_candidate = active_root.previous_sha256().as_deref()
                == Some(candidate_pointer.root_sha256())
                && one_step(active_ordinals, candidate_ordinals);
            if child_of_active {
                // verified but never promoted (pre-exchange prepared child)
                loader::verify_active_chain(storage, &candidate_pointer)?;
                state_of(&active_root)
            } else if active_child_of_candidate {
                state_of(&active_root)
            } else {
                Err(invalid_candidate_state())
            }
        }
    }
}

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
        Ok(())
    } else {
        Err(invalid_candidate_state())
    }
}

fn genesis_state() -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    Ok(FixtureStructuralStateV1 {
        root_sha256: genesis_materials()?.root_sha256,
        generation: 0,
        event_watermark: 0,
    })
}

fn state_of(root: &loader::ValidatedRootV1) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
    let (generation, event_watermark) = root.ordinals();
    Ok(FixtureStructuralStateV1 {
        root_sha256: root.sha256().to_string(),
        generation,
        event_watermark,
    })
}

fn invalid_candidate_state() -> ObservationStoreError {
    ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
}

/// Test staging helper: one bounded object write through the frozen seam.
#[cfg(test)]
pub(super) fn put(cap: &ExecutionObservationFixtureStorage, sha: &str, bytes: Vec<u8>, max: usize) {
    cap.put_immutable_bounded(sha, &bytes, max as u64).expect("put object");
}

/// Test staging helper: writes the recomputed genesis objects.
#[cfg(test)]
pub(super) fn put_genesis(cap: &ExecutionObservationFixtureStorage) -> GenesisMaterials {
    let genesis = genesis_materials().expect("genesis");
    put(
        cap,
        &genesis.view_sha256,
        to_canonical_vec(&genesis.view).expect("v"),
        CURRENT_VIEW_MAX_BYTES,
    );
    put(
        cap,
        &genesis.root_sha256,
        to_canonical_vec(&genesis.root).expect("r"),
        ROOT_MAX_BYTES,
    );
    genesis
}

/// Test staging helper: publishes an active pointer through the frozen seam.
#[cfg(test)]
pub(super) fn stage_active(cap: &ExecutionObservationFixtureStorage, root_sha256: String) {
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256,
    };
    cap.publish_active(&to_canonical_vec(&pointer).expect("pointer"))
        .expect("publish");
}
