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
