//! WP3A reader self-tests: the ADR-0009 acceptance matrix. Physical chains
//! are built through the sealed store seams on tempfile vaults; the reader
//! then replays them independently. Reducer-level adversarial inputs are fed
//! directly to the pure reducer.

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::hash::tests::{
    STARTED_EVENT_SHA, STARTED_REQUEST_SHA, TERMINAL_EVENT_SHA, TERMINAL_RECORDED_AT_MS, TERMINAL_REQUEST_SHA,
    TERMINAL_ROOT_SHA, TERMINAL_SEGMENT_SHA,
};
use super::super::ids::ExecutionAttemptKeyV1;
use super::super::model::*;
use super::super::store::{FixtureObservationStoreV1, FixtureStoredEventV1, FixtureStructuralCommitV1};
use super::super::tests::{golden_chain, GoldenChain};
use super::super::EVENTS_MAX;
use super::reducer::{reduce, ReducibleEventV1, ReducibleKindV1};
use super::FixtureObservationReaderV1;

struct LocalGenesis {
    view: FixtureCurrentViewV1,
    root: FixtureLedgerRootV1,
}

/// Writes a forged view+root pair and publishes its pointer through the
/// sealed seam (shared by the tamper and alternate-genesis corpora).
fn stage_forged(
    cap: &ExecutionObservationFixtureStorage,
    view: &FixtureCurrentViewV1,
    root: &FixtureLedgerRootV1,
) -> String {
    let view_sha = hash::current_view_sha256(view).expect("view hash");
    let mut root = root.clone();
    root.current_view_sha256 = view_sha.clone();
    let root_sha = hash::root_sha256(&root).expect("root hash");
    cap.put_immutable_bounded(
        &view_sha,
        &super::super::canonical::to_canonical_vec(view).expect("v"),
        super::super::CURRENT_VIEW_MAX_BYTES as u64,
    )
    .expect("put view");
    cap.put_immutable_bounded(
        &root_sha,
        &super::super::canonical::to_canonical_vec(&root).expect("r"),
        super::super::ROOT_MAX_BYTES as u64,
    )
    .expect("put root");
    let pointer = FixtureActivePointerV1 {
        schema: POINTER_SCHEMA.into(),
        root_sha256: root_sha.clone(),
    };
    cap.publish_active(&super::super::canonical::to_canonical_vec(&pointer).expect("p"))
        .expect("publish");
    root_sha
}

fn slots_genesis() -> LocalGenesis {
    let view = FixtureCurrentViewV1 {
        schema: CURRENT_VIEW_SCHEMA.into(),
        attestation_state: ATTESTATION_STATE.into(),
        generation: 0,
        event_watermark: 0,
        attempts: Vec::new(),
    };
    let root = FixtureLedgerRootV1 {
        schema: ROOT_SCHEMA.into(),
        trust_class: TRUST_CLASS.into(),
        generation: 0,
        previous_root_sha256: None,
        event_segment_head_sha256: None,
        event_watermark: 0,
        current_view_sha256: hash::current_view_sha256(&view).expect("genesis view hash"),
        committed_at_ms: 0,
    };
    LocalGenesis { view, root }
}

fn open_reader(path: &Path) -> FixtureObservationReaderV1 {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("open"));
    FixtureObservationReaderV1::open_fixture(owner).expect("reader")
}

fn open_reader_failure(path: &Path) -> ObservationStoreError {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("reopen"));
    match FixtureObservationReaderV1::open_fixture(owner) {
        Ok(_) => unreachable!("corrupt fixture must fail closed"),
        Err(error) => error,
    }
}

fn attempt_key(execution_id: super::super::ids::CanonicalUuid, attempt: u32) -> ExecutionAttemptKeyV1 {
    ExecutionAttemptKeyV1 {
        execution_id,
        attempt: NonZeroU32::new(attempt).expect("nonzero"),
    }
}

