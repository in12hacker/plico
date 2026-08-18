//! WP3A.2-B readonly-adaptation tests: the reader consumes the frozen
//! existing-only read capability. Three classes: opening a reader on a fresh
//! vault mutates nothing and reads an empty ledger; a reader never burns the
//! namespace claim, so a writer opens and publishes on the same vault Arc
//! while readers keep working; and every snapshot taken during concurrent
//! publication is a whole pre- or post-exchange chain, never a torn one.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::cas::PersonalVaultStorage;

use super::super::error::ObservationStoreError;
use super::super::store::FixtureObservationStoreV1;
use super::super::tests::golden_chain;
use super::tests::{attempt_key, second_attempt_started, started_bundle, terminal_bundle};
use super::FixtureObservationReaderV1;

/// SHA-256 fingerprint over every entry under `root`: relative path, file
/// kind, mode, and — for regular files — the bytes. Any create, write, or
/// chmod performed by the code under test changes the digest.
fn vault_fingerprint(root: &Path) -> String {
    fn collect(root: &Path, directory: &Path, entries: &mut Vec<Vec<u8>>) {
        let mut children: Vec<std::path::PathBuf> = std::fs::read_dir(directory)
            .expect("read_dir")
            .map(|entry| entry.expect("entry").path())
            .collect();
        children.sort();
        for child in children {
            let metadata = std::fs::symlink_metadata(&child).expect("symlink_metadata");
            let mut record = format!(
                "{}|{:?}|{:o}|",
                child.strip_prefix(root).expect("prefix").display(),
                metadata.file_type(),
                metadata.permissions().mode(),
            )
            .into_bytes();
            if metadata.is_dir() {
                collect(root, &child, entries);
            } else if metadata.is_file() {
                record.extend(std::fs::read(&child).expect("read"));
            }
            entries.push(record);
        }
    }
    let mut entries = Vec::new();
    collect(root, root, &mut entries);
    entries.sort();
    let mut hasher = Sha256::new();
    hasher.update(format!("entries={}\n", entries.len()));
    for entry in &entries {
        hasher.update(entry);
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

#[test]
fn execution_observation_reader_fresh_vault_reads_empty_and_mutates_nothing() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("vault"));
    let before = vault_fingerprint(&path);

    let reader = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("fresh vault reads as empty");
    let after = vault_fingerprint(&path);

    assert_eq!(before, after, "opening a reader must not mutate the vault");
    let chain = golden_chain();
    assert!(reader
        .read_attempt(&chain.started_event.request.key)
        .expect("read")
        .is_none());
    // The CAS observation namespace (frozen layout name) must not exist.
    assert!(!path.join("execution-observation-fixture-ledger").exists());
}

#[test]
fn execution_observation_reader_never_burns_the_writer_claim() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("vault"));
    let chain = golden_chain();

    // Reader first on the fresh vault: no claim is taken...
    let reader = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("reader");
    assert!(reader
        .read_attempt(&chain.started_event.request.key)
        .expect("read")
        .is_none());

    // ...so the WRITER opener still succeeds on the same Arc. The previous
    // writer-opener seam burned the namespace claim for the whole vault
    // lifecycle here (WP3A.1 P1-2).
    let store = FixtureObservationStoreV1::open_fixture(owner.clone()).expect("writer opens after reader");
    store.commit_structural(started_bundle(&chain)).expect("started");
    store.commit_structural(terminal_bundle(&chain)).expect("terminal");

    // While the writer is alive (claim held), a reader on the same Arc
    // still replays and returns the committed chain.
    let under_claim = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("reader under live claim");
    let observation = under_claim
        .read_attempt(&chain.started_event.request.key)
        .expect("read")
        .expect("present");
    assert_eq!(observation.key, chain.started_event.request.key);
    assert!(observation.terminal_receipt.is_some());
}

