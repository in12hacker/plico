//! Host filesystem boundary for offline legacy migration.
//!
//! The migrator binary receives bytes only. It cannot open paths, create its
//! own lock, or fall back to the runtime legacy reader.

use std::fs::{self, File};
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parcopy::{execute_plan, plan_copy, CopyPolicy, OnConflict, PlanAction, RuntimeOptions};
use rustix::fs::{flock, openat, renameat_with, FlockOperation, Mode, OFlags, RenameFlags, CWD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use tempfile::TempDir;
use walkdir::WalkDir;

use super::{ImmutableLedgerNamespace, ImmutableLedgerStorage, PersonalVaultStorage};

const MAX_VAULT_DEPTH: usize = 128;

#[derive(Debug)]
pub struct OfflineMigrationVault {
    _vault_lock: File,
    vault_directory: File,
    vault_root: PathBuf,
    vault_parent: PathBuf,
    lock_created: bool,
}

pub struct OfflineMigrationTarget {
    container: Option<TempDir>,
    staging_root: PathBuf,
    source_tree: VaultTreeManifest,
    source_root_mode: u32,
    _vault: Arc<PersonalVaultStorage>,
    storage: ImmutableLedgerStorage,
    seal: Option<OfflineMigrationSeal>,
    #[cfg(test)]
    force_post_exchange_failure: bool,
    #[cfg(test)]
    force_backup_verification_failure: bool,
}

#[derive(Clone)]
struct OfflineMigrationSeal {
    root_hash: String,
    source_manifest_hash: String,
    migration_manifest_hash: String,
    credential_role_cutoff_hash: String,
    revision_count: u64,
    policy_count: u64,
    target_tree: VaultTreeManifest,
    active_bytes_hash: String,
    root_object_bytes_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineMigrationPublication {
    pub backup_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineSourceFingerprint {
    pub source_index_hash: String,
    pub snapshots: Vec<OfflineSnapshotFingerprint>,
    pub referenced_objects: Vec<OfflineReferencedObjectFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineReferencedObjectFingerprint {
    pub cid: String,
    pub object_envelope_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineSnapshotFingerprint {
    pub legacy_agent_id: String,
    pub legacy_tier: String,
    pub cid: String,
    pub object_envelope_hash: String,
}

#[derive(Debug, thiserror::Error)]
pub enum OfflineMigrationError {
    #[error("offline migration I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("personal vault is already locked by another runtime")]
    VaultLocked,
    #[error("offline migration rejected: {category}")]
    Invalid { category: &'static str },
}

impl OfflineMigrationError {
    pub fn category(&self) -> &'static str {
        match self {
            Self::VaultLocked => "vault_locked",
            Self::Invalid { .. } => "invalid_source",
            Self::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => "permission",
            Self::Io(_) => "io",
        }
    }
}

impl OfflineMigrationVault {
    /// Open an existing personal vault and acquire the runtime's exact
    /// parent-level exclusive lock. The sole inspect-side mutation is atomic
    /// creation of a missing lock as mode 0600, followed by parent fsync.
    pub fn open(vault_root: &Path) -> Result<Self, OfflineMigrationError> {
        let parent = vault_root.parent().ok_or(OfflineMigrationError::Invalid {
            category: "vault_root_has_no_parent",
        })?;
        require_real_directory(parent)?;
        require_real_directory(vault_root)?;
        let basename = vault_root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .ok_or(OfflineMigrationError::Invalid {
                category: "invalid_vault_basename",
            })?;
        let lock_path = parent.join(format!(".{basename}.plico-vault.lock"));
        let (vault_lock, lock_created) = open_or_create_lock(&lock_path, parent)?;
        flock(&vault_lock, FlockOperation::NonBlockingLockExclusive).map_err(|error| match error.raw_os_error() {
            libc::EWOULDBLOCK => OfflineMigrationError::VaultLocked,
            code => OfflineMigrationError::Io(std::io::Error::from_raw_os_error(code)),
        })?;
        validate_open_lock_identity(&vault_lock, &lock_path)?;
        let vault_directory = open_real_directory(vault_root)?;
        Ok(Self {
            _vault_lock: vault_lock,
            vault_directory,
            vault_root: vault_root.to_path_buf(),
            vault_parent: parent.to_path_buf(),
            lock_created,
        })
    }

    pub fn lock_created(&self) -> bool {
        self.lock_created
    }

    /// Read exact legacy index bytes without updating filesystem atime.
    pub fn read_legacy_index_bytes(&self) -> Result<Vec<u8>, OfflineMigrationError> {
        read_regular_noatime_at(&self.vault_directory, "memory_index.json")
    }

    /// Read exact credential-set bytes without invoking the runtime key store,
    /// changing permissions, or exposing any path to the binary.
    pub fn read_agent_tokens_bytes(&self) -> Result<Vec<u8>, OfflineMigrationError> {
        read_private_regular_noatime_at(&self.vault_directory, "agent_tokens.json")
    }

    /// Read exact serialized legacy `AIObject` bytes without updating CAS
    /// access counters or filesystem atime. Envelope decoding stays in the
    /// offline binary's isolated legacy DTO module.
    pub fn read_legacy_object_bytes(&self, cid: &str) -> Result<Vec<u8>, OfflineMigrationError> {
        validate_hash(cid)?;
        let (prefix, suffix) = cid.split_at(2);
        let cas_directory = open_real_directory_at(&self.vault_directory, "cas")?;
        let shard_directory = open_real_directory_at(&cas_directory, prefix)?;
        read_regular_noatime_at(&shard_directory, suffix)
    }

    /// Re-read all source bytes while the exclusive vault lock is held. Any
    /// change since preflight rejects publication.
    pub fn revalidate_source(&self, expected: &OfflineSourceFingerprint) -> Result<(), OfflineMigrationError> {
        validate_fingerprint(expected)?;
        let index_bytes = self.read_legacy_index_bytes()?;
        if sha256(&index_bytes) != expected.source_index_hash {
            return invalid("legacy_source_changed");
        }
        for snapshot in &expected.snapshots {
            let bytes = self.read_legacy_object_bytes(&snapshot.cid)?;
            if sha256(&bytes) != snapshot.object_envelope_hash {
                return invalid("legacy_source_changed");
            }
        }
        for object in &expected.referenced_objects {
            let bytes = self.read_legacy_object_bytes(&object.cid)?;
            if sha256(&bytes) != object.object_envelope_hash {
                return invalid("legacy_source_changed");
            }
        }
        Ok(())
    }

    pub fn prepare_target(&self) -> Result<OfflineMigrationTarget, OfflineMigrationError> {
        let source_tree = scan_tree(&self.vault_root)?;
        let source_root_mode = fs::symlink_metadata(&self.vault_root)?.mode() & 0o7777;
        if source_tree.contains_top_level("memory-ledger") {
            return invalid("mixed_legacy_and_ledger_source");
        }
        let container = tempfile::Builder::new()
            .prefix(".plico-memory-migration-staging.")
            .tempdir_in(&self.vault_parent)?;
        let basename = self.vault_root.file_name().ok_or(OfflineMigrationError::Invalid {
            category: "invalid_vault_basename",
        })?;
        let staging_root = container.path().join(basename);
        let policy = CopyPolicy {
            on_conflict: OnConflict::Error,
            preserve_permissions: true,
            preserve_dir_permissions: true,
            preserve_symlinks: true,
            preserve_timestamps: true,
            preserve_windows_attributes: true,
            fsync: true,
            warn_escaping_symlinks: true,
            block_escaping_symlinks: true,
            max_depth: Some(MAX_VAULT_DEPTH),
        };
        let plan = plan_copy(
            vec![self.vault_root.clone()],
            staging_root.clone(),
            policy,
            RuntimeOptions::default(),
        )
        .map_err(|_| OfflineMigrationError::Invalid {
            category: "copy_plan_rejected",
        })?;
        if plan.items.len() != 1 || plan.items[0].action != PlanAction::Copy {
            return invalid("copy_plan_rejected");
        }
        let report = execute_plan(&plan, None);
        if report.has_failures() || report.items.len() != 1 {
            return invalid("staging_copy_failed");
        }
        let copied_tree = scan_tree(&staging_root)?;
        if copied_tree != source_tree || scan_tree(&self.vault_root)? != source_tree {
            return invalid("staging_copy_verification_failed");
        }
        remove_copied_legacy_marker(&staging_root)?;
        sync_directory(&staging_root)?;
        let vault =
            Arc::new(
                PersonalVaultStorage::open(&staging_root, None).map_err(|_| OfflineMigrationError::Invalid {
                    category: "staging_vault_open_failed",
                })?,
            );
        let storage =
            vault
                .immutable_ledger(ImmutableLedgerNamespace::Memory)
                .map_err(|_| OfflineMigrationError::Invalid {
                    category: "staging_ledger_open_failed",
                })?;
        Ok(OfflineMigrationTarget {
            container: Some(container),
            staging_root,
            source_tree,
            source_root_mode,
            _vault: vault,
            storage,
            seal: None,
            #[cfg(test)]
            force_post_exchange_failure: false,
            #[cfg(test)]
            force_backup_verification_failure: false,
        })
    }

    pub fn publish_target<F>(
        &self,
        mut target: OfflineMigrationTarget,
        expected: &OfflineSourceFingerprint,
        verify_credentials: F,
    ) -> Result<OfflineMigrationPublication, OfflineMigrationError>
    where
        F: FnOnce(&[u8]) -> Result<(), ()>,
    {
        let seal = target.seal.clone().ok_or(OfflineMigrationError::Invalid {
            category: "unsealed_staging_target",
        })?;
        if scan_tree(&self.vault_root)? != target.source_tree
            || scan_tree(&target.staging_root)? != seal.target_tree
            || !verify_published_seal_bytes(&target.staging_root, &seal)?
        {
            return invalid("legacy_source_changed");
        }
        self.revalidate_source(expected)?;
        let credential_bytes = zeroize::Zeroizing::new(self.read_agent_tokens_bytes()?);
        verify_credentials(&credential_bytes).map_err(|()| OfflineMigrationError::Invalid {
            category: "authorization_changed",
        })?;
        if !seal.target_tree.contains_top_level("memory-ledger")
            || seal.target_tree.contains_top_level("memory_index.json")
        {
            return invalid("invalid_staging_target");
        }
        let backup = reserve_backup_path(&self.vault_parent, &self.vault_root)?;
        target.protect_container_from_drop();
        if let Err(error) = renameat_with(CWD, &self.vault_root, CWD, &target.staging_root, RenameFlags::EXCHANGE) {
            target.allow_container_cleanup();
            return Err(OfflineMigrationError::Io(rustix_io(error)));
        }
        if sync_directory(&self.vault_parent).is_err()
            || sync_directory(target.container_path()).is_err()
            || scan_tree(&self.vault_root).map_or(true, |tree| tree != seal.target_tree)
            || !verify_published_seal_bytes(&self.vault_root, &seal).unwrap_or(false)
            || post_exchange_failure_requested(&target)
        {
            return rollback_exchange(self, target, "exchange_sync_failed");
        }
        // Harden while the exchanged legacy tree is still below tempfile's
        // owner-only parent, so a permissive legacy root is never published
        // under the public vault parent even for a rename-to-chmod window.
        if tighten_backup_permissions(&target.staging_root, &target.source_tree).is_err() {
            return rollback_after_evidence(self, target, "backup_permission_hardening_failed");
        }
        if renameat_with(CWD, &target.staging_root, CWD, &backup, RenameFlags::NOREPLACE).is_err() {
            return rollback_after_evidence(self, target, "backup_finalize_failed");
        }
        if write_backup_evidence(&backup, &target.source_tree, &seal).is_err() {
            return rollback_finalized(self, target, &backup, "backup_evidence_write_failed");
        }
        if sync_directory(&self.vault_parent).is_err()
            || sync_directory(target.container_path()).is_err()
            || !verify_backup_evidence(&backup, &target.source_tree, &seal).unwrap_or(false)
            || backup_verification_failure_requested(&target)
        {
            return rollback_finalized(self, target, &backup, "backup_verification_failed");
        }
        let backup_name = backup
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(OfflineMigrationError::Invalid {
                category: "invalid_backup_name",
            })?
            .to_string();
        target.allow_container_cleanup();
        Ok(OfflineMigrationPublication { backup_name })
    }
}

impl OfflineMigrationTarget {
    pub(crate) fn ledger_storage(&self) -> &ImmutableLedgerStorage {
        &self.storage
    }

    pub(crate) fn seal(
        &mut self,
        root_hash: String,
        source_manifest_hash: String,
        migration_manifest_hash: String,
        credential_role_cutoff_hash: String,
        revision_count: u64,
        policy_count: u64,
    ) -> Result<(), OfflineMigrationError> {
        for hash in [
            &root_hash,
            &source_manifest_hash,
            &migration_manifest_hash,
            &credential_role_cutoff_hash,
        ] {
            validate_hash(hash)?;
        }
        self.storage.flush()?;
        let active_bytes = read_private_regular_noatime(&self.staging_root.join("memory-ledger/roots/active"))?;
        let root_bytes =
            read_private_regular_noatime(&self.staging_root.join("memory-ledger/objects").join(&root_hash))?;
        self.seal = Some(OfflineMigrationSeal {
            root_hash,
            source_manifest_hash,
            migration_manifest_hash,
            credential_role_cutoff_hash,
            revision_count,
            policy_count,
            target_tree: scan_tree(&self.staging_root)?,
            active_bytes_hash: sha256(&active_bytes),
            root_object_bytes_hash: sha256(&root_bytes),
        });
        Ok(())
    }

    fn container_path(&self) -> &Path {
        self.container
            .as_ref()
            .expect("migration target retains its tempfile container")
            .path()
    }

    fn protect_container_from_drop(&mut self) {
        self.container
            .as_mut()
            .expect("migration target retains its tempfile container")
            .disable_cleanup(true);
    }

    fn allow_container_cleanup(&mut self) {
        self.container
            .as_mut()
            .expect("migration target retains its tempfile container")
            .disable_cleanup(false);
    }

    #[cfg(test)]
    fn seal_for_test(&mut self) {
        let root_hash = "a".repeat(64);
        self.seal = Some(OfflineMigrationSeal {
            root_hash,
            source_manifest_hash: "b".repeat(64),
            migration_manifest_hash: "c".repeat(64),
            credential_role_cutoff_hash: "d".repeat(64),
            revision_count: 0,
            policy_count: 0,
            target_tree: scan_tree(&self.staging_root).unwrap(),
            active_bytes_hash: "e".repeat(64),
            root_object_bytes_hash: "f".repeat(64),
        });
    }

    #[cfg(test)]
    fn force_post_exchange_failure(&mut self) {
        self.force_post_exchange_failure = true;
    }

    #[cfg(test)]
    fn force_backup_verification_failure(&mut self) {
        self.force_backup_verification_failure = true;
    }
}

#[cfg(test)]
fn post_exchange_failure_requested(target: &OfflineMigrationTarget) -> bool {
    target.force_post_exchange_failure
}

#[cfg(test)]
fn backup_verification_failure_requested(target: &OfflineMigrationTarget) -> bool {
    target.force_backup_verification_failure
}

#[cfg(not(test))]
fn backup_verification_failure_requested(_target: &OfflineMigrationTarget) -> bool {
    false
}

#[cfg(not(test))]
fn post_exchange_failure_requested(_target: &OfflineMigrationTarget) -> bool {
    false
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VaultTreeManifest(Vec<VaultTreeEntry>);

impl VaultTreeManifest {
    fn contains_top_level(&self, name: &str) -> bool {
        self.0.iter().any(|entry| entry.relative == Path::new(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct VaultTreeEntry {
    relative: PathBuf,
    kind: VaultTreeKind,
    mode: u32,
    size: u64,
    hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum VaultTreeKind {
    Directory,
    Regular,
}

const BACKUP_EVIDENCE_FILE: &str = ".plico-migration-backup-evidence.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupEvidence {
    schema: String,
    redacted_source_tree_hash: String,
    credential_role_cutoff_hash: String,
    target_root_hash: String,
    source_manifest_hash: String,
    migration_manifest_hash: String,
    revision_count: u64,
    policy_count: u64,
}

#[derive(Serialize)]
struct SafeTreeEntry<'a> {
    relative: &'a str,
    kind: &'static str,
    mode: u32,
    size: u64,
    content_hash: Option<&'a str>,
}

fn verify_published_seal_bytes(root: &Path, seal: &OfflineMigrationSeal) -> Result<bool, OfflineMigrationError> {
    let active = read_private_regular_noatime(&root.join("memory-ledger/roots/active"))?;
    let root_object = read_private_regular_noatime(&root.join("memory-ledger/objects").join(&seal.root_hash))?;
    Ok(sha256(&active) == seal.active_bytes_hash && sha256(&root_object) == seal.root_object_bytes_hash)
}

fn write_backup_evidence(
    backup_root: &Path,
    source_tree: &VaultTreeManifest,
    seal: &OfflineMigrationSeal,
) -> Result<(), OfflineMigrationError> {
    let evidence = backup_evidence(source_tree, seal)?;
    let bytes = serde_json_canonicalizer::to_vec(&evidence).map_err(|_| OfflineMigrationError::Invalid {
        category: "backup_evidence_canonicalization_failed",
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(backup_root)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(backup_root.join(BACKUP_EVIDENCE_FILE))
        .map_err(|error| error.error)?;
    sync_directory(backup_root)
}

fn verify_backup_evidence(
    backup_root: &Path,
    source_tree: &VaultTreeManifest,
    seal: &OfflineMigrationSeal,
) -> Result<bool, OfflineMigrationError> {
    let bytes = read_private_regular_noatime(&backup_root.join(BACKUP_EVIDENCE_FILE))?;
    let found: BackupEvidence = serde_json::from_slice(&bytes).map_err(|_| OfflineMigrationError::Invalid {
        category: "invalid_backup_evidence",
    })?;
    let mut actual_tree = scan_tree(backup_root)?;
    actual_tree
        .0
        .retain(|entry| entry.relative != Path::new(BACKUP_EVIDENCE_FILE));
    let expected = backup_evidence(source_tree, seal)?;
    Ok(private_backup_tree_matches(backup_root, &actual_tree, source_tree)
        && serde_json_canonicalizer::to_vec(&found).ok() == serde_json_canonicalizer::to_vec(&expected).ok())
}

fn tighten_backup_permissions(
    backup_root: &Path,
    source_tree: &VaultTreeManifest,
) -> Result<(), OfflineMigrationError> {
    if scan_tree(backup_root)? != *source_tree {
        return invalid("backup_changed_before_hardening");
    }
    set_path_mode_and_sync(backup_root, VaultTreeKind::Directory, 0o700)?;
    for entry in &source_tree.0 {
        let mode = match entry.kind {
            VaultTreeKind::Directory => 0o700,
            VaultTreeKind::Regular => 0o600,
        };
        set_path_mode_and_sync(&backup_root.join(&entry.relative), entry.kind, mode)?;
    }
    sync_directory(backup_root)?;
    if private_backup_tree_matches(backup_root, &scan_tree(backup_root)?, source_tree) {
        Ok(())
    } else {
        invalid("backup_permission_hardening_failed")
    }
}

fn restore_source_permissions(
    source_root: &Path,
    source_tree: &VaultTreeManifest,
    source_root_mode: u32,
) -> Result<(), OfflineMigrationError> {
    set_path_mode_and_sync(source_root, VaultTreeKind::Directory, 0o700)?;
    for entry in &source_tree.0 {
        set_path_mode_and_sync(&source_root.join(&entry.relative), entry.kind, entry.mode)?;
    }
    set_path_mode_and_sync(source_root, VaultTreeKind::Directory, source_root_mode)?;
    sync_directory(source_root)
}

fn set_path_mode_and_sync(path: &Path, expected_kind: VaultTreeKind, mode: u32) -> Result<(), OfflineMigrationError> {
    let flags = match expected_kind {
        VaultTreeKind::Directory => {
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME
        }
        VaultTreeKind::Regular => OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
    };
    let file = File::from(openat(CWD, path, flags, Mode::empty()).map_err(rustix_io)?);
    let metadata = file.metadata()?;
    let actual_kind = if metadata.file_type().is_dir() {
        VaultTreeKind::Directory
    } else if metadata.file_type().is_file() {
        VaultTreeKind::Regular
    } else {
        return invalid("backup_permission_target_not_regular");
    };
    if actual_kind != expected_kind {
        return invalid("backup_permission_target_type_changed");
    }
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    Ok(())
}

fn private_backup_tree_matches(backup_root: &Path, actual: &VaultTreeManifest, source: &VaultTreeManifest) -> bool {
    fs::symlink_metadata(backup_root)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && metadata.mode() & 0o777 == 0o700)
        && actual.0.len() == source.0.len()
        && actual.0.iter().zip(&source.0).all(|(actual, source)| {
            actual.relative == source.relative
                && actual.kind == source.kind
                && actual.size == source.size
                && actual.hash == source.hash
                && actual.mode & 0o777
                    == match actual.kind {
                        VaultTreeKind::Directory => 0o700,
                        VaultTreeKind::Regular => 0o600,
                    }
        })
}

fn backup_evidence(
    source_tree: &VaultTreeManifest,
    seal: &OfflineMigrationSeal,
) -> Result<BackupEvidence, OfflineMigrationError> {
    let safe: Vec<_> = source_tree
        .0
        .iter()
        .map(|entry| {
            let relative = entry.relative.to_str().ok_or(OfflineMigrationError::Invalid {
                category: "non_utf8_vault_path",
            })?;
            Ok(SafeTreeEntry {
                relative,
                kind: match entry.kind {
                    VaultTreeKind::Directory => "directory",
                    VaultTreeKind::Regular => "regular",
                },
                mode: entry.mode,
                size: entry.size,
                content_hash: (relative != "agent_tokens.json")
                    .then_some(entry.hash.as_deref())
                    .flatten(),
            })
        })
        .collect::<Result<_, OfflineMigrationError>>()?;
    let bytes = serde_json_canonicalizer::to_vec(&safe).map_err(|_| OfflineMigrationError::Invalid {
        category: "backup_evidence_canonicalization_failed",
    })?;
    let mut digest = Sha256::new();
    digest.update(b"plico.memory.migration-backup-tree.v1\0");
    digest.update(bytes);
    digest.update(seal.credential_role_cutoff_hash.as_bytes());
    Ok(BackupEvidence {
        schema: "plico.memory.migration-backup-evidence/v1".into(),
        redacted_source_tree_hash: format!("{:x}", digest.finalize()),
        credential_role_cutoff_hash: seal.credential_role_cutoff_hash.clone(),
        target_root_hash: seal.root_hash.clone(),
        source_manifest_hash: seal.source_manifest_hash.clone(),
        migration_manifest_hash: seal.migration_manifest_hash.clone(),
        revision_count: seal.revision_count,
        policy_count: seal.policy_count,
    })
}

fn scan_tree(root: &Path) -> Result<VaultTreeManifest, OfflineMigrationError> {
    require_real_directory(root)?;
    let root_device = fs::symlink_metadata(root)?.dev();
    let mut entries = Vec::new();
    for item in WalkDir::new(root)
        .follow_links(false)
        .same_file_system(true)
        .max_depth(MAX_VAULT_DEPTH)
        .sort_by_file_name()
    {
        let item = item.map_err(|_| OfflineMigrationError::Invalid {
            category: "vault_tree_walk_failed",
        })?;
        if item.depth() == 0 {
            continue;
        }
        let metadata = fs::symlink_metadata(item.path())?;
        if metadata.dev() != root_device {
            return invalid("vault_tree_crosses_filesystem");
        }
        let file_type = metadata.file_type();
        let relative = item
            .path()
            .strip_prefix(root)
            .map_err(|_| OfflineMigrationError::Invalid {
                category: "vault_tree_path_escape",
            })?
            .to_path_buf();
        if relative
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return invalid("vault_tree_path_escape");
        }
        let (kind, size, hash) = if file_type.is_dir() && !file_type.is_symlink() {
            (VaultTreeKind::Directory, 0, None)
        } else if file_type.is_file()
            && !file_type.is_symlink()
            && !file_type.is_block_device()
            && !file_type.is_char_device()
            && !file_type.is_fifo()
            && !file_type.is_socket()
        {
            let bytes = read_regular_noatime(item.path())?;
            (VaultTreeKind::Regular, metadata.len(), Some(sha256(&bytes)))
        } else {
            return invalid("vault_tree_contains_special_entry");
        };
        entries.push(VaultTreeEntry {
            relative,
            kind,
            mode: metadata.mode() & 0o7777,
            size,
            hash,
        });
    }
    entries.sort();
    Ok(VaultTreeManifest(entries))
}

fn remove_copied_legacy_marker(staging_root: &Path) -> Result<(), OfflineMigrationError> {
    let marker = staging_root.join("memory_index.json");
    let metadata = fs::symlink_metadata(&marker)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return invalid("invalid_legacy_marker");
    }
    fs::remove_file(marker)?;
    Ok(())
}

fn reserve_backup_path(parent: &Path, source: &Path) -> Result<PathBuf, OfflineMigrationError> {
    let basename = source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(OfflineMigrationError::Invalid {
            category: "invalid_vault_basename",
        })?;
    for nonce in 0_u32..1_000 {
        let candidate = parent.join(format!("{basename}.pre-ledger-backup.{nonce}"));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => {}
            Err(error) => return Err(error.into()),
        }
    }
    invalid("backup_name_exhausted")
}

fn rollback_exchange<T>(
    vault: &OfflineMigrationVault,
    mut target: OfflineMigrationTarget,
    category: &'static str,
) -> Result<T, OfflineMigrationError> {
    renameat_with(CWD, &vault.vault_root, CWD, &target.staging_root, RenameFlags::EXCHANGE).map_err(|_| {
        OfflineMigrationError::Invalid {
            category: "rollback_exchange_failed",
        }
    })?;
    sync_directory(&vault.vault_parent)?;
    sync_directory(target.container_path())?;
    target.allow_container_cleanup();
    invalid(category)
}

fn rollback_after_evidence<T>(
    vault: &OfflineMigrationVault,
    target: OfflineMigrationTarget,
    category: &'static str,
) -> Result<T, OfflineMigrationError> {
    match fs::remove_file(target.staging_root.join(BACKUP_EVIDENCE_FILE)) {
        Ok(()) => sync_directory(&target.staging_root)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    restore_source_permissions(&target.staging_root, &target.source_tree, target.source_root_mode)?;
    if scan_tree(&target.staging_root)? != target.source_tree
        || fs::symlink_metadata(&target.staging_root)?.mode() & 0o7777 != target.source_root_mode
    {
        return invalid("rollback_permission_restore_failed");
    }
    rollback_exchange(vault, target, category)
}

fn rollback_finalized<T>(
    vault: &OfflineMigrationVault,
    target: OfflineMigrationTarget,
    backup: &Path,
    category: &'static str,
) -> Result<T, OfflineMigrationError> {
    renameat_with(CWD, backup, CWD, &target.staging_root, RenameFlags::NOREPLACE).map_err(|_| {
        OfflineMigrationError::Invalid {
            category: "rollback_backup_restore_failed",
        }
    })?;
    rollback_after_evidence(vault, target, category)
}

fn read_regular_noatime(path: &Path) -> Result<Vec<u8>, OfflineMigrationError> {
    read_path_noatime(path, false)
}

fn read_private_regular_noatime(path: &Path) -> Result<Vec<u8>, OfflineMigrationError> {
    read_path_noatime(path, true)
}

fn read_path_noatime(path: &Path, require_private: bool) -> Result<Vec<u8>, OfflineMigrationError> {
    let fd = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    let mut file = File::from(fd);
    let metadata = file.metadata()?;
    let file_type = metadata.file_type();
    if !file_type.is_file()
        || file_type.is_symlink()
        || file_type.is_block_device()
        || file_type.is_char_device()
        || file_type.is_fifo()
        || file_type.is_socket()
    {
        return invalid("migration_evidence_not_regular_file");
    }
    if require_private && metadata.mode() & 0o777 != 0o600 {
        return invalid("migration_evidence_not_private");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn open_or_create_lock(lock_path: &Path, parent: &Path) -> Result<(File, bool), OfflineMigrationError> {
    let create_flags = OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    match openat(CWD, lock_path, create_flags, Mode::RUSR | Mode::WUSR) {
        Ok(fd) => {
            let file = File::from(fd);
            file.sync_all()?;
            sync_directory(parent)?;
            validate_lock_mode(&file.metadata()?)?;
            Ok((file, true))
        }
        Err(error) if error.raw_os_error() == libc::EEXIST => {
            let metadata = fs::symlink_metadata(lock_path)?;
            require_lock_metadata(&metadata)?;
            let fd = openat(
                CWD,
                lock_path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(rustix_io)?;
            let file = File::from(fd);
            validate_lock_mode(&file.metadata()?)?;
            Ok((file, false))
        }
        Err(error) => Err(OfflineMigrationError::Io(rustix_io(error))),
    }
}

fn validate_open_lock_identity(file: &File, lock_path: &Path) -> Result<(), OfflineMigrationError> {
    let opened = file.metadata()?;
    let path = fs::symlink_metadata(lock_path)?;
    require_lock_metadata(&path)?;
    #[cfg(unix)]
    if opened.dev() != path.dev() || opened.ino() != path.ino() {
        return invalid("vault_lock_replaced_during_open");
    }
    Ok(())
}

fn require_lock_metadata(metadata: &fs::Metadata) -> Result<(), OfflineMigrationError> {
    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
        validate_lock_mode(metadata)
    } else {
        invalid("invalid_vault_lock_file")
    }
}

#[cfg(unix)]
fn validate_lock_mode(metadata: &fs::Metadata) -> Result<(), OfflineMigrationError> {
    if metadata.mode() & 0o777 == 0o600 {
        Ok(())
    } else {
        invalid("invalid_vault_lock_permissions")
    }
}

#[cfg(not(unix))]
fn validate_lock_mode(_metadata: &fs::Metadata) -> Result<(), OfflineMigrationError> {
    invalid("offline_migration_requires_unix_permissions")
}

fn require_real_directory(path: &Path) -> Result<(), OfflineMigrationError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        invalid("legacy_path_is_not_real_directory")
    }
}

fn open_real_directory(path: &Path) -> Result<File, OfflineMigrationError> {
    let fd = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    Ok(File::from(fd))
}

fn open_real_directory_at(directory: &File, name: &str) -> Result<File, OfflineMigrationError> {
    let fd = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    Ok(File::from(fd))
}

fn read_regular_noatime_at(directory: &File, name: &str) -> Result<Vec<u8>, OfflineMigrationError> {
    let fd = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    let mut file = File::from(fd);
    let metadata = file.metadata()?;
    #[cfg(unix)]
    let regular = metadata.file_type().is_file()
        && !metadata.file_type().is_symlink()
        && !metadata.file_type().is_block_device()
        && !metadata.file_type().is_char_device()
        && !metadata.file_type().is_fifo()
        && !metadata.file_type().is_socket();
    #[cfg(not(unix))]
    let regular = metadata.file_type().is_file();
    if !regular {
        return invalid("legacy_source_is_not_regular_file");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn read_private_regular_noatime_at(directory: &File, name: &str) -> Result<Vec<u8>, OfflineMigrationError> {
    let fd = openat(
        directory,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    let mut file = File::from(fd);
    validate_private_regular_mode(&file.metadata()?)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(unix)]
fn validate_private_regular_mode(metadata: &fs::Metadata) -> Result<(), OfflineMigrationError> {
    if metadata.file_type().is_file() && metadata.mode() & 0o777 == 0o600 {
        Ok(())
    } else {
        invalid("invalid_credential_file_permissions")
    }
}

#[cfg(not(unix))]
fn validate_private_regular_mode(_metadata: &fs::Metadata) -> Result<(), OfflineMigrationError> {
    invalid("offline_migration_requires_unix_permissions")
}

fn sync_directory(path: &Path) -> Result<(), OfflineMigrationError> {
    let fd = openat(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(rustix_io)?;
    File::from(fd).sync_all()?;
    Ok(())
}

fn validate_fingerprint(expected: &OfflineSourceFingerprint) -> Result<(), OfflineMigrationError> {
    validate_hash(&expected.source_index_hash)?;
    if expected.snapshots.windows(2).any(|pair| {
        (&pair[0].legacy_agent_id, &pair[0].legacy_tier, &pair[0].cid)
            >= (&pair[1].legacy_agent_id, &pair[1].legacy_tier, &pair[1].cid)
    }) {
        return invalid("invalid_source_fingerprint_order");
    }
    for snapshot in &expected.snapshots {
        if snapshot.legacy_agent_id.trim().is_empty() || snapshot.legacy_tier.trim().is_empty() {
            return invalid("invalid_source_fingerprint_identity");
        }
        validate_hash(&snapshot.cid)?;
        validate_hash(&snapshot.object_envelope_hash)?;
    }
    if expected
        .referenced_objects
        .windows(2)
        .any(|pair| pair[0].cid >= pair[1].cid)
    {
        return invalid("invalid_source_fingerprint_order");
    }
    for object in &expected.referenced_objects {
        validate_hash(&object.cid)?;
        validate_hash(&object.object_envelope_hash)?;
    }
    Ok(())
}

fn validate_hash(value: &str) -> Result<(), OfflineMigrationError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        invalid("invalid_offline_migration_hash")
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn rustix_io(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

fn invalid<T>(category: &'static str) -> Result<T, OfflineMigrationError> {
    Err(OfflineMigrationError::Invalid { category })
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::{symlink, PermissionsExt};

    use tempfile::tempdir;

    use super::*;

    fn loose_legacy_vault(parent: &Path) -> PathBuf {
        let vault = parent.join("vault");
        fs::create_dir(&vault).unwrap();
        fs::set_permissions(&vault, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\":{}}").unwrap();
        fs::set_permissions(vault.join("memory_index.json"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(vault.join("agent_tokens.json"), b"{}").unwrap();
        fs::set_permissions(vault.join("agent_tokens.json"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::create_dir(vault.join("cas")).unwrap();
        fs::set_permissions(vault.join("cas"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(vault.join("cas/legacy-object"), b"private-memory").unwrap();
        fs::set_permissions(vault.join("cas/legacy-object"), fs::Permissions::from_mode(0o644)).unwrap();
        vault
    }

    fn seal_test_target(target: &mut OfflineMigrationTarget) {
        let root_hash = "a".repeat(64);
        target.ledger_storage().put_immutable(&root_hash, b"root").unwrap();
        target.ledger_storage().publish_active(b"pointer").unwrap();
        target
            .seal(root_hash, "b".repeat(64), "c".repeat(64), "d".repeat(64), 0, 0)
            .unwrap();
    }

    fn empty_fingerprint() -> OfflineSourceFingerprint {
        OfflineSourceFingerprint {
            source_index_hash: sha256(b"{\"agents\":{}}"),
            snapshots: vec![],
            referenced_objects: vec![],
        }
    }

    #[test]
    fn creates_exact_shared_lock_and_reads_without_runtime_access_tracking() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\":{}}").unwrap();
        let cas = vault.join("cas");
        let cid = sha256(b"payload");
        fs::create_dir(&cas).unwrap();
        fs::create_dir(cas.join(&cid[..2])).unwrap();
        fs::write(cas.join(&cid[..2]).join(&cid[2..]), b"object-envelope").unwrap();

        let opened = OfflineMigrationVault::open(&vault).unwrap();
        assert!(opened.lock_created());
        assert_eq!(opened.read_legacy_index_bytes().unwrap(), b"{\"agents\":{}}");
        assert_eq!(opened.read_legacy_object_bytes(&cid).unwrap(), b"object-envelope");
        let lock = parent.path().join(".vault.plico-vault.lock");
        #[cfg(unix)]
        assert_eq!(fs::metadata(lock).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn rejects_existing_bad_mode_and_symlink_without_repair() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        let lock = parent.path().join(".vault.plico-vault.lock");
        let mut file = File::create(&lock).unwrap();
        file.write_all(b"unchanged").unwrap();
        #[cfg(unix)]
        fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            OfflineMigrationVault::open(&vault),
            Err(OfflineMigrationError::Invalid {
                category: "invalid_vault_lock_permissions"
            })
        ));
        assert_eq!(fs::read(&lock).unwrap(), b"unchanged");

        fs::remove_file(&lock).unwrap();
        #[cfg(unix)]
        symlink(vault.join("memory_index.json"), &lock).unwrap();
        assert!(matches!(
            OfflineMigrationVault::open(&vault),
            Err(OfflineMigrationError::Invalid {
                category: "invalid_vault_lock_file"
            })
        ));
    }

    #[test]
    fn revalidate_detects_exact_envelope_change() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        let index = b"{\"agents\":{}}";
        fs::write(vault.join("memory_index.json"), index).unwrap();
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let expected = OfflineSourceFingerprint {
            source_index_hash: sha256(index),
            snapshots: vec![],
            referenced_objects: vec![],
        };
        opened.revalidate_source(&expected).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\": {}}").unwrap();
        assert!(matches!(
            opened.revalidate_source(&expected),
            Err(OfflineMigrationError::Invalid {
                category: "legacy_source_changed"
            })
        ));
    }

    #[test]
    fn staging_copy_is_exact_and_unsealed_target_cannot_publish() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\":{}}").unwrap();
        fs::write(vault.join("agent_tokens.json"), b"{}").unwrap();
        #[cfg(unix)]
        fs::set_permissions(vault.join("agent_tokens.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let target = opened.prepare_target().unwrap();
        let fingerprint = OfflineSourceFingerprint {
            source_index_hash: sha256(b"{\"agents\":{}}"),
            snapshots: vec![],
            referenced_objects: vec![],
        };
        assert!(matches!(
            opened.publish_target(target, &fingerprint, |_| Ok(())),
            Err(OfflineMigrationError::Invalid {
                category: "unsealed_staging_target"
            })
        ));
        assert!(vault.join("memory_index.json").is_file());
        assert!(!vault.join("memory-ledger").exists());
    }

    #[test]
    fn tampered_seal_rejects_before_exchange_and_preserves_live_source() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\":{}}").unwrap();
        fs::write(vault.join("agent_tokens.json"), b"{}").unwrap();
        #[cfg(unix)]
        fs::set_permissions(vault.join("agent_tokens.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let mut target = opened.prepare_target().unwrap();
        target.seal_for_test();
        let fingerprint = OfflineSourceFingerprint {
            source_index_hash: sha256(b"{\"agents\":{}}"),
            snapshots: vec![],
            referenced_objects: vec![],
        };
        assert!(opened.publish_target(target, &fingerprint, |_| Ok(())).is_err());
        assert!(vault.join("memory_index.json").is_file());
        assert!(!vault.join("memory-ledger").exists());
    }

    #[test]
    fn post_exchange_verification_failure_rolls_back_live_source() {
        let parent = tempdir().unwrap();
        let vault = parent.path().join("vault");
        fs::create_dir(&vault).unwrap();
        fs::write(vault.join("memory_index.json"), b"{\"agents\":{}}").unwrap();
        fs::write(vault.join("agent_tokens.json"), b"{}").unwrap();
        #[cfg(unix)]
        fs::set_permissions(vault.join("agent_tokens.json"), fs::Permissions::from_mode(0o600)).unwrap();
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let mut target = opened.prepare_target().unwrap();
        seal_test_target(&mut target);
        target.force_post_exchange_failure();
        let fingerprint = OfflineSourceFingerprint {
            source_index_hash: sha256(b"{\"agents\":{}}"),
            snapshots: vec![],
            referenced_objects: vec![],
        };
        assert!(opened.publish_target(target, &fingerprint, |_| Ok(())).is_err());
        assert!(vault.join("memory_index.json").is_file());
        assert!(!vault.join("memory-ledger").exists());
    }

    #[test]
    fn successful_backup_is_owner_only_even_when_legacy_tree_was_world_readable() {
        let parent = tempdir().unwrap();
        let vault = loose_legacy_vault(parent.path());
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let mut target = opened.prepare_target().unwrap();
        seal_test_target(&mut target);
        let publication = opened.publish_target(target, &empty_fingerprint(), |_| Ok(())).unwrap();
        let backup = parent.path().join(publication.backup_name);
        assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o700);
        for entry in WalkDir::new(&backup).follow_links(false) {
            let entry = entry.unwrap();
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            if entry.depth() == 0 || metadata.file_type().is_dir() {
                assert_eq!(metadata.mode() & 0o777, 0o700, "{}", entry.path().display());
            } else {
                assert_eq!(metadata.mode() & 0o777, 0o600, "{}", entry.path().display());
            }
        }
    }

    #[test]
    fn backup_failure_restores_original_tree_permissions_before_rollback() {
        let parent = tempdir().unwrap();
        let vault = loose_legacy_vault(parent.path());
        let opened = OfflineMigrationVault::open(&vault).unwrap();
        let mut target = opened.prepare_target().unwrap();
        seal_test_target(&mut target);
        target.force_backup_verification_failure();
        assert!(opened.publish_target(target, &empty_fingerprint(), |_| Ok(())).is_err());
        assert!(vault.join("memory_index.json").is_file());
        assert!(!vault.join("memory-ledger").exists());
        assert_eq!(fs::metadata(&vault).unwrap().mode() & 0o777, 0o755);
        assert_eq!(fs::metadata(vault.join("cas")).unwrap().mode() & 0o777, 0o755);
        assert_eq!(
            fs::metadata(vault.join("memory_index.json")).unwrap().mode() & 0o777,
            0o644
        );
        assert_eq!(
            fs::metadata(vault.join("cas/legacy-object")).unwrap().mode() & 0o777,
            0o644
        );
        assert_eq!(fs::read(vault.join("cas/legacy-object")).unwrap(), b"private-memory");
    }
}
