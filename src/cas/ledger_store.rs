//! Generic immutable object storage and atomic active-pointer publication.
//!
//! This module intentionally knows nothing about memory schemas. It is the
//! filesystem boundary for higher-level append-only ledgers.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustix::fs::{flock, open, renameat_with, Dir, FlockOperation, Mode, OFlags, RenameFlags, CWD};
use tempfile::NamedTempFile;

#[derive(Debug)]
pub(super) struct PersonalVaultLease {
    _vault_lock: File,
    pub(super) vault_root: PathBuf,
    pub(super) claimed_namespaces: std::sync::Mutex<std::collections::HashSet<ImmutableLedgerNamespace>>,
    pub(super) projection_artifacts_claimed: std::sync::Mutex<bool>,
    pub(super) projection_lifecycle: std::sync::Mutex<()>,
    #[cfg(test)]
    pub(super) fail_projection_manifest_cleanup_sync_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(super) fail_projection_artifact_cleanup_sync_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(super) projection_reset_fault_once: std::sync::atomic::AtomicU8,
}

/// Sole owner of the personal vault's process-lifetime exclusive lock.
#[derive(Debug)]
pub(crate) struct PersonalVaultStorage {
    pub(super) lease: Arc<PersonalVaultLease>,
    created_this_open: bool,
}

/// Fixed immutable-ledger namespaces. Arbitrary filesystem names are not
/// accepted at subsystem boundaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ImmutableLedgerNamespace {
    ExecutionObservationFixture,
    Memory,
    ProjectionManifest,
}

impl ImmutableLedgerNamespace {
    fn directory_name(self) -> &'static str {
        match self {
            Self::ExecutionObservationFixture => "execution-observation-fixture-ledger",
            Self::Memory => "memory-ledger",
            Self::ProjectionManifest => "projection-store/manifest",
        }
    }
}

#[derive(Debug)]
pub(crate) struct ImmutableLedgerStorage {
    _lease: Arc<PersonalVaultLease>,
    ledger_directory: PathBuf,
    objects_directory: PathBuf,
    roots_directory: PathBuf,
    active_path: PathBuf,
    candidate_path: PathBuf,
    #[cfg(test)]
    fail_pre_exchange_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_post_exchange_sync_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    bounded_collision_before_noclobber_once: std::sync::Mutex<Option<Vec<u8>>>,
}

/// Borrowed, read-only view of an existing projection tree. It cannot escape
/// the inspection closure and exposes neither paths nor write operations.
pub(crate) struct ExistingProjectionReadOnly<'a> {
    _lease: &'a PersonalVaultLease,
    manifest_objects: PathBuf,
    manifest_active: PathBuf,
    artifact_objects: PathBuf,
}

