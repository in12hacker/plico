use std::sync::Arc;

use super::*;

#[test]
fn execution_observation_storage_is_lazy_and_single_claimed() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    assert!(!vault_path.join(DIRECTORY).exists());

    let _storage = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("first fixed namespace claim");
    assert!(vault_path.join(DIRECTORY).is_dir());
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(vault),
        Err(LedgerStorageOpenError::NamespaceAlreadyClaimed)
    ));
}

#[test]
fn execution_observation_namespace_rejects_the_generic_ledger_entrypoint() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = PersonalVaultStorage::open(&vault_path, None).expect("vault");

    assert!(matches!(
        vault.immutable_ledger(ImmutableLedgerNamespace::ExecutionObservationFixture),
        Err(LedgerStorageOpenError::Io(error))
            if error.kind() == std::io::ErrorKind::InvalidInput
    ));
    assert!(!vault_path.join(DIRECTORY).exists());
}

#[test]
fn execution_observation_storage_bounds_collision_and_pre_exchange() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(vault).expect("storage");
    let hash = "a".repeat(64);

    storage.put_immutable_bounded(&hash, b"one", 3).expect("bounded put");
    storage
        .put_immutable_bounded(&hash, b"one", 3)
        .expect("identical retry");
    assert_eq!(storage.get_immutable_bounded(&hash, 3).unwrap(), b"one");
    assert_eq!(storage.list_immutable_hashes_bounded(1).unwrap(), vec![hash.clone()]);
    assert_eq!(
        storage
            .put_immutable_bounded(&hash, b"two", 3)
            .expect_err("collision")
            .kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert_eq!(
        storage
            .put_immutable_bounded(&"b".repeat(64), b"four", 3)
            .expect_err("bounded before write")
            .kind(),
        std::io::ErrorKind::InvalidInput
    );

    storage.publish_active(b"old").expect("initial publish");
    storage.inject_pre_exchange_failure_once();
    assert!(storage.publish_active(b"new").is_err());
    assert_eq!(storage.read_active_bounded(3).unwrap(), Some(b"old".to_vec()));
    assert_eq!(storage.read_candidate_bounded(3).unwrap(), Some(b"new".to_vec()));
    storage.inject_post_exchange_sync_failure_once();
    assert!(matches!(
        storage.publish_active(b"new"),
        Err(LedgerStorageError::PublishedButUnsynced(_))
    ));
}

#[test]
fn execution_observation_collision_retry_never_uses_an_unbounded_read() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(vault).expect("storage");
    let hash = "c".repeat(64);

    storage
        .put_immutable_bounded(&hash, b"four", 4)
        .expect("initial bounded put");
    assert_eq!(
        storage
            .put_immutable_bounded(&hash, b"two", 3)
            .expect_err("collision comparison remains bounded")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(storage.get_immutable_bounded(&hash, 4).expect("unchanged"), b"four");
}

#[test]
fn execution_observation_collision_is_injected_after_prepare_before_noclobber() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(vault).expect("storage");
    let hash = "e".repeat(64);

    storage.inject_bounded_collision_before_noclobber_once(b"oversized");
    assert_eq!(
        storage
            .put_immutable_bounded(&hash, b"x", 1)
            .expect_err("injected collision must be reread through the bound")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(
        storage.get_immutable_bounded(&hash, 16).expect("injected winner"),
        b"oversized"
    );
}

#[test]
fn execution_observation_concurrent_collision_has_one_atomic_winner() {
    use std::sync::Barrier;

    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let storage = Arc::new(ExecutionObservationFixtureStorage::open(vault).expect("storage"));
    let barrier = Arc::new(Barrier::new(3));
    let hash = "d".repeat(64);
    let mut writers = Vec::new();
    for bytes in [b"one".as_slice(), b"two".as_slice()] {
        let storage = Arc::clone(&storage);
        let barrier = Arc::clone(&barrier);
        let hash = hash.clone();
        writers.push(std::thread::spawn(move || {
            barrier.wait();
            storage.put_immutable_bounded(&hash, bytes, 3)
        }));
    }
    barrier.wait();
    let results: Vec<_> = writers
        .into_iter()
        .map(|writer| writer.join().expect("writer thread"))
        .collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .filter(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
            .count(),
        1
    );
    let stored = storage.get_immutable_bounded(&hash, 3).expect("winner bytes");
    assert!(stored == b"one" || stored == b"two");
}

