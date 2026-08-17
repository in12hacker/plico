//! WP2-R2 store self-tests: F06 windows, sibling linearization, exact-G0
//! anchoring, error classification, bounded writes, genesis slot rules; all
//! physical state goes through the frozen seams on tempfile directories.
//! Commit-bundle fixtures live beside `publisher::commit`; genesis staging
//! helpers beside `slots::startup` (all cfg(test)-gated).

use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use super::super::canonical::to_canonical_vec;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::hash::tests::{GENESIS_ROOT_SHA, TERMINAL_ROOT_SHA};
use super::super::tests::golden_chain;
use super::super::{CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES, SEGMENT_MAX_BYTES};
use super::publisher;
use super::publisher::{started_bundle, terminal_bundle, third_bundle};
use super::slots::{put, put_genesis, stage_active};
use super::{FixtureObservationStoreV1, STORED_EVENT_MAX_BYTES};

fn open_store(path: &Path) -> FixtureObservationStoreV1 {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("open"));
    FixtureObservationStoreV1::open_fixture(owner).expect("store")
}

fn open_failure(path: &Path) -> ObservationStoreError {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("reopen"));
    match FixtureObservationStoreV1::open_fixture(owner) {
        Ok(_) => unreachable!("corrupt fixture must fail closed"),
        Err(error) => error,
    }
}

fn capability(path: &Path) -> ExecutionObservationFixtureStorage {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("open"));
    ExecutionObservationFixtureStorage::open(owner).expect("capability")
}

#[test]
fn execution_observation_f06_pre_exchange_keeps_active_without_promotion() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    let first = store.commit_structural(started_bundle(&chain)).expect("commit #1");
    assert_eq!((first.generation, first.event_watermark), (1, 1));
    assert!(matches!(
        store.commit_structural(third_bundle(&chain)),
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::GenerationMismatch
        })
    ));
    store.inject_pre_exchange_failure_once();
    let failed = store.commit_structural(terminal_bundle(&chain)).err();
    assert_eq!(failed, Some(ObservationStoreError::StorageUnavailable));
    let expected_root = first.root_sha256.clone();
    assert_eq!(store.structural_state().expect("state").root_sha256, expected_root);
    drop(store);
    let reopened = open_store(&path);
    let state = reopened.structural_state().expect("reopen state");
    assert_eq!((state.generation, state.root_sha256), (1, expected_root));
    let retried = reopened.commit_structural(terminal_bundle(&chain)).expect("retry");
    assert_eq!(retried.root_sha256, TERMINAL_ROOT_SHA.to_string());
}

#[test]
fn execution_observation_f06_post_exchange_indeterminate_poisons_and_recovers() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("commit #1");
    store.inject_post_exchange_sync_failure_once();
    let indeterminate = store.commit_structural(terminal_bundle(&chain)).err();
    assert_eq!(indeterminate, Some(ObservationStoreError::CommitIndeterminate));
    assert_eq!(store.structural_state().err(), Some(ObservationStoreError::Poisoned));
    let poisoned_commit = store.commit_structural(third_bundle(&chain)).err();
    assert_eq!(poisoned_commit, Some(ObservationStoreError::Poisoned));
    drop(store);
    let reopened = open_store(&path);
    let state = reopened.structural_state().expect("reopen resolves uncertainty");
    assert_eq!(state.root_sha256, TERMINAL_ROOT_SHA.to_string());
    assert_eq!((state.generation, state.event_watermark), (2, 2));
}

/// R02 sequential projection of the barrier race (the concurrent barrier is
/// exercised by the architecture-owned external corpus).
#[test]
fn execution_observation_r02_sibling_commits_linearize_without_rollback() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    let first = store.commit_structural(started_bundle(&chain)).expect("sibling wins");
    let second = store.commit_structural(started_bundle(&chain));
    assert!(matches!(
        second,
        Err(ObservationStoreError::CorruptStore {
            category: CorruptionCategory::GenerationMismatch
        })
    ));
    let state = store.structural_state().expect("active not rolled back");
    assert_eq!(state.root_sha256, first.root_sha256);
    assert_eq!(state.generation, 1);
}

/// Full-chain happy path: two commits, restart replay of the whole chain,
/// and a third commit advancing on the reopened head.
#[test]
fn execution_observation_commit_happy_path_rebuilds_and_advances() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("started");
    let terminal = store.commit_structural(terminal_bundle(&chain)).expect("terminal");
    assert_eq!(terminal.root_sha256, TERMINAL_ROOT_SHA.to_string());
    assert_eq!((terminal.generation, terminal.event_watermark), (2, 2));
    drop(store);
    let reopened = open_store(&path);
    assert_eq!(
        reopened.structural_state().expect("chain").root_sha256,
        TERMINAL_ROOT_SHA
    );
    assert_eq!(
        reopened
            .commit_structural(third_bundle(&chain))
            .expect("third")
            .generation,
        3
    );
}

