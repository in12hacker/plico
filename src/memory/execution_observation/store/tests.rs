//! WP2 store tests: F06 crash-window behavior, genesis resume, slot
//! corruption fail-closed, happy-path chain commit, and bounded writes.
//! All physical state is built through the frozen store/CAS seams on
//! tempfile vaults; no raw fs manipulation.

use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::ledger_store::PersonalVaultStorage;

use super::super::canonical::to_canonical_vec;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::hash::tests::{GENESIS_ROOT_SHA, TERMINAL_RECORDED_AT_MS, TERMINAL_ROOT_SHA, TERMINAL_SEGMENT_SHA};
use super::super::tests::{golden_chain, GoldenChain};
use super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES};
use super::{
    publisher, slots, FixtureObservationStoreV1, FixtureStoredEventV1, FixtureStructuralCommitV1,
    STORED_EVENT_MAX_BYTES,
};

fn open_store(vault_path: &Path) -> FixtureObservationStoreV1 {
    let vault = Arc::new(PersonalVaultStorage::open(vault_path, None).expect("vault"));
    FixtureObservationStoreV1::open_fixture(vault).expect("store")
}

fn put_genesis_objects(capability: &ExecutionObservationFixtureStorage) -> slots::GenesisMaterials {
    let genesis = slots::genesis_materials().expect("genesis");
    for (sha, bytes, cap) in [
        (
            &genesis.view_sha256,
            to_canonical_vec(&genesis.view).expect("v"),
            CURRENT_VIEW_MAX_BYTES,
        ),
        (
            &genesis.root_sha256,
            to_canonical_vec(&genesis.root).expect("r"),
            ROOT_MAX_BYTES,
        ),
    ] {
        capability
            .put_immutable_bounded(sha, &bytes, cap as u64)
            .expect("put genesis object");
    }
    genesis
}

fn started_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Started(chain.started_event.clone()),
        segment: chain.started_segment.clone(),
        current_view: chain.open_view.clone(),
        root: chain.started_root.clone(),
    }
}

fn terminal_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Terminal(chain.terminal_event.clone()),
        segment: chain.terminal_segment.clone(),
        current_view: chain.terminal_view.clone(),
        root: chain.terminal_root.clone(),
    }
}

/// A locally derived generation-3 bundle (the terminal bundle is generation 2).
fn third_bundle(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    let mut event = chain.terminal_event.clone();
    event.sequence = 3;
    event.root_generation = 3;
    event.recorded_at_ms = TERMINAL_RECORDED_AT_MS + 1;
    let event_sha256 = hash::terminal_event_sha256(&event).expect("hash");
    let mut segment = chain.terminal_segment.clone();
    segment.first_sequence = 3;
    segment.last_sequence = 3;
    segment.previous_segment_sha256 = Some(TERMINAL_SEGMENT_SHA.to_string());
    segment.event_sha256 = event_sha256.clone();
    let segment_sha256 = hash::segment_sha256(&segment).expect("hash");
    let mut view = chain.terminal_view.clone();
    view.generation = 3;
    view.event_watermark = 3;
    let view_sha256 = hash::current_view_sha256(&view).expect("hash");
    let mut root = chain.terminal_root.clone();
    root.generation = 3;
    root.event_watermark = 3;
    root.previous_root_sha256 = Some(TERMINAL_ROOT_SHA.to_string());
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

#[test]
fn execution_observation_f06_pre_exchange_keeps_active_without_promotion() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    let store = open_store(&vault_path);
    let chain = golden_chain();
    let after_started = store.commit_structural(started_bundle(&chain)).expect("commit #1");
    assert_eq!((after_started.generation, after_started.event_watermark), (1, 1));

    store.inject_pre_exchange_failure_once();
    let failed = store.commit_structural(terminal_bundle(&chain));
    assert_eq!(failed.err(), Some(ObservationStoreError::StorageUnavailable));
    let unchanged = store
        .structural_state()
        .expect("state readable after pre-exchange failure");
    let expected_root = after_started.root_sha256.clone();
    assert_eq!((unchanged.generation, unchanged.root_sha256), (1, expected_root));
    drop(store);

    // reopen: active stays R1; the durable candidate is its direct child and
    // must be verified but never promoted (P(Rn)/P(Rn+1))
    let reopened = open_store(&vault_path);
    let state = reopened.structural_state().expect("reopen state");
    assert_eq!(state.generation, 1);
    assert_eq!(state.root_sha256, after_started.root_sha256);
    // the same bundle can be retried and succeeds over the stale candidate
    let retried = reopened
        .commit_structural(terminal_bundle(&chain))
        .expect("retry commit");
    assert_eq!(
        (retried.generation, retried.root_sha256),
        (2, TERMINAL_ROOT_SHA.to_string())
    );
}