#[cfg(unix)]
#[test]
fn execution_observation_storage_rejects_non_private_existing_topology() {
    use std::os::unix::fs::PermissionsExt;

    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let directory = vault_path.join(DIRECTORY);
    fs::create_dir(&directory).unwrap();
    fs::create_dir(directory.join("objects")).unwrap();
    fs::create_dir(directory.join("roots")).unwrap();
    fs::write(directory.join("roots/active"), []).unwrap();
    fs::write(directory.join("roots/candidate"), []).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(matches!(
        ExecutionObservationFixtureStorage::open(vault),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert_eq!(fs::metadata(&directory).unwrap().permissions().mode() & 0o777, 0o755);
}

#[test]
fn execution_observation_storage_never_repairs_missing_or_special_slots() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("storage");
    drop(storage);
    let active = vault_path.join(DIRECTORY).join("roots/active");
    fs::remove_file(&active).expect("remove active");
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(Arc::clone(&vault)),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert!(!active.exists());

    fs::create_dir(&active).expect("special active directory");
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(vault),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert!(active.is_dir());
}

#[cfg(unix)]
#[test]
fn execution_observation_storage_rejects_symlinked_slot_and_object_directory() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let storage = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("storage");
    drop(storage);
    let directory = vault_path.join(DIRECTORY);
    let active = directory.join("roots/active");
    let outside = parent.path().join("outside");
    fs::write(&outside, []).expect("outside pointer");
    fs::remove_file(&active).expect("remove active");
    symlink(&outside, &active).expect("symlink active");
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(Arc::clone(&vault)),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert!(fs::symlink_metadata(&active).unwrap().file_type().is_symlink());

    fs::remove_file(&active).expect("remove active symlink");
    fs::write(&active, []).expect("restore active");
    fs::set_permissions(&active, fs::Permissions::from_mode(0o600)).expect("restore private active mode");
    let objects = directory.join("objects");
    let outside_objects = parent.path().join("outside-objects");
    fs::create_dir(&outside_objects).expect("outside objects");
    fs::remove_dir(&objects).expect("remove empty objects");
    symlink(&outside_objects, &objects).expect("symlink objects");
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(vault),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert!(fs::symlink_metadata(&objects).unwrap().file_type().is_symlink());
    assert!(fs::read_dir(&outside_objects).unwrap().next().is_none());
}

#[cfg(unix)]
fn vault_tree_fingerprint(root: &Path) -> Vec<(String, u32, u64, Option<String>)> {
    use std::os::unix::fs::PermissionsExt;

    let mut fingerprint = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = entry.expect("vault walk entry");
        let metadata = fs::symlink_metadata(entry.path()).expect("vault entry metadata");
        let digest = if metadata.file_type().is_file() {
            use sha2::Digest;

            let bytes = fs::read(entry.path()).expect("vault file bytes");
            Some(format!("{:x}", sha2::Sha256::digest(bytes)))
        } else {
            None
        };
        fingerprint.push((
            entry
                .path()
                .strip_prefix(root)
                .expect("vault prefix")
                .display()
                .to_string(),
            metadata.permissions().mode() & 0o777,
            metadata.len(),
            digest,
        ));
    }
    fingerprint
}

#[cfg(unix)]
#[test]
fn execution_observation_readonly_fresh_vault_is_none_and_zero_mutation() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let before = vault_tree_fingerprint(&vault_path);

    let seen_absent = vault
        .with_existing_execution_observation_readonly(|view| view.is_none())
        .expect("fresh vault readonly inspection");
    assert!(seen_absent);
    assert!(vault
        .with_existing_execution_observation_readonly(|view| view.is_none())
        .expect("second fresh vault readonly inspection"));

    assert_eq!(vault_tree_fingerprint(&vault_path), before);
    assert!(!vault_path.join(DIRECTORY).exists());
}

#[test]
fn execution_observation_readonly_then_writer_same_arc() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));

    assert!(vault
        .with_existing_execution_observation_readonly(|view| view.is_none())
        .expect("readonly before writer"));

    let _writer = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("writer after readonly");
    assert!(matches!(
        ExecutionObservationFixtureStorage::open(vault),
        Err(LedgerStorageOpenError::NamespaceAlreadyClaimed)
    ));
}

#[test]
fn execution_observation_readonly_works_while_writer_claim_held() {
    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let writer = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("writer claim");
    let hash = "9".repeat(64);
    writer
        .put_immutable_bounded(&hash, b"object-bytes", 12)
        .expect("object");
    writer.publish_active(b"pointer-bytes").expect("publish");

    let observed = vault
        .with_existing_execution_observation_readonly(|view| {
            let view = view.expect("existing namespace view");
            (
                view.read_active_bounded(64).expect("active read under claim"),
                view.get_immutable_bounded(&hash, 12).expect("object read under claim"),
            )
        })
        .expect("readonly under writer claim");
    assert_eq!(observed.0, Some(b"pointer-bytes".to_vec()));
    assert_eq!(observed.1, b"object-bytes");
}