#[test]
fn execution_observation_genesis_resume_and_foreign_candidate() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    {
        let cap = capability(&path);
        let genesis = put_genesis(&cap);
        cap.inject_pre_exchange_failure_once();
        cap.publish_active(&genesis.pointer_bytes)
            .expect_err("pre-exchange failure");
    }
    drop(open_store(&path));
    let state = open_store(&path).structural_state().expect("resumed exact genesis");
    assert_eq!((state.generation, state.event_watermark), (0, 0));
    assert_eq!(state.root_sha256, GENESIS_ROOT_SHA.to_string());
    let foreign_path = parent.path().join("vault-foreign");
    {
        let cap = capability(&foreign_path);
        let mut foreign = put_genesis(&cap).pointer_bytes;
        *foreign.last_mut().expect("non-empty") ^= b'}';
        cap.inject_pre_exchange_failure_once();
        cap.publish_active(&foreign).expect_err("pre-exchange failure");
    }
    assert_eq!(
        open_failure(&foreign_path),
        ObservationStoreError::corrupt(CorruptionCategory::NoncanonicalPointer)
    );
}

/// Identical pointers in both slots are a slot-relation error (R2-R05).
#[test]
fn execution_observation_same_root_in_both_slots_fails_closed() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    {
        let cap = capability(&path);
        let genesis = put_genesis(&cap);
        cap.publish_active(&genesis.pointer_bytes).expect("first publish");
        cap.publish_active(&genesis.pointer_bytes).expect("second publish");
    }
    assert_eq!(
        open_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::InvalidCandidateState)
    );
}

/// Malformed candidate bytes are a pointer error, classified before any
/// slot-relation judgement (R2-R05 ordering).
#[test]
fn execution_observation_noncanonical_candidate_pointer_classified() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    drop(open_store(&path));
    {
        let cap = capability(&path);
        cap.inject_pre_exchange_failure_once();
        cap.publish_active(b"{}").expect_err("pre-exchange failure");
    }
    assert_eq!(
        open_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::NoncanonicalPointer)
    );
}

#[test]
fn execution_observation_bounded_writes_reject_beyond_stored_event_cap() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let cap = capability(&path);
    let at_cap = vec![0_u8; STORED_EVENT_MAX_BYTES];
    publisher::put_object(&cap, &"0".repeat(64), &at_cap, STORED_EVENT_MAX_BYTES).expect("at cap");
    let over_cap = vec![0_u8; STORED_EVENT_MAX_BYTES + 1];
    let failure =
        publisher::put_object(&cap, &"1".repeat(64), &over_cap, STORED_EVENT_MAX_BYTES).expect_err("beyond cap");
    assert_eq!(
        failure,
        ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit)
    );
}

#[test]
fn execution_observation_max_ordinals_do_not_panic() {
    // red-team P2: extreme stored ordinals must classify, never overflow-panic
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    {
        let cap = capability(&path);
        let mut root = put_genesis(&cap).root;
        root.generation = 9_007_199_254_740_991;
        root.event_watermark = 9_007_199_254_740_991;
        root.previous_root_sha256 = Some("z".repeat(64));
        let root_sha256 = hash::root_sha256(&root).expect("root hash");
        publisher::put_object(&cap, &root_sha256, &to_canonical_vec(&root).expect("r"), ROOT_MAX_BYTES)
            .expect("put max root");
        stage_active(&cap, root_sha256);
    }
    assert!(matches!(
        open_failure(&path),
        ObservationStoreError::CorruptStore { .. } | ObservationStoreError::StorageUnavailable
    ));
}

/// R03: a segment whose event reference is well-formed JSON but an invalid
/// digest must be rejected as corruption BEFORE any CAS dereference.
#[test]
fn execution_observation_r03_doctored_segment_reference_is_corruption() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let chain = golden_chain();
    {
        let cap = capability(&path);
        put_genesis(&cap);
        put(
            &cap,
            &hash::current_view_sha256(&chain.open_view).expect("view hash"),
            to_canonical_vec(&chain.open_view).expect("v"),
            CURRENT_VIEW_MAX_BYTES,
        );
        let mut segment = chain.started_segment.clone();
        segment.event_sha256 = "G".repeat(64);
        let segment_sha256 = hash::segment_sha256(&segment).expect("segment hash");
        publisher::put_object(
            &cap,
            &segment_sha256,
            &to_canonical_vec(&segment).expect("s"),
            SEGMENT_MAX_BYTES,
        )
        .expect("put doctored segment");
        let mut root = chain.started_root.clone();
        root.event_segment_head_sha256 = Some(segment_sha256);
        let root_sha256 = hash::root_sha256(&root).expect("root hash");
        publisher::put_object(&cap, &root_sha256, &to_canonical_vec(&root).expect("r"), ROOT_MAX_BYTES)
            .expect("put root");
        stage_active(&cap, root_sha256);
    }
    assert_eq!(
        open_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch)
    );
}

/// R04: only the recomputed exact G0 is an acceptable chain tail; a
/// hash-self-consistent alternate generation-0 root is broken_root_chain.
#[test]
fn execution_observation_r04_alternate_genesis_rejected() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    {
        let cap = capability(&path);
        let mut root = put_genesis(&cap).root;
        root.committed_at_ms = 1;
        let root_sha256 = hash::root_sha256(&root).expect("alternate hash");
        publisher::put_object(&cap, &root_sha256, &to_canonical_vec(&root).expect("r"), ROOT_MAX_BYTES)
            .expect("put alternate root");
        stage_active(&cap, root_sha256);
    }
    assert_eq!(
        open_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain)
    );
}