#[test]
fn execution_observation_f06_post_exchange_indeterminate_poisons_and_recovers() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    let store = open_store(&vault_path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("commit #1");

    store.inject_post_exchange_sync_failure_once();
    let failed = store.commit_structural(terminal_bundle(&chain));
    assert_eq!(failed.err(), Some(ObservationStoreError::CommitIndeterminate));
    // the poisoned handle refuses every later read/write conclusion
    assert_eq!(store.structural_state().err(), Some(ObservationStoreError::Poisoned));
    assert_eq!(
        store.commit_structural(third_bundle(&chain)).err(),
        Some(ObservationStoreError::Poisoned)
    );
    drop(store);

    // reopen resolves the uncertainty by full verification of the new active
    let reopened = open_store(&vault_path);
    let state = reopened.structural_state().expect("reopen after indeterminate");
    assert_eq!((state.generation, state.event_watermark), (2, 2));
    assert_eq!(state.root_sha256, TERMINAL_ROOT_SHA.to_string());
}

#[test]
fn execution_observation_genesis_resume_republishes_exact_genesis() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    {
        // build E/P(G0) through the frozen CAS seam only: genesis objects
        // durable, candidate pointer written, exchange deliberately failed
        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
        let capability = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("capability");
        let genesis = slots::genesis_materials().expect("genesis");
        capability
            .put_immutable_bounded(
                &genesis.view_sha256,
                &to_canonical_vec(&genesis.view).expect("bytes"),
                CURRENT_VIEW_MAX_BYTES as u64,
            )
            .expect("put view");
        capability
            .put_immutable_bounded(
                &genesis.root_sha256,
                &to_canonical_vec(&genesis.root).expect("bytes"),
                ROOT_MAX_BYTES as u64,
            )
            .expect("put root");
        capability.inject_pre_exchange_failure_once();
        capability
            .publish_active(&genesis.pointer_bytes)
            .expect_err("pre-exchange failure");
    }
    let store = open_store(&vault_path);
    let state = store.structural_state().expect("resumed genesis");
    assert_eq!((state.generation, state.event_watermark), (0, 0));
    // the recomputed genesis identity equals the frozen golden vector
    assert_eq!(state.root_sha256, GENESIS_ROOT_SHA.to_string());
    // converse: E/P where the candidate is NOT the exact genesis pointer
    // (byte-corrupted pointer bytes left by a pre-exchange-failed publish)
    // must fail closed as invalid_candidate_state
    let foreign_path = parent.path().join("vault-foreign");
    {
        let vault = Arc::new(PersonalVaultStorage::open(&foreign_path, None).expect("vault"));
        let capability = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("capability");
        let genesis = put_genesis_objects(&capability);
        let mut foreign = genesis.pointer_bytes;
        *foreign.last_mut().expect("non-empty") ^= b'}';
        capability.inject_pre_exchange_failure_once();
        capability.publish_active(&foreign).expect_err("pre-exchange failure");
    }
    let vault = Arc::new(PersonalVaultStorage::open(&foreign_path, None).expect("vault"));
    let failure = match FixtureObservationStoreV1::open_fixture(vault) {
        Ok(_) => panic!("non-genesis candidate must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        failure,
        ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
    );
}

#[test]
fn execution_observation_same_root_in_both_slots_fails_closed() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    {
        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
        let capability = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("capability");
        let genesis = put_genesis_objects(&capability);
        // publishing the identical pointer twice leaves both slots at the
        // same root, which no legal publish sequence can produce
        capability
            .publish_active(&genesis.pointer_bytes)
            .expect("first publish");
        capability
            .publish_active(&genesis.pointer_bytes)
            .expect("second publish");
    }
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let failure = match FixtureObservationStoreV1::open_fixture(vault) {
        Ok(_) => panic!("same-root slots must fail closed"),
        Err(error) => error,
    };
    assert_eq!(
        failure,
        ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
    );
}

#[test]
fn execution_observation_commit_happy_path_verifies_full_chain() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    let store = open_store(&vault_path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("started commit");
    // a bundle that skips the direct-child requirement (generation 3 while
    // active is generation 1) must be rejected before any write
    let skipped = store.commit_structural(third_bundle(&chain));
    assert!(matches!(
        skipped,
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::GenerationMismatch
        })
    ));
    let state = store
        .commit_structural(terminal_bundle(&chain))
        .expect("terminal commit");
    assert_eq!((state.generation, state.event_watermark), (2, 2));
    assert_eq!(state.root_sha256, TERMINAL_ROOT_SHA.to_string());
    drop(store);
    // every reopen replays the whole structural chain from the active pointer
    let reopened = open_store(&vault_path);
    let verified = reopened.structural_state().expect("verified chain");
    assert_eq!(verified.root_sha256, TERMINAL_ROOT_SHA.to_string());
    let third = reopened.commit_structural(third_bundle(&chain)).expect("third commit");
    assert_eq!(third.generation, 3);
}

#[test]
fn execution_observation_bounded_writes_reject_beyond_stored_event_cap() {
    let parent = TempDir::new().expect("temp parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let capability = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("capability");
    let at_cap = vec![0_u8; STORED_EVENT_MAX_BYTES];
    publisher::put_object(&capability, &"0".repeat(64), &at_cap, STORED_EVENT_MAX_BYTES)
        .expect("write exactly at the cap");
    let over_cap = vec![0_u8; STORED_EVENT_MAX_BYTES + 1];
    let failure = publisher::put_object(&capability, &"1".repeat(64), &over_cap, STORED_EVENT_MAX_BYTES)
        .expect_err("beyond the cap");
    assert_eq!(
        failure,
        ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit)
    );
}