pub(super) enum ExistingProjectionLayout<'a> {
    Invalid,
    Readable(ExistingProjectionReadOnly<'a>),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LedgerStorageError {
    #[error("immutable ledger storage I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("active ledger pointer was exchanged but directory sync failed: {0}")]
    PublishedButUnsynced(std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LedgerStorageOpenError {
    #[error("immutable ledger storage I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("rejected legacy marker exists")]
    RejectedMarker,
    #[error("immutable ledger namespace is already claimed by this vault lifecycle")]
    NamespaceAlreadyClaimed,
    #[error("projection reset transaction requires owner recovery")]
    ProjectionResetPending,
    #[error("projection reset transaction requires owner maintenance")]
    ProjectionResetMaintenanceRequired,
    #[error("projection reset recovery outcome is indeterminate; restart is required")]
    ProjectionResetIndeterminate,
    #[error("projection reset transaction requires manual intervention")]
    ProjectionResetManualIntervention,
    #[error("projection storage format requires a newer or migration-capable binary")]
    UnsupportedProjectionFormat,
}

impl PersonalVaultStorage {
    pub(crate) fn open(vault_root: &Path, rejected_marker: Option<&str>) -> Result<Self, LedgerStorageOpenError> {
        let parent = vault_root.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "personal vault root has no parent")
        })?;
        let basename = vault_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "personal vault root has no valid basename",
                )
            })?;
        ensure_existing_directory(parent)?;
        reject_symlink_or_special(vault_root, true)?;
        reject_marker_if_present(vault_root, rejected_marker)?;
        let lock_path = parent.join(format!(".{basename}.plico-vault.lock"));
        let lock_was_absent =
            fs::symlink_metadata(&lock_path).is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        let vault_lock: File = open(
            &lock_path,
            OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
        .into();
        require_private_regular_file(&vault_lock)?;
        if lock_was_absent {
            vault_lock.sync_all()?;
            sync_directory(parent)?;
        }
        flock(&vault_lock, FlockOperation::NonBlockingLockExclusive)
            .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
        validate_open_lock_identity(&vault_lock, &lock_path)?;
        reject_marker_if_present(vault_root, rejected_marker)?;
        let created_this_open = ensure_vault_directory(vault_root, parent)?;
        Ok(Self {
            lease: Arc::new(PersonalVaultLease {
                _vault_lock: vault_lock,
                vault_root: vault_root.to_path_buf(),
                claimed_namespaces: std::sync::Mutex::new(std::collections::HashSet::new()),
                projection_artifacts_claimed: std::sync::Mutex::new(false),
                projection_lifecycle: std::sync::Mutex::new(()),
                #[cfg(test)]
                fail_projection_manifest_cleanup_sync_once: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                fail_projection_artifact_cleanup_sync_once: std::sync::atomic::AtomicBool::new(false),
                #[cfg(test)]
                projection_reset_fault_once: std::sync::atomic::AtomicU8::new(0),
            }),
            created_this_open,
        })
    }

    /// Whether this storage lifecycle created the vault directory while
    /// holding the exclusive vault lease.
    ///
    /// This is the only evidence accepted by kernel startup for the empty-vault
    /// projection bootstrap. Existing empty directories and canonical genesis
    /// state are deliberately insufficient.
    pub(crate) const fn created_this_open(&self) -> bool {
        self.created_this_open
    }

    pub(crate) fn immutable_ledger(
        &self,
        namespace: ImmutableLedgerNamespace,
    ) -> Result<ImmutableLedgerStorage, LedgerStorageOpenError> {
        if namespace != ImmutableLedgerNamespace::Memory {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "namespace requires its sealed storage boundary",
            )
            .into());
        }
        self.claim_immutable_ledger(namespace)
    }

    pub(super) fn claim_immutable_ledger(
        &self,
        namespace: ImmutableLedgerNamespace,
    ) -> Result<ImmutableLedgerStorage, LedgerStorageOpenError> {
        let mut claims = self
            .lease
            .claimed_namespaces
            .lock()
            .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
        if !claims.insert(namespace) {
            return Err(LedgerStorageOpenError::NamespaceAlreadyClaimed);
        }
        drop(claims);
        let result = open_immutable_ledger(Arc::clone(&self.lease), namespace);
        if result.is_err() {
            let mut claims = self
                .lease
                .claimed_namespaces
                .lock()
                .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
            claims.remove(&namespace);
        }
        result
    }

    pub(crate) fn object_cas_root(&self) -> PathBuf {
        self.lease.vault_root.join("cas")
    }

    pub(crate) fn with_existing_projection_readonly<R>(
        &self,
        inspect: impl for<'a> FnOnce(Option<ExistingProjectionReadOnly<'a>>) -> R,
    ) -> Result<R, LedgerStorageOpenError> {
        let _lifecycle = self
            .lease
            .projection_lifecycle
            .lock()
            .map_err(|_| std::io::Error::other("projection lifecycle state is poisoned"))?;
        let manifest_claimed = self
            .lease
            .claimed_namespaces
            .lock()
            .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?
            .contains(&ImmutableLedgerNamespace::ProjectionManifest);
        let artifacts_claimed = *self
            .lease
            .projection_artifacts_claimed
            .lock()
            .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
        if manifest_claimed || artifacts_claimed {
            return Err(LedgerStorageOpenError::NamespaceAlreadyClaimed);
        }
        super::projection_store::reject_projection_reset_pending(&self.lease.vault_root)?;
        reject_legacy_projection_siblings(&self.lease.vault_root)?;
        let projection_store = self.lease.vault_root.join("projection-store");
        let projection_store_exists = path_is_absent_or_private_directory(&projection_store)?;
        match projection_store_exists {
            false => Ok(inspect(None)),
            true => match inspect_projection_layout_at(&self.lease, &projection_store)? {
                ExistingProjectionLayout::Readable(reader) => Ok(inspect(Some(reader))),
                ExistingProjectionLayout::Invalid => Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid projection storage layout",
                )
                .into()),
            },
        }
    }

    pub(super) fn projection_artifact_parts(&self) -> (Arc<PersonalVaultLease>, PathBuf) {
        (
            Arc::clone(&self.lease),
            self.lease.vault_root.join("projection-store/artifacts"),
        )
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_manifest_cleanup_sync_failure_once(&self) {
        self.lease
            .fail_projection_manifest_cleanup_sync_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_artifact_cleanup_sync_failure_once(&self) {
        self.lease
            .fail_projection_artifact_cleanup_sync_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

pub(super) fn inspect_projection_layout_at<'a>(
    lease: &'a PersonalVaultLease,
    projection_store: &Path,
) -> std::io::Result<ExistingProjectionLayout<'a>> {
    if private_directory_state(projection_store)? != ProjectionPathState::Valid {
        return Ok(ExistingProjectionLayout::Invalid);
    }
    let manifest = projection_store.join("manifest");
    let artifacts = projection_store.join("artifacts");
    let manifest_objects = manifest.join("objects");
    let roots = manifest.join("roots");
    let active = roots.join("active");
    let candidate = roots.join("candidate");
    let artifact_objects = artifacts.join("objects");
    for directory in [&manifest, &artifacts, &manifest_objects, &roots, &artifact_objects] {
        if private_directory_state(directory)? != ProjectionPathState::Valid {
            return Ok(ExistingProjectionLayout::Invalid);
        }
    }
    for file in [&active, &candidate] {
        if private_regular_state(file)? != ProjectionPathState::Valid {
            return Ok(ExistingProjectionLayout::Invalid);
        }
    }
    Ok(ExistingProjectionLayout::Readable(ExistingProjectionReadOnly {
        _lease: lease,
        manifest_objects,
        manifest_active: active,
        artifact_objects,
    }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectionPathState {
    Valid,
    Invalid,
}

fn private_directory_state(path: &Path) -> std::io::Result<ProjectionPathState> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Ok(
                    if metadata.file_type().is_dir()
                        && !metadata.file_type().is_symlink()
                        && metadata.permissions().mode() & 0o777 == 0o700
                    {
                        ProjectionPathState::Valid
                    } else {
                        ProjectionPathState::Invalid
                    },
                )
            }
            #[cfg(not(unix))]
            {
                Ok(if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
                    ProjectionPathState::Valid
                } else {
                    ProjectionPathState::Invalid
                })
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ProjectionPathState::Invalid),
        Err(error) => Err(error),
    }
}