#[test]
fn execution_observation_reader_snapshots_are_whole_during_publication() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("vault"));
    let chain = golden_chain();
    let store = FixtureObservationStoreV1::open_fixture(owner.clone()).expect("store");
    store.commit_structural(started_bundle(&chain)).expect("started");
    store.commit_structural(terminal_bundle(&chain)).expect("terminal");
    let second_started = second_attempt_started(&chain);

    let execution_id = chain.started_event.request.key.execution_id;
    let first = attempt_key(execution_id, 1);
    let second = attempt_key(execution_id, 2);

    // Deterministic pre-publication snapshot: attempt 1 terminal, attempt 2
    // absent. The barrier guarantees both threads are alive before the
    // single RENAME_EXCHANGE publication races the reader loop below.
    let pre = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("pre-publication snapshot");
    assert!(pre
        .read_attempt(&first)
        .expect("read")
        .expect("present")
        .terminal_receipt
        .is_some());
    assert!(pre.read_attempt(&second).expect("read").is_none());

    let barrier = Arc::new(Barrier::new(2));
    let writer = Arc::new(Mutex::new(store));
    let handle = {
        let writer = Arc::clone(&writer);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            writer
                .lock()
                .expect("writer lock")
                .commit_structural(second_started)
                .expect("second started publishes");
        })
    };
    barrier.wait();

    // Every snapshot racing the publication must be a whole chain: attempt 1
    // stays terminal, attempt 2 is either absent or exactly its Started
    // state, and no open ever fails. A torn active pointer would surface as
    // a corrupt_store error or an inconsistent attempt here. The loop runs at
    // least MIN_RACING_OPENS times so the publication window is provably
    // exercised, and paces itself with a short sleep: a hot spin would
    // flood the disk during the writer's fsync window and trip unrelated
    // deadline-sensitive tests sharing the full-suite run.
    const MIN_RACING_OPENS: u32 = 16;
    let mut opens = 0_u32;
    loop {
        let reader = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("whole snapshot");
        assert!(reader
            .read_attempt(&first)
            .expect("read")
            .expect("attempt 1 terminal in every whole snapshot")
            .terminal_receipt
            .is_some());
        if let Some(observation) = reader.read_attempt(&second).expect("read") {
            assert!(
                observation.terminal_receipt.is_none(),
                "attempt 2 can only ever be observed in its Started state"
            );
        }
        opens += 1;
        if opens >= MIN_RACING_OPENS && handle.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    handle.join().expect("writer thread");
    assert!(opens >= MIN_RACING_OPENS, "race loop must not be vacuous");

    // Deterministic settled snapshot: attempt 2 is present and open.
    let settled = FixtureObservationReaderV1::open_fixture(owner.clone()).expect("settled snapshot");
    assert!(settled
        .read_attempt(&second)
        .expect("read")
        .expect("attempt 2 present after publication")
        .terminal_receipt
        .is_none());
    assert!(settled
        .read_attempt(&first)
        .expect("read")
        .expect("present")
        .terminal_receipt
        .is_some());
}

#[test]
fn execution_observation_reader_damaged_topology_fails_closed_without_repair() {
    let parent = TempDir::new().expect("temp parent");
    let path = parent.path().join("vault");
    let owner = Arc::new(PersonalVaultStorage::open(&path, None).expect("vault"));
    drop(FixtureObservationStoreV1::open_fixture(owner.clone()).expect("store creates the namespace"));

    // Widen the objects directory mode: the readonly open must fail closed
    // through the reader (mapped to storage_unavailable, the same open-phase
    // classification the WP2 corpus pins for the store) and must not repair
    // the damaged mode.
    let objects = path.join("execution-observation-fixture-ledger").join("objects");
    std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(0o755)).expect("damage mode");

    let failure = match FixtureObservationReaderV1::open_fixture(owner.clone()) {
        Ok(_) => unreachable!("damaged topology must fail closed"),
        Err(error) => error,
    };
    assert_eq!(failure, ObservationStoreError::StorageUnavailable);

    let mode = std::fs::metadata(&objects).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the reader must never repair stored topology");
}
