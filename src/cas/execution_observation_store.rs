//! Sealed CAS capability for the execution-observation fixture namespace.
//!
//! This is an architecture-owned capability boundary. It deliberately omits
//! unbounded reads, arbitrary namespaces and host paths.

#![allow(dead_code)] // Architecture-frozen for WP2; B2 deliberately has no caller.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::ledger_store::{
    read_private_file_bounded, validate_hash, ImmutableLedgerNamespace, ImmutableLedgerStorage, LedgerStorageError,
    LedgerStorageOpenError, PersonalVaultLease, PersonalVaultStorage,
};

const DIRECTORY: &str = "execution-observation-fixture-ledger";

pub(crate) struct ExecutionObservationFixtureStorage {
    inner: ImmutableLedgerStorage,
}

impl ExecutionObservationFixtureStorage {
    pub(crate) fn open(vault: Arc<PersonalVaultStorage>) -> Result<Self, LedgerStorageOpenError> {
        let directory = vault.lease.vault_root.join(DIRECTORY);
        if directory.exists() {
            validate_existing_topology(&directory)?;
        }
        let inner = vault.claim_immutable_ledger(ImmutableLedgerNamespace::ExecutionObservationFixture)?;
        Ok(Self { inner })
    }

    pub(crate) fn put_immutable_bounded(&self, hash: &str, bytes: &[u8], maximum_bytes: u64) -> std::io::Result<()> {
        self.inner.put_immutable_bounded(hash, bytes, maximum_bytes)
    }

    pub(crate) fn get_immutable_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.inner.get_immutable_bounded(hash, maximum_bytes)
    }

    pub(crate) fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        self.inner.read_active_bounded(maximum_bytes)
    }

    pub(crate) fn read_candidate_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        self.inner.read_candidate_bounded(maximum_bytes)
    }

    pub(crate) fn list_immutable_hashes_bounded(&self, maximum_entries: usize) -> std::io::Result<Vec<String>> {
        self.inner.list_immutable_hashes_bounded(maximum_entries)
    }

    pub(crate) fn publish_active(&self, pointer: &[u8]) -> Result<(), LedgerStorageError> {
        self.inner.publish_active(pointer)
    }

    #[cfg(test)]
    pub(crate) fn inject_pre_exchange_failure_once(&self) {
        self.inner.inject_pre_exchange_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_post_exchange_sync_failure_once(&self) {
        self.inner.inject_post_exchange_sync_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_bounded_collision_before_noclobber_once(&self, bytes: &[u8]) {
        self.inner.inject_bounded_collision_before_noclobber_once(bytes);
    }
}

/// Borrowed, closure-bounded read-only view of an existing execution
/// observation fixture namespace. It cannot escape the inspection closure,
/// holds no generic ledger capability, and exposes neither host paths,
/// candidate access, nor any write operation.
pub(crate) struct ExistingExecutionObservationReadOnly<'a> {
    _lease: &'a PersonalVaultLease,
    objects_directory: PathBuf,
    active_path: PathBuf,
}

impl PersonalVaultStorage {
    /// Inspect the existing execution-observation namespace without any
    /// writer capability: an absent namespace yields `None`, a present but
    /// damaged topology fails closed without repair, and the namespace is
    /// never created, completed, chmod-ed, or claimed here. A writer may
    /// therefore hold the namespace claim on the same vault concurrently,
    /// and readers only ever observe the complete pre-exchange or complete
    /// post-exchange active pointer: publication is a single
    /// `RENAME_EXCHANGE` and objects are immutable and durable before the
    /// pointer moves.
    pub(crate) fn with_existing_execution_observation_readonly<R>(
        &self,
        inspect: impl for<'a> FnOnce(Option<ExistingExecutionObservationReadOnly<'a>>) -> R,
    ) -> Result<R, LedgerStorageOpenError> {
        let directory = self.lease.vault_root.join(DIRECTORY);
        let present = match fs::symlink_metadata(&directory) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !present {
            return Ok(inspect(None));
        }
        validate_existing_topology(&directory)?;
        Ok(inspect(Some(ExistingExecutionObservationReadOnly {
            _lease: &self.lease,
            objects_directory: directory.join("objects"),
            active_path: directory.join("roots").join("active"),
        })))
    }
}

impl ExistingExecutionObservationReadOnly<'_> {
    pub(crate) fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        let bytes = read_private_file_bounded(&self.active_path, maximum_bytes)?;
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub(crate) fn get_immutable_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        validate_hash(hash)?;
        read_private_file_bounded(&self.objects_directory.join(hash), maximum_bytes)
    }
}

fn validate_existing_topology(directory: &Path) -> std::io::Result<()> {
    require_private_directory(directory)?;
    require_private_directory(&directory.join("objects"))?;
    let roots = directory.join("roots");
    require_private_directory(&roots)?;
    require_private_file(&roots.join("active"))?;
    require_private_file(&roots.join("candidate"))
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
            std::io::ErrorKind::InvalidData,
            "observation directory is not private",
        ))
    }
}

#[cfg(not(unix))]
fn require_private_directory(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "observation directory is invalid",
        ))
    }
}

#[cfg(unix)]
fn require_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o777 == 0o600
    {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "observation pointer is not private",
        ))
    }
}

#[cfg(test)]
mod tests;

#[cfg(not(unix))]
fn require_private_file(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "observation pointer is invalid",
        ))
    }
}