fn private_regular_state(path: &Path) -> std::io::Result<ProjectionPathState> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Ok(
                    if metadata.file_type().is_file()
                        && !metadata.file_type().is_symlink()
                        && metadata.permissions().mode() & 0o777 == 0o600
                    {
                        ProjectionPathState::Valid
                    } else {
                        ProjectionPathState::Invalid
                    },
                )
            }
            #[cfg(not(unix))]
            {
                Ok(
                    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
                        ProjectionPathState::Valid
                    } else {
                        ProjectionPathState::Invalid
                    },
                )
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ProjectionPathState::Invalid),
        Err(error) => Err(error),
    }
}

pub(super) fn open_immutable_ledger(
    lease: Arc<PersonalVaultLease>,
    namespace: ImmutableLedgerNamespace,
) -> Result<ImmutableLedgerStorage, LedgerStorageOpenError> {
    let ledger_directory = lease.vault_root.join(namespace.directory_name());
    open_immutable_ledger_directory(lease, ledger_directory, true)
}

pub(super) fn open_immutable_ledger_directory(
    lease: Arc<PersonalVaultLease>,
    ledger_directory: PathBuf,
    create: bool,
) -> Result<ImmutableLedgerStorage, LedgerStorageOpenError> {
    let ledger_parent = ledger_directory.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "immutable ledger directory has no parent",
        )
    })?;
    if create {
        ensure_directory(&ledger_directory, ledger_parent)?;
    } else {
        require_private_directory(&ledger_directory)?;
    }
    let objects_directory = ledger_directory.join("objects");
    let roots_directory = ledger_directory.join("roots");
    if create {
        ensure_directory(&objects_directory, &ledger_directory)?;
        ensure_directory(&roots_directory, &ledger_directory)?;
    } else {
        require_private_directory(&objects_directory)?;
        require_private_directory(&roots_directory)?;
    }
    let active_path = roots_directory.join("active");
    let candidate_path = roots_directory.join("candidate");
    reject_symlink_or_special(&active_path, false)?;
    reject_symlink_or_special(&candidate_path, false)?;
    match (active_path.exists(), candidate_path.exists(), create) {
        (false, false, true) => {
            create_private_file(&active_path)?.sync_all()?;
            create_private_file(&candidate_path)?.sync_all()?;
            sync_directory(&roots_directory)?;
        }
        (true, true, _) => {}
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "incomplete immutable ledger pointer slots",
            )
            .into());
        }
    }
    set_file_mode(&active_path)?;
    set_file_mode(&candidate_path)?;
    Ok(ImmutableLedgerStorage {
        _lease: lease,
        ledger_directory,
        objects_directory,
        roots_directory,
        active_path,
        candidate_path,
        #[cfg(test)]
        fail_pre_exchange_once: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        fail_post_exchange_sync_once: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        bounded_collision_before_noclobber_once: std::sync::Mutex::new(None),
    })
}

pub(super) fn open_existing_immutable_ledger(
    lease: Arc<PersonalVaultLease>,
    namespace: ImmutableLedgerNamespace,
) -> Result<ImmutableLedgerStorage, LedgerStorageOpenError> {
    let ledger_directory = lease.vault_root.join(namespace.directory_name());
    open_immutable_ledger_directory(lease, ledger_directory, false)
}

impl ImmutableLedgerStorage {
    pub(crate) fn put_immutable(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        put_immutable_at(&self.objects_directory, hash, bytes)
    }