fn open_store(path: &Path) -> FixtureObservationStoreV1 {
    let owner = Arc::new(PersonalVaultStorage::open(path, None).expect("open"));
    FixtureObservationStoreV1::open_fixture(owner).expect("store")
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

/// A second-attempt Started bundle (same execution, attempt 2) for
/// generation 3, with every binding rehashed like the publisher does. The
/// terminal-after-started machinery is key-independent and already covered by
/// the reducer corpus and attempt 1, so attempt 2 stays Open here.
fn second_attempt_started(chain: &GoldenChain) -> FixtureStructuralCommitV1 {
    let key = ExecutionAttemptKeyV1 {
        execution_id: chain.started_event.request.key.execution_id,
        attempt: NonZeroU32::new(2).expect("nonzero"),
    };
    let mut event = chain.started_event.clone();
    event.request.key = key;
    event.sequence = 3;
    event.root_generation = 3;
    event.recorded_at_ms = TERMINAL_RECORDED_AT_MS + 1;
    event.request_sha256 = hash::started_request_sha256(&event.request).expect("request hash");
    let event_sha = hash::started_event_sha256(&event).expect("event hash");
    let mut segment = chain.started_segment.clone();
    segment.first_sequence = 3;
    segment.last_sequence = 3;
    segment.previous_segment_sha256 = Some(TERMINAL_SEGMENT_SHA.into());
    segment.event_sha256 = event_sha.clone();
    let segment_sha = hash::segment_sha256(&segment).expect("segment hash");
    let mut view = chain.terminal_view.clone();
    view.generation = 3;
    view.event_watermark = 3;
    view.attempts.push(FixtureAttemptViewV1 {
        key,
        attestation_state: ATTESTATION_STATE.into(),
        started_request_sha256: event.request_sha256.clone(),
        started_event_sha256: event_sha,
        terminal_request_sha256: None,
        terminal_event_sha256: None,
    });
    view.attempts.sort_by(|left, right| {
        (left.key.execution_id.as_bytes(), left.key.attempt.get())
            .cmp(&(right.key.execution_id.as_bytes(), right.key.attempt.get()))
    });
    let mut root = chain.terminal_root.clone();
    root.generation = 3;
    root.event_watermark = 3;
    root.previous_root_sha256 = Some(TERMINAL_ROOT_SHA.into());
    root.event_segment_head_sha256 = Some(segment_sha.clone());
    root.current_view_sha256 = hash::current_view_sha256(&view).expect("view hash");
    root.committed_at_ms = TERMINAL_RECORDED_AT_MS + 1;
    FixtureStructuralCommitV1 {
        event: FixtureStoredEventV1::Started(event),
        segment,
        current_view: view,
        root,
    }
}

/// One plain reducible event with overridable key/kind (synthetic digests).
fn reducible_event(
    chain: &GoldenChain,
    sequence: u64,
    key: ExecutionAttemptKeyV1,
    kind: ReducibleKindV1,
) -> ReducibleEventV1 {
    ReducibleEventV1 {
        sequence,
        root_generation: sequence,
        root_sha256: "a".repeat(64),
        recorded_at_ms: chain.started_event.recorded_at_ms,
        event_sha256: "b".repeat(64),
        request_sha256: "c".repeat(64),
        key,
        kind,
    }
}

fn started_kind(chain: &GoldenChain) -> ReducibleKindV1 {
    let request = &chain.started_event.request;
    ReducibleKindV1::Started {
        policy_sha256: request.policy_sha256.clone(),
        runtime_sha256: request.runtime_sha256.clone(),
    }
}

fn terminal_kind(chain: &GoldenChain) -> ReducibleKindV1 {
    let request = &chain.terminal_event.request;
    ReducibleKindV1::Terminal {
        policy_sha256: request.policy_sha256.clone(),
        runtime_sha256: request.runtime_sha256.clone(),
    }
}

#[test]
fn execution_observation_reader_empty_chain_reads_none() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    drop(open_store(&path));
    let reader = open_reader(&path);
    let chain = golden_chain();
    let result = reader.read_attempt(&chain.started_event.request.key).expect("read");
    assert!(result.is_none());
}

#[test]
fn execution_observation_reader_open_attempt_has_null_terminal() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("started");
    drop(store);
    let reader = open_reader(&path);
    let observation = reader
        .read_attempt(&chain.started_event.request.key)
        .expect("read")
        .expect("present");
    assert_eq!(observation.attestation_state, ATTESTATION_STATE);
    assert_eq!(observation.started_receipt.sequence, 1);
    assert_eq!(observation.started_receipt.root_generation, 1);
    assert_eq!(observation.started_receipt.request_sha256, STARTED_REQUEST_SHA);
    assert_eq!(observation.started_receipt.event_sha256, STARTED_EVENT_SHA);
    assert!(observation.terminal_receipt.is_none());
}

#[test]
fn execution_observation_reader_terminal_attempt_and_restart_determinism() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let store = open_store(&path);
    let chain = golden_chain();
    store.commit_structural(started_bundle(&chain)).expect("started");
    store.commit_structural(terminal_bundle(&chain)).expect("terminal");
    drop(store);
    let first = open_reader(&path);
    let second = open_reader(&path);
    for reader in [&first, &second] {
        let observation = reader
            .read_attempt(&chain.started_event.request.key)
            .expect("read")
            .expect("present");
        let terminal = observation.terminal_receipt.as_ref().expect("terminal");
        assert_eq!(terminal.sequence, 2);
        assert_eq!(terminal.root_generation, 2);
        assert_eq!(terminal.request_sha256, TERMINAL_REQUEST_SHA);
        assert_eq!(terminal.event_sha256, TERMINAL_EVENT_SHA);
        assert_eq!(observation.started_receipt.request_sha256, STARTED_REQUEST_SHA);
        assert_eq!(observation.started_receipt.event_sha256, STARTED_EVENT_SHA);
        assert_ne!(observation.started_receipt.root_sha256, terminal.root_sha256);
    }
    assert_eq!(
        first.read_attempt(&chain.started_event.request.key).expect("read"),
        second.read_attempt(&chain.started_event.request.key).expect("read"),
    );
}