#[test]
fn execution_observation_readonly_exchange_race_sees_whole_pointers() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let parent = tempfile::tempdir().expect("temporary parent");
    let vault = Arc::new(PersonalVaultStorage::open(&parent.path().join("vault"), None).expect("vault"));
    let writer = Arc::new(ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("writer"));
    let old_hash = "1".repeat(64);
    let new_hash = "2".repeat(64);
    writer
        .put_immutable_bounded(&old_hash, b"old-object", 10)
        .expect("old object");
    writer
        .put_immutable_bounded(&new_hash, b"new-object", 10)
        .expect("new object");
    let old_pointer = b"old-pointer".to_vec();
    let new_pointer = b"new-pointer".to_vec();
    writer.publish_active(&old_pointer).expect("initial publish");

    let writer_done = Arc::new(AtomicBool::new(false));
    let publisher = {
        let writer = Arc::clone(&writer);
        let writer_done = Arc::clone(&writer_done);
        let publisher_old = old_pointer.clone();
        let publisher_new = new_pointer.clone();
        std::thread::spawn(move || {
            let mut failures = 0usize;
            for round in 0..200u32 {
                let pointer = if round % 2 == 0 { &publisher_new } else { &publisher_old };
                if writer.publish_active(pointer).is_err() {
                    failures += 1;
                }
            }
            writer_done.store(true, Ordering::SeqCst);
            failures
        })
    };

    let observed = vault
        .with_existing_execution_observation_readonly(|view| {
            let view = view.expect("existing namespace view");
            let mut old_seen = 0usize;
            let mut new_seen = 0usize;
            while !writer_done.load(Ordering::SeqCst) {
                let pointer = view
                    .read_active_bounded(64)
                    .expect("bounded active read during exchange")
                    .expect("published pointer");
                assert!(
                    pointer == old_pointer || pointer == new_pointer,
                    "torn active pointer observed during exchange"
                );
                if pointer == old_pointer {
                    old_seen += 1;
                } else {
                    new_seen += 1;
                }
                assert_eq!(
                    view.get_immutable_bounded(&old_hash, 10).expect("old object"),
                    b"old-object"
                );
                assert_eq!(
                    view.get_immutable_bounded(&new_hash, 10).expect("new object"),
                    b"new-object"
                );
                std::thread::yield_now();
            }
            (old_seen, new_seen)
        })
        .expect("readonly during exchanges");
    let failures = publisher.join().expect("publisher thread");
    assert_eq!(failures, 0);
    assert!(
        observed.0 > 0 && observed.1 > 0,
        "race window not exercised: {observed:?}"
    );
}

#[cfg(unix)]
#[test]
fn execution_observation_readonly_damaged_topology_fails_closed_without_repair() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let parent = tempfile::tempdir().expect("temporary parent");
    let vault_path = parent.path().join("vault");
    let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).expect("vault"));
    let writer = ExecutionObservationFixtureStorage::open(Arc::clone(&vault)).expect("writer topology");
    drop(writer);
    let directory = vault_path.join(DIRECTORY);

    let active = directory.join("roots/active");
    let outside = parent.path().join("outside-pointer");
    fs::write(&outside, []).expect("outside pointer");
    fs::remove_file(&active).expect("remove active");
    symlink(&outside, &active).expect("symlink active");
    let damaged = vault_tree_fingerprint(&vault_path);
    assert!(matches!(
        vault.with_existing_execution_observation_readonly(|view| view.is_none()),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert_eq!(vault_tree_fingerprint(&vault_path), damaged);
    assert!(fs::symlink_metadata(&active)
        .expect("symlink preserved")
        .file_type()
        .is_symlink());

    fs::remove_file(&active).expect("remove active symlink");
    fs::write(&active, []).expect("restore active");
    fs::set_permissions(&active, fs::Permissions::from_mode(0o600)).expect("restore active mode");

    let candidate = directory.join("roots/candidate");
    fs::remove_file(&candidate).expect("remove candidate");
    let damaged = vault_tree_fingerprint(&vault_path);
    assert!(matches!(
        vault.with_existing_execution_observation_readonly(|view| view.is_none()),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert_eq!(vault_tree_fingerprint(&vault_path), damaged);
    assert!(!candidate.exists());

    fs::write(&candidate, []).expect("restore candidate");
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o600)).expect("restore candidate mode");
    let objects = directory.join("objects");
    fs::set_permissions(&objects, fs::Permissions::from_mode(0o755)).expect("permissive objects");
    let damaged = vault_tree_fingerprint(&vault_path);
    assert!(matches!(
        vault.with_existing_execution_observation_readonly(|view| view.is_none()),
        Err(LedgerStorageOpenError::Io(_))
    ));
    assert_eq!(vault_tree_fingerprint(&vault_path), damaged);
    assert_eq!(
        fs::metadata(&objects).expect("objects metadata").permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn execution_observation_readonly_surface_pins_frozen_api() {
    let read_active_bounded: fn(
        &ExistingExecutionObservationReadOnly<'static>,
        u64,
    ) -> std::io::Result<Option<Vec<u8>>> = <ExistingExecutionObservationReadOnly<'static>>::read_active_bounded;
    let get_immutable_bounded: fn(
        &ExistingExecutionObservationReadOnly<'static>,
        &str,
        u64,
    ) -> std::io::Result<Vec<u8>> = <ExistingExecutionObservationReadOnly<'static>>::get_immutable_bounded;
    // The pinned pair is the complete read surface of the frozen view; any
    // added capability (candidate, list, put, publish, paths) shows up as a
    // diff against this pin.
    let _ = (read_active_bounded, get_immutable_bounded);
}