    /// Atomically installs one size-bounded immutable object.  A concurrent
    /// winner is compared through the same bound; this method never falls
    /// back to the legacy unbounded collision path.
    pub(super) fn put_immutable_bounded(&self, hash: &str, bytes: &[u8], maximum_bytes: u64) -> std::io::Result<()> {
        #[cfg(test)]
        if let Some(collision_bytes) = self
            .bounded_collision_before_noclobber_once
            .lock()
            .map_err(|_| std::io::Error::other("bounded collision injector is poisoned"))?
            .take()
        {
            let (target, temporary) = prepare_immutable_bounded(&self.objects_directory, hash, bytes, maximum_bytes)?;
            let mut collision = create_private_file(&target)?;
            collision.write_all(&collision_bytes)?;
            collision.sync_all()?;
            sync_directory(&self.objects_directory)?;
            return persist_immutable_bounded(&self.objects_directory, &target, temporary, bytes, maximum_bytes);
        }
        put_immutable_bounded_at(&self.objects_directory, hash, bytes, maximum_bytes)
    }

    #[cfg(test)]
    pub(super) fn inject_bounded_collision_before_noclobber_once(&self, bytes: &[u8]) {
        *self
            .bounded_collision_before_noclobber_once
            .lock()
            .expect("bounded collision injector lock") = Some(bytes.to_vec());
    }

    pub(crate) fn get_immutable(&self, hash: &str) -> std::io::Result<Vec<u8>> {
        get_immutable_at(&self.objects_directory, hash)
    }

    pub(crate) fn get_immutable_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        validate_hash(hash)?;
        read_private_file_bounded(&self.objects_directory.join(hash), maximum_bytes)
    }

    pub(crate) fn read_active(&self) -> std::io::Result<Option<Vec<u8>>> {
        let bytes = read_private_file(&self.active_path)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(crate) fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        let bytes = read_private_file_bounded(&self.active_path, maximum_bytes)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(crate) fn read_candidate_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        let bytes = read_private_file_bounded(&self.candidate_path, maximum_bytes)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(crate) fn list_immutable_hashes(&self) -> std::io::Result<Vec<String>> {
        self.list_immutable_hashes_bounded(usize::MAX)
    }

    pub(crate) fn list_immutable_hashes_bounded(&self, maximum_entries: usize) -> std::io::Result<Vec<String>> {
        let mut hashes = Vec::new();
        for entry in fs::read_dir(&self.objects_directory)? {
            if hashes.len() >= maximum_entries {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "immutable ledger inventory exceeds entry limit",
                ));
            }
            let entry = entry?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 immutable object name"))?;
            validate_hash(&hash)?;
            if !entry.file_type()?.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "immutable ledger object is not a regular file",
                ));
            }
            require_regular_file(&entry.path())?;
            set_file_mode(&entry.path())?;
            hashes.push(hash);
        }
        hashes.sort();
        Ok(hashes)
    }

    /// Publish candidate bytes with an atomic `RENAME_EXCHANGE` of the two
    /// fixed pointer slots. There is intentionally no non-atomic fallback.
    pub(crate) fn publish_active(&self, bytes: &[u8]) -> Result<(), LedgerStorageError> {
        require_regular_file(&self.active_path)?;
        require_regular_file(&self.candidate_path)?;
        let mut temporary = NamedTempFile::new_in(&self.roots_directory)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.candidate_path).map_err(|error| error.error)?;
        set_file_mode(&self.candidate_path)?;
        sync_directory(&self.roots_directory)?;
        #[cfg(test)]
        if self
            .fail_pre_exchange_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(LedgerStorageError::Io(std::io::Error::other(
                "injected pre-exchange failure",
            )));
        }
        renameat_with(CWD, &self.active_path, CWD, &self.candidate_path, RenameFlags::EXCHANGE)
            .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
        #[cfg(test)]
        if self
            .fail_post_exchange_sync_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                "injected post-exchange directory sync failure",
            )));
        }
        sync_directory(&self.roots_directory).map_err(LedgerStorageError::PublishedButUnsynced)
    }

    #[cfg(test)]
    pub(crate) fn inject_pre_exchange_failure_once(&self) {
        self.fail_pre_exchange_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_post_exchange_sync_failure_once(&self) {
        self.fail_post_exchange_sync_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn flush(&self) -> std::io::Result<()> {
        sync_directory(&self.objects_directory)?;
        sync_directory(&self.roots_directory)?;
        sync_directory(&self.ledger_directory)
    }

    pub(super) fn remove_unreferenced_projection_objects(
        &self,
        referenced: &std::collections::HashSet<String>,
    ) -> std::io::Result<usize> {
        let hashes =
            self.list_immutable_hashes_bounded(super::projection_store::MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES)?;
        let mut removed = 0usize;
        for hash in hashes {
            if referenced.contains(&hash) {
                continue;
            }
            remove_private_regular_file_exact(&self.objects_directory.join(&hash))?;
            removed = removed
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("projection manifest cleanup count overflow"))?;
        }
        #[cfg(test)]
        if self
            ._lease
            .fail_projection_manifest_cleanup_sync_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(std::io::Error::other(
                "injected projection manifest cleanup directory sync failure",
            ));
        }
        sync_directory(&self.objects_directory)?;
        Ok(removed)
    }
}