#[test]
fn execution_observation_reader_same_execution_multiple_attempts() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let chain = golden_chain();
    let store = open_store(&path);
    store.commit_structural(started_bundle(&chain)).expect("started");
    store.commit_structural(terminal_bundle(&chain)).expect("terminal");
    store
        .commit_structural(second_attempt_started(&chain))
        .expect("second started");
    drop(store);
    let reader = open_reader(&path);
    let execution_id = chain.started_event.request.key.execution_id;
    let mut seen_digests: Vec<String> = Vec::new();
    // attempt 1 is Terminal; attempt 2 (same execution) is Open at sequence 3
    for (attempt, started_sequence, terminal_sequence) in [(1_u32, 1_u64, Some(2_u64)), (2, 3, None)] {
        let key = attempt_key(execution_id, attempt);
        let observation = reader.read_attempt(&key).expect("read").expect("present");
        assert_eq!(observation.key, key);
        assert_eq!(observation.started_receipt.sequence, started_sequence);
        assert_eq!(
            observation.terminal_receipt.as_ref().map(|r| r.sequence),
            terminal_sequence
        );
        let mut receipts = vec![&observation.started_receipt];
        receipts.extend(observation.terminal_receipt.iter());
        for receipt in receipts {
            for digest in [&receipt.request_sha256, &receipt.event_sha256, &receipt.root_sha256] {
                assert_eq!(digest.len(), 64);
                assert!(digest.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()));
                assert!(
                    !seen_digests.contains(digest),
                    "receipt digests must be attempt-distinct"
                );
                seen_digests.push(digest.clone());
            }
        }
    }
    // absent attempt on a committed chain reads as None
    assert!(reader
        .read_attempt(&attempt_key(execution_id, 9))
        .expect("read")
        .is_none());
}

/// A hash-self-consistent alternate generation-0 root is rejected by the
/// reader's own replay walk, not only by the store's loader.
#[test]
fn execution_observation_reader_rejects_alternate_genesis() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    drop(open_store(&path));
    {
        let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("open"));
        let cap = ExecutionObservationFixtureStorage::open(owner).expect("capability");
        let genesis = slots_genesis();
        let mut root = genesis.root.clone();
        root.committed_at_ms = 1;
        stage_forged(&cap, &genesis.view, &root);
    }
    assert_eq!(
        open_reader_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::BrokenRootChain)
    );
}

/// WP3A.1 adversarial A: modify a stored event's persisted root_generation
/// and recompute every downstream hash (event -> segment -> view -> root ->
/// pointer). The chain is fully hash-self-consistent; only the reader's
/// stamp-binding checks can catch it, and they must (GenerationMismatch).
#[test]
fn execution_observation_reader_tampered_stamp_generation_mismatch() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let chain = golden_chain();
    {
        let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("open"));
        let cap = ExecutionObservationFixtureStorage::open(owner).expect("capability");
        // genesis + tampered Started at generation 1
        let genesis = slots_genesis();
        cap.put_immutable_bounded(
            &hash::current_view_sha256(&genesis.view).expect("gv"),
            &super::super::canonical::to_canonical_vec(&genesis.view).expect("v"),
            super::super::CURRENT_VIEW_MAX_BYTES as u64,
        )
        .expect("put genesis view");
        cap.put_immutable_bounded(
            &hash::root_sha256(&genesis.root).expect("gr"),
            &super::super::canonical::to_canonical_vec(&genesis.root).expect("r"),
            super::super::ROOT_MAX_BYTES as u64,
        )
        .expect("put genesis root");
        let mut event = chain.started_event.clone();
        event.root_generation = 2; // the lie: stamp claims generation 2
        event.request_sha256 = hash::started_request_sha256(&event.request).expect("req");
        let event_sha = hash::started_event_sha256(&event).expect("event hash");
        let mut segment = chain.started_segment.clone();
        segment.event_sha256 = event_sha.clone();
        let segment_sha = hash::segment_sha256(&segment).expect("segment hash");
        let mut view = chain.open_view.clone();
        view.attempts[0].started_request_sha256 = event.request_sha256.clone();
        view.attempts[0].started_event_sha256 = event_sha.clone();
        let view_sha = hash::current_view_sha256(&view).expect("view hash");
        let mut root = chain.started_root.clone();
        root.event_segment_head_sha256 = Some(segment_sha.clone());
        root.current_view_sha256 = view_sha.clone();
        root.previous_root_sha256 = Some(hash::root_sha256(&genesis.root).expect("gr"));
        let root_sha = hash::root_sha256(&root).expect("root hash");
        cap.put_immutable_bounded(
            &event_sha,
            &super::super::canonical::to_canonical_vec(&event).expect("e"),
            (super::super::validation::CANONICAL_REQUEST_MAX_BYTES + 4_096) as u64,
        )
        .expect("put event");
        cap.put_immutable_bounded(
            &segment_sha,
            &super::super::canonical::to_canonical_vec(&segment).expect("s"),
            super::super::SEGMENT_MAX_BYTES as u64,
        )
        .expect("put segment");
        cap.put_immutable_bounded(
            &view_sha,
            &super::super::canonical::to_canonical_vec(&view).expect("v"),
            super::super::CURRENT_VIEW_MAX_BYTES as u64,
        )
        .expect("put view");
        cap.put_immutable_bounded(
            &root_sha,
            &super::super::canonical::to_canonical_vec(&root).expect("r"),
            super::super::ROOT_MAX_BYTES as u64,
        )
        .expect("put root");
        let pointer = FixtureActivePointerV1 {
            schema: POINTER_SCHEMA.into(),
            root_sha256: root_sha,
        };
        cap.publish_active(&super::super::canonical::to_canonical_vec(&pointer).expect("p"))
            .expect("publish");
    }
    assert_eq!(
        open_reader_failure(&path),
        ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch)
    );
}