#[cfg(unix)]
fn remove_private_regular_file_exact(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let file: File = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
    .into();
    require_private_regular_file(&file)?;
    let opened = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if current.file_type().is_symlink()
        || !current.file_type().is_file()
        || opened.dev() != current.dev()
        || opened.ino() != current.ino()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection manifest object identity changed during cleanup",
        ));
    }
    fs::remove_file(path)
}

#[cfg(not(unix))]
fn remove_private_regular_file_exact(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection manifest cleanup requires same-file identity support",
    ))
}

impl ExistingProjectionReadOnly<'_> {
    pub(crate) fn read_manifest_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        let bytes = read_private_file_bounded(&self.manifest_active, maximum_bytes)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(crate) fn read_manifest_object_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        validate_hash(hash)?;
        read_private_file_bounded(&self.manifest_objects.join(hash), maximum_bytes)
    }

    pub(crate) fn read_artifact_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        validate_hash(hash)?;
        read_private_file_bounded(&self.artifact_objects.join(hash), maximum_bytes)
    }

    pub(crate) fn validate_artifact_inventory(
        &self,
        referenced_hashes: &std::collections::HashSet<String>,
        maximum_bytes: u64,
    ) -> std::io::Result<()> {
        validate_readonly_inventory(&self.artifact_objects, referenced_hashes, maximum_bytes)
    }
}

fn reject_marker_if_present(vault_root: &Path, rejected_marker: Option<&str>) -> Result<(), LedgerStorageOpenError> {
    let Some(marker) = rejected_marker else {
        return Ok(());
    };
    match fs::symlink_metadata(vault_root.join(marker)) {
        Ok(_) => Err(LedgerStorageOpenError::RejectedMarker),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_existing_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "immutable ledger parent is not a directory",
        ))
    }
}

pub(super) fn ensure_directory(path: &Path, parent: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => set_directory_mode(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "immutable ledger path is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(path)?;
            set_directory_mode(path)?;
            sync_directory(parent)
        }
        Err(error) => Err(error),
    }
}

fn ensure_vault_directory(path: &Path, parent: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            restrict_owned_vault_directory(path)?;
            Ok(false)
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "personal vault path is not a directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_private_directory(path)?;
            sync_directory(parent)?;
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn restrict_owned_vault_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    set_directory_mode(path)
}

#[cfg(not(unix))]
fn restrict_owned_vault_directory(path: &Path) -> std::io::Result<()> {
    set_directory_mode(path)
}

fn reject_symlink_or_special(path: &Path, allow_directory: bool) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && (metadata.file_type().is_file() || allow_directory && metadata.file_type().is_dir()) =>
        {
            Ok(())
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "immutable ledger path has an unsupported file type",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn require_regular_file(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "immutable ledger path is not a regular file",
        ))
    }
}

#[cfg(unix)]
fn require_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o777 == 0o700
    {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "immutable ledger directory is not private",
        ))
    }
}

fn path_is_absent_or_private_directory(path: &Path) -> std::io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            require_private_directory(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn reject_legacy_projection_siblings(vault_root: &Path) -> Result<(), LedgerStorageOpenError> {
    for name in ["projection-manifest", "projection-artifacts"] {
        match fs::symlink_metadata(vault_root.join(name)) {
            Ok(_) => {
                return Err(LedgerStorageOpenError::UnsupportedProjectionFormat);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn validate_readonly_inventory(
    directory: &Path,
    referenced_hashes: &std::collections::HashSet<String>,
    maximum_bytes: u64,
) -> std::io::Result<()> {
    let directory_fd = open(
        directory,
        readonly_file_flags() | OFlags::DIRECTORY | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let mut entries =
        Dir::new(directory_fd).map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
        let name = entry.file_name().to_bytes();
        if matches!(name, b"." | b"..") {
            continue;
        }
        let hash = std::str::from_utf8(name)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 artifact hash"))?
            .to_string();
        validate_hash(&hash)?;
        let path = directory.join(&hash);
        if read_private_file_bounded(&path, maximum_bytes).is_ok() {
            continue;
        }
        if !referenced_hashes.contains(&hash) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unreferenced projection artifact is not a bounded private regular file",
            ));
        }
    }
    Ok(())
}

fn readonly_file_flags() -> OFlags {
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    #[cfg(target_os = "linux")]
    let flags = flags | OFlags::NOATIME;
    flags
}

#[cfg(not(unix))]
fn require_private_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "immutable ledger directory is not private",
        ))
    }
}

pub(super) fn create_private_file(path: &Path) -> std::io::Result<File> {
    let file: File = open(
        path,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
    .into();
    require_private_regular_file(&file)?;
    Ok(file)
}

pub(super) fn read_private_file(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file: File = open(path, readonly_file_flags(), Mode::empty())
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
        .into();
    require_private_regular_file(&file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn read_private_file_bounded(path: &Path, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
    let mut file: File = open(path, readonly_file_flags() | OFlags::NONBLOCK, Mode::empty())
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
        .into();
    require_private_regular_file(&file)?;
    if file.metadata()?.len() > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "private immutable object exceeds size limit",
        ));
    }
    let read_limit = maximum_bytes
        .checked_add(1)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid immutable object size limit"))?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(read_limit)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "private immutable object exceeds size limit",
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn require_private_regular_file(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = file.metadata()?;
    if metadata.file_type().is_file() && metadata.permissions().mode() & 0o777 == 0o600 {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "immutable ledger file is not a private regular file",
        ))
    }
}

#[cfg(unix)]
fn validate_open_lock_identity(file: &File, lock_path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata()?;
    let path = fs::symlink_metadata(lock_path)?;
    if path.file_type().is_symlink()
        || !path.file_type().is_file()
        || opened.dev() != path.dev()
        || opened.ino() != path.ino()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "vault lock identity changed during acquisition",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_open_lock_identity(file: &File, lock_path: &Path) -> std::io::Result<()> {
    require_private_regular_file(file)?;
    require_regular_file(lock_path)
}

#[cfg(not(unix))]
fn require_private_regular_file(file: &File) -> std::io::Result<()> {
    if file.metadata()?.is_file() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "immutable ledger file is not a regular file",
        ))
    }
}

#[cfg(unix)]
fn set_directory_mode(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)?.permissions().mode() & 0o777;
    if mode == 0o700 {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "immutable ledger directory permissions could not be restricted",
        ))
    }
}

#[cfg(unix)]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_private_directory(path: &Path) -> std::io::Result<()> {
    fs::create_dir(path)
}

#[cfg(not(unix))]
fn set_directory_mode(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn set_file_mode(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::symlink_metadata(path)?.permissions().mode() & 0o777;
    if mode == 0o600 {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "immutable ledger file permissions could not be restricted",
        ))
    }
}

#[cfg(not(unix))]
pub(super) fn set_file_mode(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

pub(super) fn validate_hash(hash: &str) -> std::io::Result<()> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid immutable ledger object hash",
        ))
    }
}

pub(super) fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

pub(super) fn put_immutable_at(directory: &Path, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
    validate_hash(hash)?;
    let target = directory.join(hash);
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            if read_private_file(&target)? == bytes {
                return Ok(());
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "immutable object collision",
            ));
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "immutable object is not a regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut temporary = NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(target).map_err(|error| error.error)?;
    set_file_mode(&directory.join(hash))?;
    sync_directory(directory)
}

/// Bounded, no-clobber immutable publication for sealed capabilities.
///
/// Unlike [`put_immutable_at`], this deliberately does not pre-read the
/// destination.  The filesystem chooses the winner atomically; if another
/// writer won, only a bounded read may establish an identical retry.
pub(super) fn put_immutable_bounded_at(
    directory: &Path,
    hash: &str,
    bytes: &[u8],
    maximum_bytes: u64,
) -> std::io::Result<()> {
    let (target, temporary) = prepare_immutable_bounded(directory, hash, bytes, maximum_bytes)?;
    persist_immutable_bounded(directory, &target, temporary, bytes, maximum_bytes)
}

fn prepare_immutable_bounded(
    directory: &Path,
    hash: &str,
    bytes: &[u8],
    maximum_bytes: u64,
) -> std::io::Result<(PathBuf, NamedTempFile)> {
    validate_hash(hash)?;
    let byte_count = u64::try_from(bytes.len())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "object is too large"))?;
    if byte_count > maximum_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "object exceeds bounded write limit",
        ));
    }

    let target = directory.join(hash);
    let mut temporary = NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    Ok((target, temporary))
}

fn persist_immutable_bounded(
    directory: &Path,
    target: &Path,
    temporary: NamedTempFile,
    bytes: &[u8],
    maximum_bytes: u64,
) -> std::io::Result<()> {
    match temporary.persist_noclobber(target) {
        Ok(_) => {
            set_file_mode(target)?;
            sync_directory(directory)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = read_private_file_bounded(target, maximum_bytes)?;
            if existing == bytes {
                // A previous writer may have installed these bytes but failed
                // while syncing the directory.  A byte-identical retry must
                // re-establish that durability boundary before reporting
                // success.
                sync_directory(directory)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "immutable object collision",
                ))
            }
        }
        Err(error) => Err(error.error),
    }
}