#[test]
fn execution_observation_reducer_rejects_sequence_gap() {
    let chain = golden_chain();
    let key = chain.started_event.request.key;
    let events = vec![
        reducible_event(&chain, 1, key, started_kind(&chain)),
        reducible_event(&chain, 3, key, terminal_kind(&chain)),
    ];
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::SequenceGap)
    );
}

#[test]
fn execution_observation_reducer_rejects_duplicate_started() {
    let chain = golden_chain();
    let key = chain.started_event.request.key;
    let events = vec![
        reducible_event(&chain, 1, key, started_kind(&chain)),
        reducible_event(&chain, 2, key, started_kind(&chain)),
    ];
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::DuplicateStarted)
    );
}

#[test]
fn execution_observation_reducer_rejects_terminal_without_started() {
    let chain = golden_chain();
    let key = chain.started_event.request.key;
    let events = vec![reducible_event(&chain, 1, key, terminal_kind(&chain))];
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition)
    );
}

#[test]
fn execution_observation_reducer_rejects_second_terminal_and_rebind() {
    let chain = golden_chain();
    let key = chain.started_event.request.key;
    let events = vec![
        reducible_event(&chain, 1, key, started_kind(&chain)),
        reducible_event(&chain, 2, key, terminal_kind(&chain)),
        reducible_event(&chain, 3, key, terminal_kind(&chain)),
    ];
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::DuplicateTerminal)
    );
    let mut rebound = terminal_kind(&chain);
    if let ReducibleKindV1::Terminal { policy_sha256, .. } = &mut rebound {
        policy_sha256.replace_range(0..1, "9");
    }
    let events = vec![
        reducible_event(&chain, 1, key, started_kind(&chain)),
        reducible_event(&chain, 2, key, rebound),
    ];
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition)
    );
}

#[test]
fn execution_observation_reducer_enforces_event_cap_and_generation_binding() {
    let chain = golden_chain();
    let key = chain.started_event.request.key;
    let mut events: Vec<ReducibleEventV1> = Vec::new();
    for sequence in 1..=EVENTS_MAX + 1 {
        events.push(reducible_event(&chain, sequence, key, started_kind(&chain)));
    }
    assert_eq!(
        reduce(events).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit)
    );
    let mut mismatched = reducible_event(&chain, 1, key, started_kind(&chain));
    mismatched.root_generation = 7;
    assert_eq!(
        reduce(vec![mismatched]).unwrap_err(),
        ObservationStoreError::corrupt(CorruptionCategory::GenerationMismatch)
    );
    // the accept side: exactly EVENTS_MAX distinct attempts reduce cleanly
    let execution_id = chain.started_event.request.key.execution_id;
    let full: Vec<ReducibleEventV1> = (1..=EVENTS_MAX)
        .map(|sequence| {
            reducible_event(
                &chain,
                sequence,
                attempt_key(execution_id, sequence as u32),
                started_kind(&chain),
            )
        })
        .collect();
    assert_eq!(
        reduce(full).expect("boundary ledger reduces").len(),
        EVENTS_MAX as usize
    );
}