pub(super) fn get_immutable_at(directory: &Path, hash: &str) -> std::io::Result<Vec<u8>> {
    validate_hash(hash)?;
    read_private_file(&directory.join(hash))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::time::Duration;

    fn create_projection_pair_fixture(vault: &Path) {
        for directory in [
            vault.join("projection-store"),
            vault.join("projection-store/manifest"),
            vault.join("projection-store/manifest/objects"),
            vault.join("projection-store/manifest/roots"),
            vault.join("projection-store/artifacts"),
            vault.join("projection-store/artifacts/objects"),
        ] {
            fs::create_dir(&directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for file in [
            vault.join("projection-store/manifest/roots/active"),
            vault.join("projection-store/manifest/roots/candidate"),
        ] {
            fs::write(&file, []).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
        }
    }

    #[test]
    fn fresh_vault_evidence_is_lifecycle_exact() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("cas::ledger_store::tests::fresh_vault_evidence_lifecycle_child")
            .arg("--nocapture")
            .env("PLICO_FRESH_VAULT_EVIDENCE_CHILD", "1")
            .status()
            .unwrap();
        assert!(status.success(), "fresh-vault evidence child failed");
    }

    #[test]
    fn fresh_vault_evidence_lifecycle_child() {
        if std::env::var_os("PLICO_FRESH_VAULT_EVIDENCE_CHILD").is_none() {
            return;
        }
        let parent = tempfile::tempdir().unwrap();
        let fresh_path = parent.path().join("fresh-vault");
        let fresh = PersonalVaultStorage::open(&fresh_path, None).unwrap();
        assert!(fresh.created_this_open());
        drop(fresh);

        let reopened = PersonalVaultStorage::open(&fresh_path, None).unwrap();
        assert!(!reopened.created_this_open());
        drop(reopened);

        let existing_empty = parent.path().join("existing-empty-vault");
        fs::create_dir(&existing_empty).unwrap();
        fs::set_permissions(&existing_empty, fs::Permissions::from_mode(0o700)).unwrap();
        let existing = PersonalVaultStorage::open(&existing_empty, None).unwrap();
        assert!(!existing.created_this_open());
    }

    #[test]
    fn immutable_object_symlink_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        let storage = owner.immutable_ledger(ImmutableLedgerNamespace::Memory).unwrap();
        let hash = "a".repeat(64);
        let outside = parent.path().join("outside");
        fs::write(&outside, b"secret").unwrap();
        symlink(&outside, vault.join("memory-ledger").join("objects").join(&hash)).unwrap();

        let error = storage.get_immutable(&hash).unwrap_err();
        assert!(error.kind() == std::io::ErrorKind::InvalidData || error.raw_os_error() == Some(40));
        assert_eq!(
            storage.list_immutable_hashes().unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn pointer_slot_symlink_is_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::create_dir(vault.join("memory-ledger")).unwrap();
        fs::create_dir(vault.join("memory-ledger").join("objects")).unwrap();
        fs::create_dir(vault.join("memory-ledger").join("roots")).unwrap();
        for directory in [
            vault.clone(),
            vault.join("memory-ledger"),
            vault.join("memory-ledger").join("objects"),
            vault.join("memory-ledger").join("roots"),
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let outside = parent.path().join("outside");
        fs::write(&outside, []).unwrap();
        symlink(&outside, vault.join("memory-ledger").join("roots").join("active")).unwrap();
        let candidate = vault.join("memory-ledger").join("roots").join("candidate");
        fs::write(&candidate, []).unwrap();
        fs::set_permissions(candidate, fs::Permissions::from_mode(0o600)).unwrap();

        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        assert!(matches!(
            owner.immutable_ledger(ImmutableLedgerNamespace::Memory),
            Err(LedgerStorageOpenError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData
        ));
    }

    #[test]
    fn rejected_marker_does_not_modify_parent_directory() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("legacy"), b"legacy").unwrap();
        let before: Vec<_> = fs::read_dir(parent.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();

        assert!(matches!(
            PersonalVaultStorage::open(&vault, Some("legacy")),
            Err(LedgerStorageOpenError::RejectedMarker)
        ));
        let after: Vec<_> = fs::read_dir(parent.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(before, after);
    }

    #[test]
    fn ledger_directories_and_files_are_private() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        let storage = owner.immutable_ledger(ImmutableLedgerNamespace::Memory).unwrap();
        let hash = "b".repeat(64);
        storage.put_immutable(&hash, b"value").unwrap();

        for directory in [
            vault.clone(),
            vault.join("memory-ledger"),
            vault.join("memory-ledger").join("objects"),
            vault.join("memory-ledger").join("roots"),
        ] {
            assert_eq!(fs::metadata(directory).unwrap().permissions().mode() & 0o777, 0o700);
        }
        for file in [
            parent.path().join(".vault.plico-vault.lock"),
            vault.join("memory-ledger").join("roots").join("active"),
            vault.join("memory-ledger").join("roots").join("candidate"),
            vault.join("memory-ledger").join("objects").join(hash),
        ] {
            assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn uppercase_hash_alias_and_permissive_existing_tree_are_rejected() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        let storage = owner.immutable_ledger(ImmutableLedgerNamespace::Memory).unwrap();
        assert_eq!(
            storage.get_immutable(&"A".repeat(64)).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        drop(storage);
        assert!(matches!(
            owner.immutable_ledger(ImmutableLedgerNamespace::Memory),
            Err(LedgerStorageOpenError::NamespaceAlreadyClaimed)
        ));

        let permissive_parent = tempfile::tempdir().unwrap();
        let permissive_vault = permissive_parent.path().join("vault");
        fs::create_dir(&permissive_vault).unwrap();
        fs::set_permissions(&permissive_vault, fs::Permissions::from_mode(0o700)).unwrap();
        fs::create_dir(permissive_vault.join("memory-ledger")).unwrap();
        fs::set_permissions(
            permissive_vault.join("memory-ledger"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let permissive_owner = PersonalVaultStorage::open(&permissive_vault, None).unwrap();
        let error = permissive_owner
            .immutable_ledger(ImmutableLedgerNamespace::Memory)
            .unwrap_err();
        assert!(matches!(error, LedgerStorageOpenError::Io(_)), "{error:?}");
    }

    #[test]
    fn owned_existing_vault_root_is_restricted_before_ledger_creation() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::set_permissions(&vault, fs::Permissions::from_mode(0o755)).unwrap();

        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        let storage = owner.immutable_ledger(ImmutableLedgerNamespace::Memory).unwrap();
        assert_eq!(fs::metadata(&vault).unwrap().permissions().mode() & 0o777, 0o700);
        drop(storage);
    }

    #[test]
    fn failed_namespace_initialization_releases_lifecycle_claim() {
        let parent = tempfile::tempdir().unwrap();
        let vault = parent.path().join("vault");
        let owner = PersonalVaultStorage::open(&vault, None).unwrap();
        let projection_store = vault.join("projection-store");
        fs::create_dir(&projection_store).unwrap();
        fs::set_permissions(&projection_store, fs::Permissions::from_mode(0o700)).unwrap();
        let namespace = projection_store.join("manifest");
        fs::create_dir(&namespace).unwrap();
        fs::set_permissions(&namespace, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(matches!(
            owner.open_existing_projection_writer(),
            Err(LedgerStorageOpenError::Io(_))
        ));
        fs::remove_dir(&namespace).unwrap();
        fs::remove_dir(&projection_store).unwrap();
        drop(
            owner
                .prepare_projection_pair_target(crate::cas::ProjectionPairPublishMode::CreateAbsent)
                .unwrap(),
        );
    }

    #[test]
    fn projection_readonly_transaction_serializes_writer_claim_until_return() {
        let parent = tempfile::tempdir().unwrap();
        let vault_path = parent.path().join("vault");
        let initial_owner = PersonalVaultStorage::open(&vault_path, None).unwrap();
        create_projection_pair_fixture(&vault_path);
        let owner = Arc::new(initial_owner);
        let (reader_entered_tx, reader_entered_rx) = mpsc::sync_channel(1);
        let (reader_release_tx, reader_release_rx) = mpsc::sync_channel(1);
        let reader_owner = Arc::clone(&owner);
        let reader = std::thread::spawn(move || {
            reader_owner
                .with_existing_projection_readonly(|existing| {
                    assert!(existing.is_some());
                    reader_entered_tx.send(()).unwrap();
                    reader_release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                })
                .unwrap();
        });
        reader_entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();

        let (writer_started_tx, writer_started_rx) = mpsc::sync_channel(1);
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writer_owner = Arc::clone(&owner);
        let writer = std::thread::spawn(move || {
            writer_started_tx.send(()).unwrap();
            let _ = writer_done_tx.send(writer_owner.open_existing_projection_writer());
        });
        writer_started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let writer_was_blocked = matches!(
            writer_done_rx.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        reader_release_tx.send(()).unwrap();
        assert!(writer_was_blocked);
        assert!(writer_done_rx.recv_timeout(Duration::from_secs(2)).unwrap().is_ok());
        reader.join().unwrap();
        writer.join().unwrap();
    }

    #[test]
    fn partial_projection_pair_is_rejected_without_claim_or_complement_creation() {
        let parent = tempfile::tempdir().unwrap();
        let vault_path = parent.path().join("vault");
        let owner = PersonalVaultStorage::open(&vault_path, None).unwrap();
        let projection_store = vault_path.join("projection-store");
        fs::create_dir(&projection_store).unwrap();
        fs::set_permissions(&projection_store, fs::Permissions::from_mode(0o700)).unwrap();
        let manifest = projection_store.join("manifest");
        fs::create_dir(&manifest).unwrap();
        fs::set_permissions(&manifest, fs::Permissions::from_mode(0o700)).unwrap();

        assert!(matches!(
            owner.with_existing_projection_readonly(|_| ()),
            Err(LedgerStorageOpenError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidData
        ));
        assert!(!projection_store.join("artifacts").exists());
        assert!(!matches!(
            owner.prepare_projection_pair_target(crate::cas::ProjectionPairPublishMode::CreateAbsent),
            Err(LedgerStorageOpenError::NamespaceAlreadyClaimed)
        ));
        assert!(!projection_store.join("artifacts").exists());

        fs::remove_dir(&manifest).unwrap();
        fs::remove_dir(&projection_store).unwrap();
        drop(
            owner
                .prepare_projection_pair_target(crate::cas::ProjectionPairPublishMode::CreateAbsent)
                .unwrap(),
        );
    }
}
