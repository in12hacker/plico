//! Owner-only immutable storage for derived projection artifacts.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(target_os = "linux")]
use rustix::fd::AsFd;
#[cfg(target_os = "linux")]
use rustix::fs::{fchmod, fstat, fsync, openat2, statat, unlinkat, AtFlags, Dir, FileType, ResolveFlags};
use rustix::fs::{open, renameat_with, Mode, OFlags, RenameFlags, CWD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ledger_store::{
    create_private_file, ensure_directory, inspect_projection_layout_at, open_existing_immutable_ledger,
    open_immutable_ledger_directory, put_immutable_at, read_private_file_bounded, set_file_mode, sync_directory,
    validate_hash, ExistingProjectionLayout, ImmutableLedgerNamespace, ImmutableLedgerStorage, PersonalVaultLease,
    PersonalVaultStorage,
};
use super::{LedgerStorageError, LedgerStorageOpenError};

const PROJECTION_STORE_DIRECTORY: &str = "projection-store";
const PROJECTION_MANIFEST_DIRECTORY: &str = "manifest";
const PROJECTION_ARTIFACT_DIRECTORY: &str = "artifacts";
const PROJECTION_PAIR_SEAL_SCHEMA: &str = "plico.projection.pair-seal/v1";
const PROJECTION_PAIR_SEAL_FILE: &str = "projection-pair-seal.json";
const PROJECTION_PAIR_TREE_DOMAIN: &[u8] = b"plico.projection.pair-tree.v1\0";
const PROJECTION_PAIR_SEAL_DOMAIN: &[u8] = b"plico.projection.pair-seal.v1\0";
const PROJECTION_PAIR_ACTIVE_DOMAIN: &[u8] = b"plico.projection.pair-active.v1\0";
const PROJECTION_RESET_MARKER_SCHEMA: &str = "plico.projection.reset-marker/v1";
const PROJECTION_RESET_MARKER_FILE: &str = ".plico-projection-reset-active.json";
const PROJECTION_RESET_MARKER_TRANSITION_PREFIX: &str = ".plico-projection-reset-marker-transition.";
const PROJECTION_RESET_MARKER_DOMAIN: &[u8] = b"plico.projection.reset-marker.v1\0";
const PROJECTION_RESET_LIVE_DOMAIN: &[u8] = b"plico.projection.reset-live.v1\0";
const MAX_PROJECTION_PAIR_TREE_ENTRIES: usize = 16;
const MAX_PROJECTION_PAIR_TREE_DEPTH: usize = 4;
const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub(super) const MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES: usize = 4096;

fn trace_projection_reset(
    phase: &'static str,
    outcome: &'static str,
    result_category: &'static str,
    reset_reason: &'static str,
) {
    tracing::debug!(
        operation = "projection_reset",
        phase,
        outcome,
        result_category,
        reset_reason
    );
}

fn projection_reset_reason_trace_value(value: &str) -> &'static str {
    match value {
        "manifest_incomplete" => "manifest_incomplete",
        "manifest_integrity_invalid" => "manifest_integrity_invalid",
        "storage_layout_invalid" => "storage_layout_invalid",
        "canonical_lineage_invalid" => "canonical_lineage_invalid",
        _ => "invalid",
    }
}
#[cfg(test)]
const RESET_FAULT_AFTER_PAIR_EXCHANGE: u8 = 1;
#[cfg(test)]
const RESET_FAULT_AFTER_TRANSITION_PERSIST: u8 = 2;
#[cfg(test)]
const RESET_FAULT_AFTER_MARKER_EXCHANGE: u8 = 3;
#[cfg(test)]
const RESET_FAULT_AFTER_QUARANTINE_PAIR_REMOVAL: u8 = 4;
#[cfg(test)]
const RESET_FAULT_AFTER_SEAL_REMOVAL: u8 = 5;
#[cfg(test)]
const RESET_FAULT_AFTER_CONTAINER_REMOVAL: u8 = 6;
#[cfg(test)]
const RESET_FAULT_AFTER_PREPARED_MARKER_PERSIST: u8 = 7;
#[cfg(test)]
const RESET_FAULT_AFTER_ACTIVE_MARKER_UNLINK: u8 = 8;
#[cfg(test)]
const RESET_FAULT_PREFLIGHT_PERMISSION_DENIED: u8 = 9;
#[cfg(test)]
const RESET_FAULT_QUARANTINE_PRE_MUTATION_IO: u8 = 10;

#[derive(Debug)]
pub(crate) struct ProjectionArtifactStorage {
    _lease: Arc<PersonalVaultLease>,
    artifact_directory: PathBuf,
    objects_directory: PathBuf,
    #[cfg(test)]
    fail_after_durable_put_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_cleanup_once: std::sync::atomic::AtomicBool,
}

/// Opaque, single-owner capability that keeps the projection manifest and
/// artifact namespaces under one lifecycle claim.
pub(crate) struct ProjectionStorageBundle {
    manifest: ImmutableLedgerStorage,
    artifacts: ProjectionArtifactStorage,
    reset_operation_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionPairPublishMode {
    CreateAbsent,
    ReplaceExisting,
}

pub(crate) struct ProjectionPairTarget {
    lease: Arc<PersonalVaultLease>,
    container: Option<tempfile::TempDir>,
    pair_path: PathBuf,
    container_directory: File,
    pair_directory: File,
    directory_identity: ProjectionPairDirectoryIdentity,
    pair_identity: ProjectionPairDirectoryIdentity,
    expected_live_identity: Option<ProjectionPairEntryIdentity>,
    expected_live_fingerprint: Option<String>,
    reset_reason: Option<ProjectionPairResetReason>,
    reset_transaction_id: Option<uuid::Uuid>,
    storage: Option<ProjectionStorageBundle>,
    seal: Option<ProjectionPairSeal>,
    publish_mode: ProjectionPairPublishMode,
    claims_reserved: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProjectionPairDirectoryIdentity {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProjectionPairEntryIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    kind: u8,
}

struct ProjectionResetAppliedTopology<'a> {
    lease: &'a PersonalVaultLease,
    vault_root: &'a Path,
    container_path: &'a Path,
    container_identity: ProjectionPairDirectoryIdentity,
    target_pair_identity: ProjectionPairDirectoryIdentity,
    expected_live_identity: ProjectionPairEntryIdentity,
}

pub(crate) enum ProjectionPairPublication {
    ReadyStorage(Box<ProjectionStorageBundle>),
    ResetMaintenance(Box<ProjectionResetMaintenance>),
}

pub(crate) struct ProjectionResetMaintenance {
    lease: Arc<PersonalVaultLease>,
    storage: Option<ProjectionStorageBundle>,
    container_path: PathBuf,
    container_directory: Option<File>,
    container_identity: ProjectionPairDirectoryIdentity,
    marker: ProjectionResetMarker,
}

pub(crate) enum ProjectionResetMaintenanceError {
    ManualIntervention,
    CommitIndeterminate,
    UnsupportedFormat,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionPairResetReason {
    ManifestIncomplete,
    ManifestIntegrityInvalid,
    StorageLayoutInvalid,
    CanonicalLineageInvalid,
}

impl ProjectionPairResetReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ManifestIncomplete => "manifest_incomplete",
            Self::ManifestIntegrityInvalid => "manifest_integrity_invalid",
            Self::StorageLayoutInvalid => "storage_layout_invalid",
            Self::CanonicalLineageInvalid => "canonical_lineage_invalid",
        }
    }
}

pub(crate) enum ProjectionClaimedLiveInspection<R> {
    StorageLayoutInvalid,
    Readable(R),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProjectionPairGenesisEvidence {
    projection_root_hash: String,
    current_view_hash: String,
    canonical_genesis_root_hash: String,
    canonical_generation: u64,
    canonical_revision_watermark: u64,
    canonical_policy_watermark: u64,
    canonical_relation_watermark: u64,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionPairSeal {
    payload: ProjectionPairSealPayload,
    seal_hash: String,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionPairSealPayload {
    schema: String,
    genesis: ProjectionPairGenesisEvidence,
    tree_digest: String,
    active_pointer_digest: String,
    artifact_count: u64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionResetMarker {
    payload: ProjectionResetMarkerPayload,
    marker_hash: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionResetMarkerPayload {
    schema: String,
    transaction_id: String,
    phase: ProjectionResetMarkerPhase,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ProjectionResetMarkerPhase {
    Prepared {
        binding: ProjectionResetPreparedBinding,
    },
    AppliedMaintenance {
        binding: ProjectionResetPreparedBinding,
        new_live_identity: ProjectionResetMarkerEntryIdentity,
        quarantine_container_identity: ProjectionResetMarkerEntryIdentity,
        quarantined_entry_identity: ProjectionResetMarkerEntryIdentity,
    },
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionResetPreparedBinding {
    staging_container_basename: String,
    transition_evidence_basename: String,
    staging_container_identity: ProjectionResetMarkerEntryIdentity,
    target_pair_identity: ProjectionResetMarkerEntryIdentity,
    target_seal_hash: String,
    target_tree_digest: String,
    target_active_pointer_digest: String,
    expected_live_identity: ProjectionResetMarkerEntryIdentity,
    expected_live_fingerprint: String,
    reset_reason: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectionResetMarkerEntryIdentity {
    device: String,
    inode: String,
    mode: u32,
    kind: u8,
}

enum ProjectionResetMarkerWriteError {
    PrePublish(std::io::Error),
    Pending,
}

enum ProjectionResetRecoveryError {
    Manual,
    Indeterminate,
    UnsupportedFormat,
    Unavailable,
}

enum ProjectionResetEvidenceMutationError {
    Manual,
    Indeterminate,
    UnsupportedFormat,
    Unavailable,
}

enum ProjectionResetProtocolError {
    UnsupportedFormat,
    Invalid,
    Io(std::io::Error),
}

#[derive(Serialize)]
struct ProjectionPairTreeRow {
    relative_path: String,
    kind: &'static str,
    mode: u32,
    size: u64,
    content_digest: Option<String>,
}

#[derive(Serialize)]
struct ProjectionResetLiveRow {
    relative_path: String,
    kind: &'static str,
    mode: u32,
    size: u64,
    device: String,
    inode: String,
    content_evidence: Option<ProjectionResetContentEvidence>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ProjectionResetContentEvidence {
    Digest { sha256: String },
    Oversize,
    NonPrivateMode,
    SymlinkTarget { sha256: String },
    Special,
}

struct ProjectionResetLiveFingerprint {
    digest: String,
    storage_layout_invalid: bool,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArtifactIdentity {
    device: u64,
    inode: u64,
    mode: u32,
    length: u64,
}

#[cfg(unix)]
pub(crate) struct ProjectionArtifactSnapshot {
    hash: String,
    identity: Option<ArtifactIdentity>,
    file: Option<File>,
    bytes: Option<Vec<u8>>,
}

#[cfg(all(test, unix))]
pub(crate) struct ProjectionExternalCanaryFingerprint {
    path: PathBuf,
    identity: ArtifactIdentity,
    bytes: Vec<u8>,
}

#[cfg(unix)]
impl ProjectionArtifactSnapshot {
    pub(crate) fn is_missing(&self) -> bool {
        self.identity.is_none()
    }

    pub(crate) fn bytes(&self) -> Option<&[u8]> {
        self.bytes.as_deref()
    }

    pub(crate) fn is_private_regular(&self) -> bool {
        self.file.is_some() && self.identity.is_some_and(|identity| identity.mode & 0o777 == 0o600)
    }
}

impl PersonalVaultStorage {
    pub(crate) fn open_existing_projection_writer(&self) -> Result<ProjectionStorageBundle, LedgerStorageOpenError> {
        self.claim_projection_writer()
    }

    pub(crate) fn recover_projection_reset_maintenance(
        &self,
    ) -> Result<ProjectionResetMaintenance, LedgerStorageOpenError> {
        let (lease, _) = self.projection_artifact_parts();
        let _lifecycle = lease
            .projection_lifecycle
            .lock()
            .map_err(|_| std::io::Error::other("projection lifecycle state is poisoned"))?;
        let mut namespace_claims = lease
            .claimed_namespaces
            .lock()
            .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
        let mut artifacts_claimed = lease
            .projection_artifacts_claimed
            .lock()
            .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
        if namespace_claims.contains(&ImmutableLedgerNamespace::ProjectionManifest) || *artifacts_claimed {
            return Err(LedgerStorageOpenError::NamespaceAlreadyClaimed);
        }
        namespace_claims.insert(ImmutableLedgerNamespace::ProjectionManifest);
        *artifacts_claimed = true;
        drop(artifacts_claimed);
        drop(namespace_claims);
        let mut recovery_indeterminate = false;
        let mut recovery_reason = "invalid";
        let mut recovery_operation_id = None;
        let result = (|| {
            let marker = read_projection_reset_marker(&lease.vault_root)?
                .ok_or(LedgerStorageOpenError::ProjectionResetManualIntervention)?;
            recovery_operation_id = Some(marker.payload.transaction_id.clone());
            recovery_reason = projection_reset_reason_trace_value(&marker_binding(&marker).reset_reason);
            #[cfg(test)]
            if take_projection_reset_fault(&lease, RESET_FAULT_PREFLIGHT_PERMISSION_DENIED) {
                return Err(LedgerStorageOpenError::Io(std::io::Error::from_raw_os_error(13)));
            }
            preflight_projection_reset_protocol(&lease.vault_root, &marker)?;
            let reset_span = tracing::debug_span!(
                "projection_reset_operation",
                operation = "projection_reset",
                reset_operation_id = %marker.payload.transaction_id
            );
            let _reset_span = reset_span.enter();
            trace_projection_reset(
                "recovery",
                "started",
                match &marker.payload.phase {
                    ProjectionResetMarkerPhase::Prepared { .. } => "prepared",
                    ProjectionResetMarkerPhase::AppliedMaintenance { .. } => "applied_maintenance",
                },
                recovery_reason,
            );
            let marker = match &marker.payload.phase {
                ProjectionResetMarkerPhase::Prepared { .. } => {
                    match recover_prepared_projection_reset(&lease, &marker) {
                        Ok(applied) => applied,
                        Err(ProjectionResetRecoveryError::Manual) => {
                            return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
                        }
                        Err(ProjectionResetRecoveryError::Indeterminate) => {
                            recovery_indeterminate = true;
                            return Err(LedgerStorageOpenError::ProjectionResetIndeterminate);
                        }
                        Err(ProjectionResetRecoveryError::UnsupportedFormat) => {
                            return Err(LedgerStorageOpenError::UnsupportedProjectionFormat);
                        }
                        Err(ProjectionResetRecoveryError::Unavailable) => {
                            return Err(LedgerStorageOpenError::Io(std::io::Error::other(
                                "projection reset protocol storage is unavailable",
                            )));
                        }
                    }
                }
                ProjectionResetMarkerPhase::AppliedMaintenance { .. } => marker,
            };
            verify_projection_reset_live(&lease.vault_root, &marker, None)
                .map_err(|_| LedgerStorageOpenError::ProjectionResetManualIntervention)?;
            let storage = open_projection_bundle_at(
                Arc::clone(&lease),
                &lease.vault_root.join(PROJECTION_STORE_DIRECTORY),
                false,
            )?;
            let binding = marker_binding(&marker);
            let container_path = lease.vault_root.join(&binding.staging_container_basename);
            let (container_directory, container_identity) = match std::fs::symlink_metadata(&container_path) {
                Ok(_) => {
                    let (directory, identity) = open_private_directory_identity(&container_path)?;
                    if reset_marker_directory_identity(identity) != binding.staging_container_identity {
                        return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
                    }
                    (Some(directory), identity)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (
                    None,
                    marker_directory_identity(&binding.staging_container_identity)
                        .ok_or(LedgerStorageOpenError::ProjectionResetManualIntervention)?,
                ),
                Err(error) => return Err(error.into()),
            };
            Ok(ProjectionResetMaintenance {
                lease: Arc::clone(&lease),
                storage: Some(storage),
                container_path,
                container_directory,
                container_identity,
                marker,
            })
        })();
        if let Some(operation_id) = recovery_operation_id.as_ref() {
            let recovery_terminal_span = tracing::debug_span!(
                "projection_reset_operation",
                operation = "projection_reset",
                reset_operation_id = %operation_id
            );
            let _recovery_terminal_span = recovery_terminal_span.enter();
            match &result {
                Ok(_) => trace_projection_reset("recovery", "ok", "maintenance_capability", recovery_reason),
                Err(_) if recovery_indeterminate => {
                    trace_projection_reset("recovery", "indeterminate", "mutation_unverified", recovery_reason);
                }
                Err(LedgerStorageOpenError::UnsupportedProjectionFormat | LedgerStorageOpenError::RejectedMarker) => {
                    trace_projection_reset("recovery", "unsupported", "upgrade_required", recovery_reason);
                }
                Err(LedgerStorageOpenError::ProjectionResetManualIntervention) => {
                    trace_projection_reset("recovery", "manual", "evidence_invalid", recovery_reason);
                }
                Err(LedgerStorageOpenError::ProjectionResetPending) => {
                    trace_projection_reset("recovery", "pending", "reset_pending", recovery_reason);
                }
                Err(LedgerStorageOpenError::ProjectionResetMaintenanceRequired) => {
                    trace_projection_reset("recovery", "pending", "maintenance_required", recovery_reason);
                }
                Err(LedgerStorageOpenError::ProjectionResetIndeterminate) => {
                    trace_projection_reset("recovery", "indeterminate", "mutation_unverified", recovery_reason);
                }
                Err(LedgerStorageOpenError::NamespaceAlreadyClaimed) => {
                    trace_projection_reset("recovery", "unavailable", "vault_locked", recovery_reason);
                }
                Err(LedgerStorageOpenError::Io(_)) => {
                    trace_projection_reset("recovery", "unavailable", "storage_io", recovery_reason);
                }
            }
        }
        if result.is_err() && !recovery_indeterminate {
            rollback_projection_claims(&lease)?;
        }
        result
    }

    fn claim_projection_writer(&self) -> Result<ProjectionStorageBundle, LedgerStorageOpenError> {
        let (lease, artifact_directory) = self.projection_artifact_parts();
        let _lifecycle = lease
            .projection_lifecycle
            .lock()
            .map_err(|_| std::io::Error::other("projection lifecycle state is poisoned"))?;
        let mut namespace_claims = lease
            .claimed_namespaces
            .lock()
            .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
        let mut artifacts_claimed = lease
            .projection_artifacts_claimed
            .lock()
            .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
        if namespace_claims.contains(&ImmutableLedgerNamespace::ProjectionManifest) || *artifacts_claimed {
            return Err(LedgerStorageOpenError::NamespaceAlreadyClaimed);
        }
        reject_projection_reset_pending(&lease.vault_root)?;
        for legacy_name in ["projection-manifest", "projection-artifacts"] {
            if !std::fs::symlink_metadata(lease.vault_root.join(legacy_name))
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "legacy projection storage layout is unsupported",
                )
                .into());
            }
        }
        let projection_store = lease.vault_root.join("projection-store");
        require_private_directory(&projection_store)?;
        namespace_claims.insert(ImmutableLedgerNamespace::ProjectionManifest);
        *artifacts_claimed = true;
        drop(artifacts_claimed);
        drop(namespace_claims);

        let rollback_lease = Arc::clone(&lease);
        let artifact_lease = Arc::clone(&lease);
        let result = (|| {
            let manifest =
                open_existing_immutable_ledger(Arc::clone(&lease), ImmutableLedgerNamespace::ProjectionManifest)?;
            let artifacts = open_projection_artifacts(artifact_lease, artifact_directory, false)?;
            Ok(ProjectionStorageBundle {
                manifest,
                artifacts,
                reset_operation_id: None,
            })
        })();
        if result.is_err() {
            let mut namespace_claims = rollback_lease
                .claimed_namespaces
                .lock()
                .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
            namespace_claims.remove(&ImmutableLedgerNamespace::ProjectionManifest);
            let mut artifacts_claimed = rollback_lease
                .projection_artifacts_claimed
                .lock()
                .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
            *artifacts_claimed = false;
        }
        result
    }

    pub(crate) fn prepare_projection_pair_target(
        &self,
        publish_mode: ProjectionPairPublishMode,
    ) -> Result<ProjectionPairTarget, LedgerStorageOpenError> {
        let (lease, _) = self.projection_artifact_parts();
        let _lifecycle = lease
            .projection_lifecycle
            .lock()
            .map_err(|_| std::io::Error::other("projection lifecycle state is poisoned"))?;
        let mut namespace_claims = lease
            .claimed_namespaces
            .lock()
            .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
        let mut artifacts_claimed = lease
            .projection_artifacts_claimed
            .lock()
            .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
        if namespace_claims.contains(&ImmutableLedgerNamespace::ProjectionManifest) || *artifacts_claimed {
            return Err(LedgerStorageOpenError::NamespaceAlreadyClaimed);
        }
        reject_projection_reset_pending(&lease.vault_root)?;
        reject_legacy_projection_layout(&lease.vault_root)?;
        let live_pair = lease.vault_root.join(PROJECTION_STORE_DIRECTORY);
        let live_identity = match std::fs::symlink_metadata(&live_pair) {
            Ok(metadata) => Some(projection_pair_entry_identity(&metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        match (publish_mode, live_identity) {
            (ProjectionPairPublishMode::CreateAbsent, None) | (ProjectionPairPublishMode::ReplaceExisting, Some(_)) => {
            }
            (ProjectionPairPublishMode::CreateAbsent, Some(_)) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "projection storage already exists",
                )
                .into());
            }
            (ProjectionPairPublishMode::ReplaceExisting, None) => {
                return Err(
                    std::io::Error::new(std::io::ErrorKind::NotFound, "projection storage does not exist").into(),
                );
            }
        }
        namespace_claims.insert(ImmutableLedgerNamespace::ProjectionManifest);
        *artifacts_claimed = true;
        drop(artifacts_claimed);
        drop(namespace_claims);

        let result = (|| {
            let container = tempfile::Builder::new()
                .prefix(".plico-projection-pair-staging.")
                .tempdir_in(&lease.vault_root)?;
            restrict_new_staging_directory(container.path())?;
            require_private_directory(container.path())?;
            let pair_path = container.path().join(PROJECTION_STORE_DIRECTORY);
            ensure_directory(&pair_path, container.path())?;
            let storage = open_projection_bundle_at(Arc::clone(&lease), &pair_path, true)?;
            let (container_directory, directory_identity) = open_private_directory_identity(container.path())?;
            let (pair_directory, pair_identity) = open_private_directory_identity(&pair_path)?;
            Ok(ProjectionPairTarget {
                lease: Arc::clone(&lease),
                container: Some(container),
                pair_path,
                container_directory,
                pair_directory,
                directory_identity,
                pair_identity,
                expected_live_identity: live_identity,
                expected_live_fingerprint: None,
                reset_reason: None,
                reset_transaction_id: (publish_mode == ProjectionPairPublishMode::ReplaceExisting)
                    .then(uuid::Uuid::new_v4),
                storage: Some(storage),
                seal: None,
                publish_mode,
                claims_reserved: true,
            })
        })();
        if result.is_err() {
            rollback_projection_claims(&lease)?;
        }
        result
    }

    pub(crate) fn publish_projection_pair_target(
        &self,
        mut target: ProjectionPairTarget,
    ) -> Result<ProjectionPairPublication, LedgerStorageError> {
        let (lease, _) = self.projection_artifact_parts();
        if !Arc::ptr_eq(&lease, &target.lease) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "projection target belongs to another vault lifecycle",
            )
            .into());
        }
        let reset_operation_id = match target.publish_mode {
            ProjectionPairPublishMode::CreateAbsent => None,
            ProjectionPairPublishMode::ReplaceExisting => Some(
                target
                    .reset_transaction_id
                    .as_ref()
                    .ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "projection reset target has no transaction identity",
                        )
                    })?
                    .hyphenated()
                    .to_string(),
            ),
        };
        let reset_span = if target.publish_mode == ProjectionPairPublishMode::ReplaceExisting {
            tracing::debug_span!(
                "projection_reset_operation",
                operation = "projection_reset",
                reset_operation_id = %reset_operation_id
                    .as_deref()
                    .expect("replace target checked its reset transaction identity")
            )
        } else {
            tracing::Span::none()
        };
        let _reset_span = reset_span.enter();
        let _lifecycle = lease
            .projection_lifecycle
            .lock()
            .map_err(|_| std::io::Error::other("projection lifecycle state is poisoned"))?;
        let seal = target
            .seal
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "projection target is unsealed"))?;
        validate_private_directory_identity(
            &target.container_directory,
            target
                .container
                .as_ref()
                .expect("projection target retains its staging container")
                .path(),
            target.directory_identity,
        )?;
        validate_private_directory_identity(&target.pair_directory, &target.pair_path, target.pair_identity)?;
        verify_projection_pair_target(&target, seal)?;
        let live_pair = lease.vault_root.join(PROJECTION_STORE_DIRECTORY);
        let live_identity = match std::fs::symlink_metadata(&live_pair) {
            Ok(metadata) => Some(projection_pair_entry_identity(&metadata)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        match (target.publish_mode, live_identity, target.expected_live_identity) {
            (ProjectionPairPublishMode::CreateAbsent, None, None) => {}
            (ProjectionPairPublishMode::ReplaceExisting, Some(actual), Some(expected)) if actual == expected => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection pair publication precondition changed",
                )
                .into());
            }
        }
        if target.publish_mode == ProjectionPairPublishMode::ReplaceExisting {
            let expected_fingerprint = target.expected_live_fingerprint.as_ref().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "projection reset live guard is unbound",
                )
            })?;
            if projection_reset_live_fingerprint(&live_pair)?.digest != *expected_fingerprint
                || target.reset_reason.is_none()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection reset live guard changed before publication",
                )
                .into());
            }
            trace_projection_reset(
                "inspection",
                "ok",
                "reset_required",
                target
                    .reset_reason
                    .expect("projection reset reason was checked above")
                    .as_str(),
            );
        }

        target
            .container
            .as_mut()
            .expect("projection target retains its staging container")
            .disable_cleanup(true);
        let reset_marker = if target.publish_mode == ProjectionPairPublishMode::ReplaceExisting {
            match persist_projection_reset_marker(&target, seal) {
                Ok(marker) => {
                    trace_projection_reset(
                        "prepared",
                        "ok",
                        "marker_durable",
                        target
                            .reset_reason
                            .expect("projection reset target retains its typed reason")
                            .as_str(),
                    );
                    Some(marker)
                }
                Err(ProjectionResetMarkerWriteError::PrePublish(error)) => {
                    trace_projection_reset(
                        "prepared",
                        "failed",
                        "prepublish_io",
                        target
                            .reset_reason
                            .expect("projection reset target retains its typed reason")
                            .as_str(),
                    );
                    target
                        .container
                        .as_mut()
                        .expect("projection target retains its staging container")
                        .disable_cleanup(false);
                    return Err(LedgerStorageError::Io(error));
                }
                Err(ProjectionResetMarkerWriteError::Pending) => {
                    trace_projection_reset(
                        "prepared",
                        "indeterminate",
                        "marker_pending",
                        target
                            .reset_reason
                            .expect("projection reset target retains its typed reason")
                            .as_str(),
                    );
                    target.claims_reserved = false;
                    return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                        "projection reset marker publication requires owner recovery",
                    )));
                }
            }
        } else {
            None
        };
        #[cfg(test)]
        if reset_marker.is_some() && take_projection_reset_fault(&lease, RESET_FAULT_AFTER_PREPARED_MARKER_PERSIST) {
            target.claims_reserved = false;
            return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                "injected failure after prepared projection reset marker publication",
            )));
        }
        drop(target.storage.take());
        let rename_flags = match target.publish_mode {
            ProjectionPairPublishMode::CreateAbsent => RenameFlags::NOREPLACE,
            ProjectionPairPublishMode::ReplaceExisting => RenameFlags::EXCHANGE,
        };
        if let Err(error) = renameat_with(CWD, &target.pair_path, CWD, &live_pair, rename_flags) {
            let rename_error = std::io::Error::from_raw_os_error(error.raw_os_error());
            if reset_marker.is_some() {
                trace_projection_reset(
                    "pair_exchange",
                    "indeterminate",
                    "exchange_requires_recovery",
                    target
                        .reset_reason
                        .expect("projection reset target retains its typed reason")
                        .as_str(),
                );
                target.claims_reserved = false;
                return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                    "projection reset exchange requires owner recovery",
                )));
            }
            target
                .container
                .as_mut()
                .expect("projection target retains its staging container")
                .disable_cleanup(false);
            return Err(LedgerStorageError::Io(rename_error));
        }
        if reset_marker.is_some() {
            trace_projection_reset(
                "pair_exchange",
                "progress",
                "namespace_exchanged_unverified",
                target
                    .reset_reason
                    .expect("projection reset target retains its typed reason")
                    .as_str(),
            );
        }

        #[cfg(test)]
        if reset_marker.is_some() && take_projection_reset_fault(&lease, RESET_FAULT_AFTER_PAIR_EXCHANGE) {
            target.claims_reserved = false;
            return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                "injected failure after projection pair exchange",
            )));
        }

        let container_path = target
            .container
            .as_ref()
            .expect("projection target retains its staging container")
            .path()
            .to_path_buf();
        let live_parent_sync = sync_directory(&lease.vault_root);
        let staging_parent_sync = sync_directory(&container_path);
        let staging_parent_fd_sync = target.container_directory.sync_all();
        let staging_parent_identity = validate_private_directory_identity(
            &target.container_directory,
            &container_path,
            target.directory_identity,
        );
        let live_tree_verified = projection_pair_tree_digest(&live_pair, Some(&seal.payload.genesis))
            .is_ok_and(|digest| digest == seal.payload.tree_digest);
        let active_verified =
            active_pointer_digest(&live_pair).is_ok_and(|digest| digest == seal.payload.active_pointer_digest);
        let quarantine_is_private = require_private_directory(&container_path).is_ok();
        let live_identity_verified =
            validate_private_directory_identity(&target.pair_directory, &live_pair, target.pair_identity).is_ok();
        if live_parent_sync.is_err()
            || staging_parent_sync.is_err()
            || staging_parent_fd_sync.is_err()
            || staging_parent_identity.is_err()
            || !live_tree_verified
            || !active_verified
            || !quarantine_is_private
            || !live_identity_verified
        {
            if reset_marker.is_some() {
                trace_projection_reset(
                    "pair_exchange",
                    "indeterminate",
                    "durability_unverified",
                    target
                        .reset_reason
                        .expect("projection reset target retains its typed reason")
                        .as_str(),
                );
            }
            target.claims_reserved = false;
            return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                "projection pair exchange could not be durably verified",
            )));
        }
        if reset_marker.is_some() {
            trace_projection_reset(
                "pair_exchange",
                "ok",
                "published",
                target
                    .reset_reason
                    .expect("projection reset target retains its typed reason")
                    .as_str(),
            );
        }
        let storage = match open_projection_bundle_at(Arc::clone(&lease), &live_pair, false) {
            Ok(storage) => storage,
            Err(_) => {
                target.claims_reserved = false;
                return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                    "published projection pair could not be reopened",
                )));
            }
        };

        let applied_marker = if let Some(marker) = reset_marker.as_ref() {
            let container_path = target
                .container
                .as_ref()
                .expect("projection reset target retains its quarantine container")
                .path();
            let topology = ProjectionResetAppliedTopology {
                lease: &target.lease,
                vault_root: &target.lease.vault_root,
                container_path,
                container_identity: target.directory_identity,
                target_pair_identity: target.pair_identity,
                expected_live_identity: target
                    .expected_live_identity
                    .expect("projection reset target retains expected live identity"),
            };
            match persist_applied_projection_reset_marker(&topology, marker) {
                Ok(applied) => {
                    trace_projection_reset(
                        "marker_transition",
                        "ok",
                        "applied_maintenance",
                        target
                            .reset_reason
                            .expect("projection reset target retains its typed reason")
                            .as_str(),
                    );
                    Some(applied)
                }
                Err(_) => {
                    trace_projection_reset(
                        "marker_transition",
                        "indeterminate",
                        "transition_requires_recovery",
                        target
                            .reset_reason
                            .expect("projection reset target retains its typed reason")
                            .as_str(),
                    );
                    target.claims_reserved = false;
                    return Err(LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                        "published projection reset marker could not be advanced to maintenance",
                    )));
                }
            }
        } else {
            None
        };

        // A potentially corrupt exchanged tree is never recursively deleted.
        // It remains under the 0700 quarantine for bounded, fd-relative
        // maintenance.
        target.claims_reserved = false;
        match applied_marker {
            Some(marker) => Ok(ProjectionPairPublication::ResetMaintenance(Box::new({
                let container = target
                    .container
                    .take()
                    .expect("projection reset target retains its quarantine container");
                let container_path = container.keep();
                let (container_directory, container_identity) = open_private_directory_identity(&container_path)
                    .map_err(|_| {
                        LedgerStorageError::PublishedButUnsynced(std::io::Error::other(
                            "projection reset quarantine could not be reopened",
                        ))
                    })?;
                ProjectionResetMaintenance {
                    lease: Arc::clone(&lease),
                    storage: Some(storage),
                    container_path,
                    container_directory: Some(container_directory),
                    container_identity,
                    marker,
                }
            }))),
            None => {
                target
                    .container
                    .as_mut()
                    .expect("projection target retains its staging container")
                    .disable_cleanup(false);
                Ok(ProjectionPairPublication::ReadyStorage(Box::new(storage)))
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_manifest_orphan_for_test(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        validate_hash(hash)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/manifest/objects");
        put_immutable_at(&objects, hash, bytes)?;
        sync_directory(&objects)
    }

    #[cfg(test)]
    pub(crate) fn remove_projection_manifest_object_for_test(&self, hash: &str) -> std::io::Result<()> {
        validate_hash(hash)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/manifest/objects");
        std::fs::remove_file(objects.join(hash))?;
        sync_directory(&objects)
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_artifact_orphan_for_test(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        validate_hash(hash)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/artifacts/objects");
        put_immutable_at(&objects, hash, bytes)?;
        sync_directory(&objects)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_projection_artifact_symlink_orphan_for_test(
        &self,
        hash: &str,
    ) -> std::io::Result<ProjectionExternalCanaryFingerprint> {
        use std::os::unix::fs::symlink;

        validate_hash(hash)?;
        let (lease, _) = self.projection_artifact_parts();
        let objects = lease.vault_root.join("projection-store/artifacts/objects");
        let parent = lease
            .vault_root
            .parent()
            .ok_or_else(|| std::io::Error::other("vault has no parent for cleanup canary"))?;
        let canary_path = parent.join("projection-cleanup-external-canary");
        let canary_bytes = b"projection-cleanup-external-canary-v1".to_vec();
        let mut canary = create_private_file(&canary_path)?;
        canary.write_all(&canary_bytes)?;
        canary.sync_all()?;
        sync_directory(parent)?;
        let identity = artifact_identity(&canary.metadata()?);
        symlink(&canary_path, objects.join(hash))?;
        sync_directory(&objects)?;
        Ok(ProjectionExternalCanaryFingerprint {
            path: canary_path,
            identity,
            bytes: canary_bytes,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn assert_projection_external_canary_unchanged_for_test(
        &self,
        canary: &ProjectionExternalCanaryFingerprint,
    ) -> std::io::Result<()> {
        let metadata = std::fs::symlink_metadata(&canary.path)?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_file()
            || artifact_identity(&metadata) != canary.identity
            || read_private_file_bounded(&canary.path, 4096)? != canary.bytes
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection cleanup external canary changed",
            ));
        }
        Ok(())
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_projection_artifact_permissive_orphan_for_test(
        &self,
        hash: &str,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        self.inject_projection_artifact_orphan_for_test(hash, bytes)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/artifacts/objects");
        std::fs::set_permissions(objects.join(hash), std::fs::Permissions::from_mode(0o644))?;
        sync_directory(&objects)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_projection_artifact_fifo_orphan_for_test(&self, hash: &str) -> std::io::Result<()> {
        validate_hash(hash)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/artifacts/objects");
        rustix::fs::mkfifoat(rustix::fs::CWD, objects.join(hash), Mode::RUSR | Mode::WUSR)
            .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
        sync_directory(&objects)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_projection_artifact_sparse_orphan_for_test(
        &self,
        hash: &str,
        logical_size: u64,
    ) -> std::io::Result<()> {
        validate_hash(hash)?;
        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/artifacts/objects");
        let file = create_private_file(&objects.join(hash))?;
        file.set_len(logical_size)?;
        file.sync_all()?;
        sync_directory(&objects)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn set_projection_artifact_objects_mode_for_test(&self, mode: u32) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let objects = self
            .projection_artifact_parts()
            .0
            .vault_root
            .join("projection-store/artifacts/objects");
        std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(mode))?;
        let artifacts = objects
            .parent()
            .ok_or_else(|| std::io::Error::other("projection artifacts objects directory has no parent"))?;
        sync_directory(artifacts)
    }

    #[cfg(test)]
    pub(crate) fn projection_quarantine_count_for_test(&self) -> std::io::Result<usize> {
        let (lease, _) = self.projection_artifact_parts();
        let mut count = 0usize;
        for entry in std::fs::read_dir(&lease.vault_root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 vault entry"))?;
            if name.starts_with(".plico-projection-pair-staging.") {
                require_private_directory(&entry.path())?;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| std::io::Error::other("projection quarantine count overflow"))?;
            }
        }
        Ok(count)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn set_projection_quarantine_artifact_objects_mode_for_test(&self, mode: u32) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let (lease, _) = self.projection_artifact_parts();
        let mut quarantine = None;
        for entry in std::fs::read_dir(&lease.vault_root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 vault entry"))?;
            if name.starts_with(".plico-projection-pair-staging.") {
                require_private_directory(&entry.path())?;
                if quarantine.replace(entry.path()).is_some() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "multiple projection quarantines",
                    ));
                }
            }
        }
        let quarantine = quarantine
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection quarantine is absent"))?;
        let objects = quarantine.join("projection-store/artifacts/objects");
        std::fs::set_permissions(&objects, std::fs::Permissions::from_mode(mode))?;
        sync_directory(
            objects
                .parent()
                .ok_or_else(|| std::io::Error::other("quarantine artifact objects has no parent"))?,
        )
    }

    #[cfg(test)]
    pub(crate) fn remove_projection_reset_quarantine_seal_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let mut quarantine = None;
        for entry in std::fs::read_dir(&lease.vault_root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 vault entry"))?;
            if name.starts_with(".plico-projection-pair-staging.") {
                require_private_directory(&entry.path())?;
                if quarantine.replace(entry.path()).is_some() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "multiple projection quarantines",
                    ));
                }
            }
        }
        let quarantine = quarantine
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection quarantine is absent"))?;
        std::fs::remove_file(quarantine.join(PROJECTION_PAIR_SEAL_FILE))?;
        sync_directory(&quarantine)
    }

    #[cfg(all(test, target_os = "linux"))]
    pub(crate) fn tamper_projection_reset_marker_cross_field_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let path = lease.vault_root.join(PROJECTION_RESET_MARKER_FILE);
        let bytes = read_private_file_bounded(&path, 4096)?;
        let mut marker: ProjectionResetMarker = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let ProjectionResetMarkerPhase::AppliedMaintenance { new_live_identity, .. } = &mut marker.payload.phase else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset marker is not applied",
            ));
        };
        let inode = new_live_identity
            .inode
            .parse::<u64>()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        new_live_identity.inode = inode
            .checked_add(1)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "test inode overflow"))?
            .to_string();
        let payload_bytes = serde_json_canonicalizer::to_vec(&marker.payload)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        marker.marker_hash = domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes);
        let bytes = serde_json_canonicalizer::to_vec(&marker)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let mut temporary = tempfile::NamedTempFile::new_in(&lease.vault_root)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&path).map_err(|error| error.error)?;
        sync_directory(&lease.vault_root)
    }

    #[cfg(test)]
    pub(crate) fn tamper_projection_reset_active_marker_future_schema_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let path = lease.vault_root.join(PROJECTION_RESET_MARKER_FILE);
        let bytes = read_private_file_bounded(&path, 4096)?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["payload"]["schema"] = serde_json::Value::String("plico.projection.reset-marker/v2".to_string());
        let payload_bytes = serde_json_canonicalizer::to_vec(&value["payload"])
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["marker_hash"] = serde_json::Value::String(domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes));
        let bytes = serde_json_canonicalizer::to_vec(&value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        replace_private_test_file(&path, &bytes)?;
        sync_directory(&lease.vault_root)
    }

    #[cfg(test)]
    pub(crate) fn tamper_projection_reset_seal_future_schema_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let marker = read_projection_reset_marker(&lease.vault_root)
            .map_err(|_| std::io::Error::other("projection reset marker is unreadable"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection reset marker is absent"))?;
        let container = lease
            .vault_root
            .join(&marker_binding(&marker).staging_container_basename);
        let path = container.join(PROJECTION_PAIR_SEAL_FILE);
        let bytes = read_private_file_bounded(&path, 4096)?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["payload"]["schema"] = serde_json::Value::String("plico.projection.pair-seal/v2".to_string());
        let payload_bytes = serde_json_canonicalizer::to_vec(&value["payload"])
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["seal_hash"] = serde_json::Value::String(domain_hash(PROJECTION_PAIR_SEAL_DOMAIN, &payload_bytes));
        let bytes = serde_json_canonicalizer::to_vec(&value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        replace_private_test_file(&path, &bytes)?;
        sync_directory(&container)
    }

    #[cfg(test)]
    pub(crate) fn tamper_projection_reset_transition_future_schema_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let marker = read_projection_reset_marker(&lease.vault_root)
            .map_err(|_| std::io::Error::other("projection reset marker is unreadable"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection reset marker is absent"))?;
        let path = lease
            .vault_root
            .join(&marker_binding(&marker).transition_evidence_basename);
        let bytes = read_private_file_bounded(&path, 4096)?;
        let mut value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["payload"]["schema"] = serde_json::Value::String("plico.projection.reset-marker/v2".to_string());
        let payload_bytes = serde_json_canonicalizer::to_vec(&value["payload"])
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        value["marker_hash"] = serde_json::Value::String(domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes));
        let bytes = serde_json_canonicalizer::to_vec(&value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        replace_private_test_file(&path, &bytes)?;
        sync_directory(&lease.vault_root)
    }

    #[cfg(test)]
    pub(crate) fn resolve_projection_reset_transition_for_test(&self) -> std::io::Result<()> {
        let (lease, _) = self.projection_artifact_parts();
        let marker = read_projection_reset_marker(&lease.vault_root)
            .map_err(|_| std::io::Error::other("projection reset marker is unreadable"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection reset marker is absent"))?;
        resolve_applied_transition_evidence(&lease.vault_root, &marker)
            .map(|_| ())
            .map_err(|_| std::io::Error::other("projection reset transition evidence could not be resolved"))
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_pair_exchange_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_PAIR_EXCHANGE);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_transition_persist_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_TRANSITION_PERSIST);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_marker_exchange_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_MARKER_EXCHANGE);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_quarantine_pair_removal_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_QUARANTINE_PAIR_REMOVAL);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_seal_removal_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_SEAL_REMOVAL);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_container_removal_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_CONTAINER_REMOVAL);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_prepared_marker_persist_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_PREPARED_MARKER_PERSIST);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_after_active_marker_unlink_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_AFTER_ACTIVE_MARKER_UNLINK);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_preflight_permission_denied_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_PREFLIGHT_PERMISSION_DENIED);
    }

    #[cfg(test)]
    pub(crate) fn inject_projection_reset_quarantine_pre_mutation_io_once(&self) {
        self.set_projection_reset_fault_once(RESET_FAULT_QUARANTINE_PRE_MUTATION_IO);
    }

    #[cfg(test)]
    fn set_projection_reset_fault_once(&self, fault: u8) {
        self.projection_artifact_parts()
            .0
            .projection_reset_fault_once
            .store(fault, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
fn take_projection_reset_fault(lease: &PersonalVaultLease, fault: u8) -> bool {
    lease
        .projection_reset_fault_once
        .compare_exchange(
            fault,
            0,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_ok()
}

impl ProjectionPairTarget {
    pub(crate) fn storage(&self) -> &ProjectionStorageBundle {
        self.storage
            .as_ref()
            .expect("unpublished projection target retains its storage")
    }

    pub(crate) fn inspect_and_bind_reset_live<R>(
        &mut self,
        inspect: impl for<'a> FnOnce(crate::cas::ExistingProjectionReadOnly<'a>) -> (Option<ProjectionPairResetReason>, R),
    ) -> Result<ProjectionClaimedLiveInspection<R>, LedgerStorageOpenError> {
        if self.publish_mode != ProjectionPairPublishMode::ReplaceExisting {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "projection live reset guard requires replace mode",
            )
            .into());
        }
        let live = self.lease.vault_root.join(PROJECTION_STORE_DIRECTORY);
        let metadata = std::fs::symlink_metadata(&live)?;
        if Some(projection_pair_entry_identity(&metadata)) != self.expected_live_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection live identity changed before claimed inspection",
            )
            .into());
        }
        let fingerprint = projection_reset_live_fingerprint(&live)?;
        self.expected_live_fingerprint = Some(fingerprint.digest);
        match inspect_projection_layout_at(&self.lease, &live)? {
            ExistingProjectionLayout::Invalid => {
                self.reset_reason = Some(ProjectionPairResetReason::StorageLayoutInvalid);
                Ok(ProjectionClaimedLiveInspection::StorageLayoutInvalid)
            }
            ExistingProjectionLayout::Readable(reader) => {
                let (reason, result) = inspect(reader);
                if fingerprint.storage_layout_invalid {
                    self.reset_reason = Some(ProjectionPairResetReason::StorageLayoutInvalid);
                    Ok(ProjectionClaimedLiveInspection::StorageLayoutInvalid)
                } else {
                    self.reset_reason = reason;
                    Ok(ProjectionClaimedLiveInspection::Readable(result))
                }
            }
        }
    }

    pub(crate) fn seal_clean_genesis(
        &mut self,
        genesis: ProjectionPairGenesisEvidence,
    ) -> Result<(), LedgerStorageError> {
        genesis.validate()?;
        let storage = self.storage();
        storage.flush_manifest()?;
        storage.flush_artifacts()?;
        let artifact_count = u64::try_from(storage.artifacts.list_immutable_hashes(8 * 1024 * 1024)?.len())
            .map_err(|_| std::io::Error::other("projection artifact count overflow"))?;
        if artifact_count != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "clean projection genesis contains artifacts",
            )
            .into());
        }
        let payload = ProjectionPairSealPayload {
            schema: PROJECTION_PAIR_SEAL_SCHEMA.to_string(),
            tree_digest: projection_pair_tree_digest(&self.pair_path, Some(&genesis))?,
            active_pointer_digest: active_pointer_digest(&self.pair_path)?,
            artifact_count,
            genesis,
        };
        let payload_bytes = serde_json_canonicalizer::to_vec(&payload)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let seal = ProjectionPairSeal {
            seal_hash: domain_hash(PROJECTION_PAIR_SEAL_DOMAIN, &payload_bytes),
            payload,
        };
        let bytes = serde_json_canonicalizer::to_vec(&seal)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let seal_path = self
            .container
            .as_ref()
            .expect("projection target retains its staging container")
            .path()
            .join(PROJECTION_PAIR_SEAL_FILE);
        let mut file = create_private_file(&seal_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        sync_directory(
            self.container
                .as_ref()
                .expect("projection target retains its staging container")
                .path(),
        )?;
        self.seal = Some(seal);
        Ok(())
    }
}

impl ProjectionPairGenesisEvidence {
    pub(crate) fn new(
        projection_root_hash: String,
        current_view_hash: String,
        canonical_genesis_root_hash: String,
        canonical_generation: u64,
        canonical_revision_watermark: u64,
        canonical_policy_watermark: u64,
        canonical_relation_watermark: u64,
    ) -> Result<Self, LedgerStorageError> {
        let evidence = Self {
            projection_root_hash,
            current_view_hash,
            canonical_genesis_root_hash,
            canonical_generation,
            canonical_revision_watermark,
            canonical_policy_watermark,
            canonical_relation_watermark,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    fn validate(&self) -> Result<(), LedgerStorageError> {
        for hash in [
            &self.projection_root_hash,
            &self.current_view_hash,
            &self.canonical_genesis_root_hash,
        ] {
            validate_hash(hash)?;
        }
        if [
            self.canonical_generation,
            self.canonical_revision_watermark,
            self.canonical_policy_watermark,
            self.canonical_relation_watermark,
        ]
        .into_iter()
        .any(|value| value > MAX_JCS_SAFE_INTEGER)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair genesis evidence exceeds JCS safe integer range",
            )
            .into());
        }
        if self.canonical_generation != 0
            || self.canonical_revision_watermark != 0
            || self.canonical_policy_watermark != 0
            || self.canonical_relation_watermark != 0
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair target is not bound to canonical genesis",
            )
            .into());
        }
        Ok(())
    }
}

impl Drop for ProjectionPairTarget {
    fn drop(&mut self) {
        if self.claims_reserved {
            let _ = rollback_projection_claims(&self.lease);
        }
    }
}

fn projection_reset_pre_mutation_error(error: std::io::Error) -> ProjectionResetMaintenanceError {
    match error.kind() {
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound => {
            ProjectionResetMaintenanceError::ManualIntervention
        }
        std::io::ErrorKind::PermissionDenied if error.raw_os_error().is_none() => {
            ProjectionResetMaintenanceError::ManualIntervention
        }
        _ => ProjectionResetMaintenanceError::Unavailable,
    }
}

fn projection_reset_recovery_pre_mutation_error(error: std::io::Error) -> ProjectionResetRecoveryError {
    match error.kind() {
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound => ProjectionResetRecoveryError::Manual,
        std::io::ErrorKind::PermissionDenied if error.raw_os_error().is_none() => ProjectionResetRecoveryError::Manual,
        _ => ProjectionResetRecoveryError::Unavailable,
    }
}

fn projection_reset_recovery_after_boundary_error(
    error: std::io::Error,
    mutated: bool,
) -> ProjectionResetRecoveryError {
    if mutated {
        ProjectionResetRecoveryError::Indeterminate
    } else {
        projection_reset_recovery_pre_mutation_error(error)
    }
}

fn projection_reset_maintenance_after_boundary(
    error: ProjectionResetEvidenceMutationError,
    mutated: bool,
) -> ProjectionResetMaintenanceError {
    if mutated || matches!(error, ProjectionResetEvidenceMutationError::Indeterminate) {
        return ProjectionResetMaintenanceError::CommitIndeterminate;
    }
    match error {
        ProjectionResetEvidenceMutationError::Manual => ProjectionResetMaintenanceError::ManualIntervention,
        ProjectionResetEvidenceMutationError::UnsupportedFormat => ProjectionResetMaintenanceError::UnsupportedFormat,
        ProjectionResetEvidenceMutationError::Unavailable => ProjectionResetMaintenanceError::Unavailable,
        ProjectionResetEvidenceMutationError::Indeterminate => ProjectionResetMaintenanceError::CommitIndeterminate,
    }
}

fn projection_reset_io_after_boundary(error: std::io::Error, mutated: bool) -> ProjectionResetMaintenanceError {
    if mutated {
        ProjectionResetMaintenanceError::CommitIndeterminate
    } else {
        projection_reset_pre_mutation_error(error)
    }
}

fn trace_projection_reset_maintenance_error(
    phase: &'static str,
    error: &ProjectionResetMaintenanceError,
    reset_reason: &'static str,
) {
    let (outcome, result_category) = match error {
        ProjectionResetMaintenanceError::ManualIntervention => ("manual", "evidence_invalid"),
        ProjectionResetMaintenanceError::CommitIndeterminate => ("indeterminate", "mutation_unverified"),
        ProjectionResetMaintenanceError::UnsupportedFormat => ("unsupported", "upgrade_required"),
        ProjectionResetMaintenanceError::Unavailable => ("unavailable", "storage_io"),
    };
    trace_projection_reset(phase, outcome, result_category, reset_reason);
}

impl ProjectionResetMaintenance {
    pub(crate) fn finish(mut self) -> Result<ProjectionStorageBundle, ProjectionResetMaintenanceError> {
        let result = self.finish_inner();
        if matches!(
            &result,
            Err(ProjectionResetMaintenanceError::ManualIntervention
                | ProjectionResetMaintenanceError::UnsupportedFormat
                | ProjectionResetMaintenanceError::Unavailable)
        ) {
            rollback_projection_claims(&self.lease).map_err(|_| ProjectionResetMaintenanceError::Unavailable)?;
        }
        result
    }

    fn finish_inner(&mut self) -> Result<ProjectionStorageBundle, ProjectionResetMaintenanceError> {
        validate_projection_reset_marker(&self.lease.vault_root, &self.marker)
            .map_err(projection_reset_pre_mutation_error)?;
        preflight_projection_reset_protocol(&self.lease.vault_root, &self.marker).map_err(|error| match error {
            LedgerStorageOpenError::UnsupportedProjectionFormat => ProjectionResetMaintenanceError::UnsupportedFormat,
            LedgerStorageOpenError::Io(_) => ProjectionResetMaintenanceError::Unavailable,
            _ => ProjectionResetMaintenanceError::ManualIntervention,
        })?;
        let reset_span = tracing::debug_span!(
            "projection_reset_operation",
            operation = "projection_reset",
            reset_operation_id = %self.marker.payload.transaction_id
        );
        let _reset_span = reset_span.enter();
        let reset_reason = projection_reset_reason_trace_value(&marker_binding(&self.marker).reset_reason);
        trace_projection_reset("recovery", "started", "applied_maintenance", reset_reason);
        if !matches!(
            self.marker.payload.phase,
            ProjectionResetMarkerPhase::AppliedMaintenance { .. }
        ) {
            trace_projection_reset("recovery", "manual", "phase_invalid", reset_reason);
            return Err(ProjectionResetMaintenanceError::ManualIntervention);
        }
        let (mut seal, pair_exists) = if let Some(container_directory) = self.container_directory.as_ref() {
            validate_private_directory_identity(container_directory, &self.container_path, self.container_identity)
                .map_err(projection_reset_pre_mutation_error)?;
            container_directory
                .sync_all()
                .map_err(projection_reset_pre_mutation_error)?;
            let seal = read_optional_projection_reset_seal(container_directory).map_err(|error| match error {
                ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetMaintenanceError::UnsupportedFormat,
                ProjectionResetProtocolError::Invalid => ProjectionResetMaintenanceError::ManualIntervention,
                ProjectionResetProtocolError::Io(_) => ProjectionResetMaintenanceError::Unavailable,
            })?;
            let pair_exists = projection_reset_container_entry_exists(container_directory, PROJECTION_STORE_DIRECTORY)
                .map_err(projection_reset_pre_mutation_error)?;
            if pair_exists && seal.is_none() {
                trace_projection_reset("quarantine_cleanup", "manual", "pair_without_seal", reset_reason);
                return Err(ProjectionResetMaintenanceError::ManualIntervention);
            }
            verify_projection_reset_live(
                &self.lease.vault_root,
                &self.marker,
                seal.as_ref().map(|evidence| &evidence.seal),
            )
            .map_err(projection_reset_pre_mutation_error)?;
            (seal, pair_exists)
        } else {
            verify_projection_reset_live(&self.lease.vault_root, &self.marker, None)
                .map_err(projection_reset_pre_mutation_error)?;
            (None, false)
        };
        if let (Some(container_directory), Some(seal_evidence)) = (self.container_directory.as_ref(), seal.as_mut()) {
            validate_projection_reset_seal_evidence(container_directory, seal_evidence).map_err(
                |error| match error {
                    ProjectionResetProtocolError::UnsupportedFormat => {
                        ProjectionResetMaintenanceError::UnsupportedFormat
                    }
                    ProjectionResetProtocolError::Invalid => ProjectionResetMaintenanceError::ManualIntervention,
                    ProjectionResetProtocolError::Io(_) => ProjectionResetMaintenanceError::Unavailable,
                },
            )?;
        }
        if pair_exists {
            validate_projection_reset_quarantine_cleanup(
                self.container_directory
                    .as_ref()
                    .ok_or(ProjectionResetMaintenanceError::ManualIntervention)?,
                &self.marker,
            )
            .map_err(projection_reset_pre_mutation_error)?;
        }
        let mut mutated_this_attempt = match resolve_applied_transition_evidence(&self.lease.vault_root, &self.marker) {
            Ok(mutated) => mutated,
            Err(ProjectionResetEvidenceMutationError::Manual) => {
                trace_projection_reset("marker_transition", "manual", "evidence_invalid", reset_reason);
                return Err(ProjectionResetMaintenanceError::ManualIntervention);
            }
            Err(ProjectionResetEvidenceMutationError::Indeterminate) => {
                trace_projection_reset(
                    "marker_transition",
                    "indeterminate",
                    "evidence_cleanup_unverified",
                    reset_reason,
                );
                return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
            }
            Err(ProjectionResetEvidenceMutationError::UnsupportedFormat) => {
                trace_projection_reset("marker_transition", "unsupported", "upgrade_required", reset_reason);
                return Err(ProjectionResetMaintenanceError::UnsupportedFormat);
            }
            Err(ProjectionResetEvidenceMutationError::Unavailable) => {
                trace_projection_reset("marker_transition", "unavailable", "storage_io", reset_reason);
                return Err(ProjectionResetMaintenanceError::Unavailable);
            }
        };
        if mutated_this_attempt {
            trace_projection_reset("marker_transition", "ok", "transition_evidence_removed", reset_reason);
        }
        verify_projection_reset_live(
            &self.lease.vault_root,
            &self.marker,
            seal.as_ref().map(|evidence| &evidence.seal),
        )
        .map_err(|error| projection_reset_io_after_boundary(error, mutated_this_attempt))?;
        if let Some(container_directory) = self.container_directory.as_ref() {
            if pair_exists {
                let cleanup = {
                    #[cfg(test)]
                    if take_projection_reset_fault(&self.lease, RESET_FAULT_QUARANTINE_PRE_MUTATION_IO) {
                        Err(ProjectionResetEvidenceMutationError::Unavailable)
                    } else {
                        cleanup_projection_reset_quarantine(container_directory, &self.marker)
                    }
                    #[cfg(not(test))]
                    cleanup_projection_reset_quarantine(container_directory, &self.marker)
                };
                match cleanup {
                    Ok(()) => trace_projection_reset("quarantine_cleanup", "ok", "pair_removed", reset_reason),
                    Err(error @ ProjectionResetEvidenceMutationError::Manual) => {
                        trace_projection_reset("quarantine_cleanup", "manual", "evidence_invalid", reset_reason);
                        return Err(projection_reset_maintenance_after_boundary(error, mutated_this_attempt));
                    }
                    Err(ProjectionResetEvidenceMutationError::Indeterminate) => {
                        trace_projection_reset(
                            "quarantine_cleanup",
                            "indeterminate",
                            "mutation_unverified",
                            reset_reason,
                        );
                        return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
                    }
                    Err(error @ ProjectionResetEvidenceMutationError::UnsupportedFormat) => {
                        let error = projection_reset_maintenance_after_boundary(error, mutated_this_attempt);
                        trace_projection_reset_maintenance_error("quarantine_cleanup", &error, reset_reason);
                        return Err(error);
                    }
                    Err(error @ ProjectionResetEvidenceMutationError::Unavailable) => {
                        let error = projection_reset_maintenance_after_boundary(error, mutated_this_attempt);
                        trace_projection_reset_maintenance_error("quarantine_cleanup", &error, reset_reason);
                        return Err(error);
                    }
                }
                mutated_this_attempt = true;
                #[cfg(test)]
                if take_projection_reset_fault(&self.lease, RESET_FAULT_AFTER_QUARANTINE_PAIR_REMOVAL) {
                    return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
                }
            } else {
                container_directory
                    .sync_all()
                    .map_err(|error| projection_reset_io_after_boundary(error, mutated_this_attempt))?;
            }
            verify_projection_reset_live(
                &self.lease.vault_root,
                &self.marker,
                seal.as_ref().map(|evidence| &evidence.seal),
            )
            .map_err(|error| projection_reset_io_after_boundary(error, mutated_this_attempt))?;
            if let Some(mut seal_evidence) = seal {
                validate_projection_reset_seal_evidence(container_directory, &mut seal_evidence).map_err(|error| {
                    let error = match error {
                        ProjectionResetProtocolError::UnsupportedFormat => {
                            ProjectionResetEvidenceMutationError::UnsupportedFormat
                        }
                        ProjectionResetProtocolError::Invalid => ProjectionResetEvidenceMutationError::Manual,
                        ProjectionResetProtocolError::Io(_) => ProjectionResetEvidenceMutationError::Unavailable,
                    };
                    projection_reset_maintenance_after_boundary(error, mutated_this_attempt)
                })?;
                remove_projection_reset_seal(container_directory, &seal_evidence.stat).map_err(|error| {
                    let error = projection_reset_maintenance_after_boundary(error, mutated_this_attempt);
                    trace_projection_reset_maintenance_error("seal_cleanup", &error, reset_reason);
                    error
                })?;
                mutated_this_attempt = true;
                trace_projection_reset("seal_cleanup", "ok", "seal_removed", reset_reason);
                #[cfg(test)]
                if take_projection_reset_fault(&self.lease, RESET_FAULT_AFTER_SEAL_REMOVAL) {
                    return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
                }
            } else {
                container_directory
                    .sync_all()
                    .map_err(|error| projection_reset_io_after_boundary(error, mutated_this_attempt))?;
            }
            self.container_directory.take();
            let basename = marker_staging_basename(&self.marker).ok_or(if mutated_this_attempt {
                ProjectionResetMaintenanceError::CommitIndeterminate
            } else {
                ProjectionResetMaintenanceError::ManualIntervention
            })?;
            remove_projection_reset_container(&self.lease.vault_root, basename, self.container_identity).map_err(
                |error| {
                    let error = projection_reset_maintenance_after_boundary(error, mutated_this_attempt);
                    trace_projection_reset_maintenance_error("container_cleanup", &error, reset_reason);
                    error
                },
            )?;
            mutated_this_attempt = true;
            trace_projection_reset("container_cleanup", "ok", "container_removed", reset_reason);
            #[cfg(test)]
            if take_projection_reset_fault(&self.lease, RESET_FAULT_AFTER_CONTAINER_REMOVAL) {
                return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
            }
        } else {
            sync_directory(&self.lease.vault_root)
                .map_err(|error| projection_reset_io_after_boundary(error, mutated_this_attempt))?;
        }
        verify_projection_reset_live(&self.lease.vault_root, &self.marker, None).map_err(|error| {
            trace_projection_reset("marker_clear", "indeterminate", "live_invalid", reset_reason);
            projection_reset_io_after_boundary(error, mutated_this_attempt)
        })?;
        match clear_projection_reset_marker(&self.lease, &self.marker) {
            Ok(()) => trace_projection_reset("marker_clear", "ok", "marker_removed", reset_reason),
            Err(error @ ProjectionResetEvidenceMutationError::Manual) => {
                trace_projection_reset("marker_clear", "manual", "marker_invalid", reset_reason);
                return Err(projection_reset_maintenance_after_boundary(error, mutated_this_attempt));
            }
            Err(ProjectionResetEvidenceMutationError::Indeterminate) => {
                trace_projection_reset("marker_clear", "indeterminate", "mutation_unverified", reset_reason);
                return Err(ProjectionResetMaintenanceError::CommitIndeterminate);
            }
            Err(error @ ProjectionResetEvidenceMutationError::UnsupportedFormat) => {
                return Err(projection_reset_maintenance_after_boundary(error, mutated_this_attempt));
            }
            Err(error @ ProjectionResetEvidenceMutationError::Unavailable) => {
                return Err(projection_reset_maintenance_after_boundary(error, mutated_this_attempt));
            }
        }
        let mut storage = self
            .storage
            .take()
            .ok_or(ProjectionResetMaintenanceError::CommitIndeterminate)?;
        storage.reset_operation_id = Some(self.marker.payload.transaction_id.clone());
        Ok(storage)
    }
}

#[cfg(target_os = "linux")]
fn validate_projection_reset_quarantine_cleanup(
    container: &File,
    marker: &ProjectionResetMarker,
) -> std::io::Result<()> {
    let ProjectionResetMarkerPhase::AppliedMaintenance {
        quarantined_entry_identity,
        ..
    } = &marker.payload.phase
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker is not in maintenance phase",
        ));
    };
    let root = statat(container, PROJECTION_STORE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
    if !reset_marker_cleanup_identity_matches_stat(quarantined_entry_identity, &root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine identity mismatch",
        ));
    }
    if FileType::from_raw_mode(root.st_mode) != FileType::Directory {
        return Ok(());
    }
    if root.st_mode & 0o777 == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine directory cannot be opened safely",
        ));
    }
    let directory = openat2(
        container,
        PROJECTION_STORE_DIRECTORY,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOATIME,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )
    .map_err(rustix_io)?;
    let opened = fstat(&directory).map_err(rustix_io)?;
    if !same_reset_directory_identity(&root, &opened) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine changed while opening",
        ));
    }
    let mut visited = 0usize;
    validate_projection_reset_quarantine_directory(directory.as_fd(), opened.st_dev as u64, 0, &mut visited)
}

#[cfg(not(target_os = "linux"))]
fn validate_projection_reset_quarantine_cleanup(
    _container: &File,
    _marker: &ProjectionResetMarker,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset maintenance requires Linux openat2",
    ))
}

#[cfg(target_os = "linux")]
fn validate_projection_reset_quarantine_directory(
    directory: rustix::fd::BorrowedFd<'_>,
    root_device: u64,
    depth: usize,
    visited: &mut usize,
) -> std::io::Result<()> {
    if depth > 8 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine exceeds depth limit",
        ));
    }
    let mut reader = Dir::read_from(directory).map_err(rustix_io)?;
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(rustix_io)?;
        if matches!(entry.file_name().to_bytes(), b"." | b"..") {
            continue;
        }
        *visited = visited.checked_add(1).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "projection reset inventory overflow")
        })?;
        if *visited > MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine exceeds inventory limit",
            ));
        }
        let name = entry.file_name();
        let observed = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
        if observed.st_dev as u64 != root_device {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine crosses a filesystem boundary",
            ));
        }
        if FileType::from_raw_mode(observed.st_mode) != FileType::Directory {
            continue;
        }
        if observed.st_mode & 0o777 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine directory cannot be opened safely",
            ));
        }
        let child = openat2(
            directory,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOATIME,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(rustix_io)?;
        let opened = fstat(&child).map_err(rustix_io)?;
        if !same_reset_directory_identity(&observed, &opened) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine changed while opening",
            ));
        }
        validate_projection_reset_quarantine_directory(child.as_fd(), root_device, depth + 1, visited)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn cleanup_projection_reset_quarantine(
    container: &File,
    marker: &ProjectionResetMarker,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    let mut mutated = false;
    cleanup_projection_reset_quarantine_inner(container, marker, &mut mutated).map_err(|error| {
        if mutated {
            ProjectionResetEvidenceMutationError::Indeterminate
        } else if matches!(
            error.kind(),
            std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound
        ) {
            ProjectionResetEvidenceMutationError::Manual
        } else {
            ProjectionResetEvidenceMutationError::Unavailable
        }
    })
}

#[cfg(target_os = "linux")]
fn cleanup_projection_reset_quarantine_inner(
    container: &File,
    marker: &ProjectionResetMarker,
    mutated: &mut bool,
) -> std::io::Result<()> {
    let ProjectionResetMarkerPhase::AppliedMaintenance {
        quarantined_entry_identity,
        ..
    } = &marker.payload.phase
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker is not in maintenance phase",
        ));
    };
    let mut root_stat = statat(container, PROJECTION_STORE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
    if !reset_marker_cleanup_identity_matches_stat(quarantined_entry_identity, &root_stat) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine identity mismatch",
        ));
    }
    if FileType::from_raw_mode(root_stat.st_mode) != FileType::Directory {
        unlinkat(container, PROJECTION_STORE_DIRECTORY, AtFlags::empty()).map_err(rustix_io)?;
        *mutated = true;
        return fsync(container).map_err(rustix_io);
    }
    if root_stat.st_mode & 0o777 == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine directory cannot be opened safely",
        ));
    }
    let quarantine = openat2(
        container,
        PROJECTION_STORE_DIRECTORY,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )
    .map_err(rustix_io)?;
    let mut opened_root = fstat(&quarantine).map_err(rustix_io)?;
    if !same_reset_directory_identity(&root_stat, &opened_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine changed while opening",
        ));
    }
    if opened_root.st_mode & 0o777 != 0o700 {
        fchmod(&quarantine, Mode::from_raw_mode(0o700)).map_err(rustix_io)?;
        *mutated = true;
        fsync(&quarantine).map_err(rustix_io)?;
        let restricted = fstat(&quarantine).map_err(rustix_io)?;
        if !same_reset_directory_identity(&opened_root, &restricted) || restricted.st_mode & 0o777 != 0o700 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine changed while restricting permissions",
            ));
        }
        opened_root = restricted;
        root_stat = statat(container, PROJECTION_STORE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
        if !same_reset_directory_identity(&opened_root, &root_stat) || root_stat.st_mode & 0o777 != 0o700 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine path changed after restriction",
            ));
        }
        fsync(container).map_err(rustix_io)?;
    }
    let mut visited = 0usize;
    cleanup_projection_reset_directory(quarantine.as_fd(), opened_root.st_dev as u64, 0, &mut visited, mutated)?;
    fsync(&quarantine).map_err(rustix_io)?;
    let current = statat(container, PROJECTION_STORE_DIRECTORY, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
    if !same_reset_directory_identity(&opened_root, &current) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine changed before removal",
        ));
    }
    unlinkat(container, PROJECTION_STORE_DIRECTORY, AtFlags::REMOVEDIR).map_err(rustix_io)?;
    *mutated = true;
    fsync(container).map_err(rustix_io)
}

#[cfg(not(target_os = "linux"))]
fn cleanup_projection_reset_quarantine(
    _container: &File,
    _marker: &ProjectionResetMarker,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    Err(ProjectionResetEvidenceMutationError::Manual)
}

#[cfg(target_os = "linux")]
fn cleanup_projection_reset_directory(
    directory: rustix::fd::BorrowedFd<'_>,
    root_device: u64,
    depth: usize,
    visited: &mut usize,
    mutated: &mut bool,
) -> std::io::Result<()> {
    if depth > 8 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset quarantine exceeds depth limit",
        ));
    }
    let mut reader = Dir::read_from(directory).map_err(rustix_io)?;
    let mut names = Vec::new();
    while let Some(entry) = reader.read() {
        let entry = entry.map_err(rustix_io)?;
        if matches!(entry.file_name().to_bytes(), b"." | b"..") {
            continue;
        }
        *visited = visited.checked_add(1).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "projection reset inventory overflow")
        })?;
        if *visited > MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine exceeds inventory limit",
            ));
        }
        names.push(entry.file_name().to_owned());
    }
    names.sort_by(|left, right| left.to_bytes().cmp(right.to_bytes()));
    for name in names {
        let observed = statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
        if observed.st_dev as u64 != root_device {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset quarantine crosses a filesystem boundary",
            ));
        }
        if FileType::from_raw_mode(observed.st_mode) == FileType::Directory {
            if observed.st_mode & 0o777 == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection reset quarantine directory cannot be opened safely",
                ));
            }
            let child = openat2(
                directory,
                &name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
            )
            .map_err(rustix_io)?;
            let mut opened = fstat(&child).map_err(rustix_io)?;
            if !same_reset_directory_identity(&observed, &opened) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection reset directory changed while opening",
                ));
            }
            if opened.st_mode & 0o777 != 0o700 {
                fchmod(&child, Mode::from_raw_mode(0o700)).map_err(rustix_io)?;
                *mutated = true;
                fsync(&child).map_err(rustix_io)?;
                let restricted = fstat(&child).map_err(rustix_io)?;
                if !same_reset_directory_identity(&opened, &restricted) || restricted.st_mode & 0o777 != 0o700 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "projection reset directory changed while restricting permissions",
                    ));
                }
                opened = restricted;
                fsync(directory).map_err(rustix_io)?;
            }
            cleanup_projection_reset_directory(child.as_fd(), root_device, depth + 1, visited, mutated)?;
            fsync(&child).map_err(rustix_io)?;
            let current = statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
            if !same_reset_directory_identity(&opened, &current) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection reset directory changed before removal",
                ));
            }
            unlinkat(directory, &name, AtFlags::REMOVEDIR).map_err(rustix_io)?;
            *mutated = true;
        } else {
            let current = statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW).map_err(rustix_io)?;
            if !same_reset_leaf_identity(&observed, &current) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection reset entry changed before removal",
                ));
            }
            unlinkat(directory, &name, AtFlags::empty()).map_err(rustix_io)?;
            *mutated = true;
        }
        fsync(directory).map_err(rustix_io)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn same_reset_directory_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && FileType::from_raw_mode(left.st_mode) == FileType::Directory
        && FileType::from_raw_mode(right.st_mode) == FileType::Directory
}

#[cfg(target_os = "linux")]
fn same_reset_leaf_identity(left: &rustix::fs::Stat, right: &rustix::fs::Stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_mode == right.st_mode
        && left.st_size == right.st_size
}

#[cfg(target_os = "linux")]
fn reset_marker_cleanup_identity_matches_stat(
    identity: &ProjectionResetMarkerEntryIdentity,
    stat: &rustix::fs::Stat,
) -> bool {
    identity.device == stat.st_dev.to_string()
        && identity.inode == stat.st_ino.to_string()
        && identity.kind == reset_stat_kind(stat.st_mode)
        && (identity.mode == stat.st_mode & 0o777 || (identity.kind == 1 && stat.st_mode & 0o777 == 0o700))
}

#[cfg(target_os = "linux")]
fn reset_stat_kind(mode: u32) -> u8 {
    match FileType::from_raw_mode(mode) {
        FileType::Directory => 1,
        FileType::RegularFile => 2,
        FileType::Symlink => 3,
        FileType::Fifo => 4,
        FileType::Socket => 5,
        FileType::BlockDevice => 6,
        FileType::CharacterDevice => 7,
        _ => 255,
    }
}

#[cfg(target_os = "linux")]
fn remove_projection_reset_container(
    vault_root: &Path,
    basename: &str,
    expected: ProjectionPairDirectoryIdentity,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    let vault = open(
        vault_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    let observed = statat(&vault, basename, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if observed.st_dev as u64 != expected.device
        || observed.st_ino as u64 != expected.inode
        || FileType::from_raw_mode(observed.st_mode) != FileType::Directory
        || observed.st_mode & 0o777 != 0o700
    {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    unlinkat(&vault, basename, AtFlags::REMOVEDIR).map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    fsync(&vault).map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)
}

#[cfg(not(target_os = "linux"))]
fn remove_projection_reset_container(
    _vault_root: &Path,
    _basename: &str,
    _expected: ProjectionPairDirectoryIdentity,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    Err(ProjectionResetEvidenceMutationError::Unavailable)
}

fn marker_staging_basename(marker: &ProjectionResetMarker) -> Option<&str> {
    match &marker.payload.phase {
        ProjectionResetMarkerPhase::Prepared { binding }
        | ProjectionResetMarkerPhase::AppliedMaintenance { binding, .. } => {
            Some(binding.staging_container_basename.as_str())
        }
    }
}

#[cfg(target_os = "linux")]
fn rustix_io(error: rustix::io::Errno) -> std::io::Error {
    std::io::Error::from_raw_os_error(error.raw_os_error())
}

#[cfg(all(test, unix))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectionTreeFingerprintEntry {
    pub(crate) relative_path: String,
    pub(crate) file_type: &'static str,
    pub(crate) mode: u32,
    pub(crate) size: u64,
    pub(crate) modified_seconds: i64,
    pub(crate) modified_nanoseconds: i64,
    pub(crate) content_digest: Option<String>,
}

#[cfg(all(test, unix))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectionTreeAtimeEntry {
    pub(crate) relative_path: String,
    pub(crate) accessed_seconds: i64,
    pub(crate) accessed_nanoseconds: i64,
}

#[cfg(all(test, unix))]
impl PersonalVaultStorage {
    pub(crate) fn projection_tree_fingerprint_for_test(&self) -> std::io::Result<Vec<ProjectionTreeFingerprintEntry>> {
        self.named_tree_fingerprint_for_test(&["projection-store"])
    }

    pub(crate) fn projection_reset_protocol_fingerprint_for_test(
        &self,
    ) -> std::io::Result<Vec<ProjectionTreeFingerprintEntry>> {
        let (lease, _) = self.projection_artifact_parts();
        let marker = read_projection_reset_marker(&lease.vault_root)
            .map_err(|_| std::io::Error::other("projection reset marker is unreadable"))?
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "projection reset marker is absent"))?;
        let binding = marker_binding(&marker);
        let names = [
            PROJECTION_STORE_DIRECTORY.to_string(),
            PROJECTION_RESET_MARKER_FILE.to_string(),
            binding.transition_evidence_basename.clone(),
            binding.staging_container_basename.clone(),
        ];
        let names = names.iter().map(String::as_str).collect::<Vec<_>>();
        self.named_tree_fingerprint_for_test(&names)
    }

    pub(crate) fn canonical_tree_fingerprint_for_test(&self) -> std::io::Result<Vec<ProjectionTreeFingerprintEntry>> {
        self.named_tree_fingerprint_for_test(&["memory-ledger"])
    }

    fn named_tree_fingerprint_for_test(&self, names: &[&str]) -> std::io::Result<Vec<ProjectionTreeFingerprintEntry>> {
        use walkdir::WalkDir;

        let (lease, _) = self.projection_artifact_parts();
        let mut entries = Vec::new();
        for name in names {
            let path = lease.vault_root.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(_) => {
                    for entry in WalkDir::new(&path).follow_links(false).same_file_system(true) {
                        let entry = entry.map_err(std::io::Error::other)?;
                        fingerprint_projection_entry(&lease.vault_root, entry.path(), &mut entries)?;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(entries)
    }

    pub(crate) fn projection_tree_atimes_for_test(
        &self,
        fingerprint: &[ProjectionTreeFingerprintEntry],
    ) -> std::io::Result<Vec<ProjectionTreeAtimeEntry>> {
        self.tree_atimes_for_test(fingerprint)
    }

    pub(crate) fn canonical_tree_atimes_for_test(
        &self,
        fingerprint: &[ProjectionTreeFingerprintEntry],
    ) -> std::io::Result<Vec<ProjectionTreeAtimeEntry>> {
        self.tree_atimes_for_test(fingerprint)
    }

    fn tree_atimes_for_test(
        &self,
        fingerprint: &[ProjectionTreeFingerprintEntry],
    ) -> std::io::Result<Vec<ProjectionTreeAtimeEntry>> {
        use std::os::unix::fs::MetadataExt;

        let (lease, _) = self.projection_artifact_parts();
        fingerprint
            .iter()
            .map(|entry| {
                let path = lease.vault_root.join(&entry.relative_path);
                let metadata = std::fs::symlink_metadata(path)?;
                if metadata.file_type().is_symlink()
                    || !(metadata.file_type().is_dir() || metadata.file_type().is_file())
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "projection atime fixture encountered a special file",
                    ));
                }
                Ok(ProjectionTreeAtimeEntry {
                    relative_path: entry.relative_path.clone(),
                    accessed_seconds: metadata.atime(),
                    accessed_nanoseconds: metadata.atime_nsec(),
                })
            })
            .collect()
    }
}

#[cfg(all(test, unix))]
fn fingerprint_projection_entry(
    root: &std::path::Path,
    path: &std::path::Path,
    output: &mut Vec<ProjectionTreeFingerprintEntry>,
) -> std::io::Result<()> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !(metadata.file_type().is_dir() || metadata.file_type().is_file()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection fingerprint encountered a special file",
        ));
    }
    let content_digest = if metadata.file_type().is_file() {
        let bytes = read_private_file_bounded(path, 32 * 1024 * 1024)?;
        Some(format!("{:x}", Sha256::digest(bytes)))
    } else {
        None
    };
    let relative = path
        .strip_prefix(root)
        .map_err(|_| std::io::Error::other("projection fingerprint path escaped vault"))?
        .to_str()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 projection path"))?;
    output.push(ProjectionTreeFingerprintEntry {
        relative_path: relative.to_string(),
        file_type: if metadata.file_type().is_dir() {
            "directory"
        } else {
            "file"
        },
        mode: metadata.permissions().mode() & 0o777,
        size: metadata.size(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        content_digest,
    });
    Ok(())
}

impl ProjectionStorageBundle {
    pub(crate) fn reset_operation_id(&self) -> Option<&str> {
        self.reset_operation_id.as_deref()
    }

    pub(crate) fn read_manifest_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        self.manifest.read_active_bounded(maximum_bytes)
    }

    pub(crate) fn read_manifest_object_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.manifest.get_immutable_bounded(hash, maximum_bytes)
    }

    pub(crate) fn put_manifest_object(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.manifest.put_immutable(hash, bytes)
    }

    pub(crate) fn list_manifest_objects(&self) -> std::io::Result<Vec<String>> {
        self.manifest.list_immutable_hashes()
    }

    pub(crate) fn manifest_inventory_matches(&self, expected: &HashSet<String>) -> std::io::Result<bool> {
        Ok(self
            .manifest
            .list_immutable_hashes_bounded(MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES)?
            .into_iter()
            .collect::<HashSet<_>>()
            == *expected)
    }

    pub(crate) fn flush_manifest(&self) -> std::io::Result<()> {
        self.manifest.flush()
    }

    pub(crate) fn cleanup_unreferenced_manifest_objects(
        &self,
        referenced_hashes: &HashSet<String>,
    ) -> std::io::Result<usize> {
        self.manifest.remove_unreferenced_projection_objects(referenced_hashes)
    }

    #[cfg(unix)]
    pub(crate) fn cleanup_all_artifact_orphans(&self, maximum_bytes: u64) -> std::io::Result<usize> {
        self.artifacts
            .cleanup_all_bounded(maximum_bytes, MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES)
    }

    pub(crate) fn publish_manifest_active(&self, bytes: &[u8]) -> Result<(), super::LedgerStorageError> {
        self.manifest.publish_active(bytes)
    }

    pub(crate) fn read_artifact_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.artifacts.get_immutable_bounded(hash, maximum_bytes)
    }

    pub(crate) fn put_artifact(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.artifacts.put_immutable(hash, bytes)
    }

    pub(crate) fn flush_artifacts(&self) -> std::io::Result<()> {
        self.artifacts.flush()
    }

    pub(crate) fn validate_artifact_inventory(
        &self,
        referenced_hashes: &HashSet<String>,
        maximum_bytes: u64,
    ) -> std::io::Result<()> {
        self.artifacts.validate_inventory(referenced_hashes, maximum_bytes)
    }

    pub(crate) fn artifact_inventory_is_empty(&self, maximum_bytes: u64) -> std::io::Result<bool> {
        Ok(self
            .artifacts
            .list_immutable_hashes_bounded(maximum_bytes, MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES)?
            .is_empty())
    }

    #[cfg(unix)]
    pub(crate) fn snapshot_artifact_for_cleanup(
        &self,
        hash: &str,
        maximum_bytes: u64,
    ) -> std::io::Result<ProjectionArtifactSnapshot> {
        self.artifacts.snapshot_for_cleanup(hash, maximum_bytes)
    }

    #[cfg(unix)]
    pub(crate) fn remove_artifact_snapshot(&self, snapshot: ProjectionArtifactSnapshot) -> std::io::Result<()> {
        self.artifacts.remove_snapshot(snapshot)
    }

    #[cfg(test)]
    pub(crate) fn inject_post_artifact_durability_failure_once(&self) {
        self.artifacts.inject_post_artifact_durability_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_post_exchange_sync_failure_once(&self) {
        self.manifest.inject_post_exchange_sync_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn artifact_hashes(&self, maximum_bytes: u64) -> std::io::Result<Vec<String>> {
        self.artifacts.list_immutable_hashes(maximum_bytes)
    }

    #[cfg(test)]
    pub(crate) fn inject_remove_artifact(&self, hash: &str) -> std::io::Result<()> {
        self.artifacts.inject_remove_for_test(hash)
    }

    #[cfg(test)]
    pub(crate) fn inject_artifact_cleanup_failure_once(&self) {
        self.artifacts.inject_cleanup_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_replace_artifact(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.artifacts.inject_replace_for_test(hash, bytes)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_permissive_artifact_mode(&self, hash: &str) -> std::io::Result<()> {
        self.artifacts.inject_permissive_mode_for_test(hash)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_artifact_symlink(&self, hash: &str) -> std::io::Result<()> {
        self.artifacts.inject_symlink_for_test(hash)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_artifact_fifo(&self, hash: &str) -> std::io::Result<()> {
        self.artifacts.inject_fifo_for_test(hash)
    }
}

fn open_projection_artifacts(
    lease: Arc<PersonalVaultLease>,
    artifact_directory: PathBuf,
    create: bool,
) -> Result<ProjectionArtifactStorage, LedgerStorageOpenError> {
    if create {
        ensure_directory(&artifact_directory, &lease.vault_root)?;
    } else {
        require_private_directory(&artifact_directory).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "projection artifact root is not private",
            )
        })?;
    }
    let objects_directory = artifact_directory.join("objects");
    if create {
        ensure_directory(&objects_directory, &artifact_directory)?;
    } else {
        require_private_directory(&objects_directory).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "projection artifact object directory is not private",
            )
        })?;
    }
    Ok(ProjectionArtifactStorage {
        _lease: lease,
        artifact_directory,
        objects_directory,
        #[cfg(test)]
        fail_after_durable_put_once: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        fail_cleanup_once: std::sync::atomic::AtomicBool::new(false),
    })
}

fn open_projection_bundle_at(
    lease: Arc<PersonalVaultLease>,
    pair_path: &Path,
    create: bool,
) -> Result<ProjectionStorageBundle, LedgerStorageOpenError> {
    if create {
        ensure_directory(
            pair_path,
            pair_path
                .parent()
                .ok_or_else(|| std::io::Error::other("missing pair parent"))?,
        )?;
    } else {
        require_private_directory(pair_path)?;
    }
    let manifest = open_immutable_ledger_directory(
        Arc::clone(&lease),
        pair_path.join(PROJECTION_MANIFEST_DIRECTORY),
        create,
    )?;
    let artifacts = open_projection_artifacts(lease, pair_path.join(PROJECTION_ARTIFACT_DIRECTORY), create)?;
    Ok(ProjectionStorageBundle {
        manifest,
        artifacts,
        reset_operation_id: None,
    })
}

fn rollback_projection_claims(lease: &PersonalVaultLease) -> Result<(), LedgerStorageOpenError> {
    let mut namespace_claims = lease
        .claimed_namespaces
        .lock()
        .map_err(|_| std::io::Error::other("personal vault namespace claim state is poisoned"))?;
    namespace_claims.remove(&ImmutableLedgerNamespace::ProjectionManifest);
    let mut artifacts_claimed = lease
        .projection_artifacts_claimed
        .lock()
        .map_err(|_| std::io::Error::other("projection artifact claim state is poisoned"))?;
    *artifacts_claimed = false;
    Ok(())
}

fn reject_legacy_projection_layout(vault_root: &Path) -> Result<(), LedgerStorageOpenError> {
    for name in ["projection-manifest", "projection-artifacts"] {
        match std::fs::symlink_metadata(vault_root.join(name)) {
            Ok(_) => {
                return Err(LedgerStorageOpenError::UnsupportedProjectionFormat);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub(super) fn reject_projection_reset_pending(vault_root: &Path) -> Result<(), LedgerStorageOpenError> {
    match read_projection_reset_marker(vault_root)? {
        Some(marker) => {
            preflight_projection_reset_protocol(vault_root, &marker)?;
            match marker.payload.phase {
                ProjectionResetMarkerPhase::Prepared { .. } => Err(LedgerStorageOpenError::ProjectionResetPending),
                ProjectionResetMarkerPhase::AppliedMaintenance { .. } => {
                    Err(LedgerStorageOpenError::ProjectionResetMaintenanceRequired)
                }
            }
        }
        None => Ok(()),
    }
}

fn read_projection_reset_marker(vault_root: &Path) -> Result<Option<ProjectionResetMarker>, LedgerStorageOpenError> {
    let path = vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    read_projection_reset_marker_file(&path, &metadata).map(Some)
}

fn read_projection_reset_marker_file(
    path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<ProjectionResetMarker, LedgerStorageOpenError> {
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || projection_mode(metadata) != 0o600
        || metadata.len() > 4096
    {
        return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
    }
    let bytes = read_private_file_bounded(path, 4096)?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| LedgerStorageOpenError::ProjectionResetManualIntervention)?;
    match value.get("payload").and_then(|payload| payload.get("schema")) {
        Some(serde_json::Value::String(schema)) if schema == PROJECTION_RESET_MARKER_SCHEMA => {}
        Some(serde_json::Value::String(_)) => {
            return Err(LedgerStorageOpenError::UnsupportedProjectionFormat);
        }
        _ => return Err(LedgerStorageOpenError::ProjectionResetManualIntervention),
    }
    let canonical = serde_json_canonicalizer::to_vec(&value)
        .map_err(|_| LedgerStorageOpenError::ProjectionResetManualIntervention)?;
    if canonical != bytes {
        return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
    }
    let marker: ProjectionResetMarker =
        serde_json::from_value(value).map_err(|_| LedgerStorageOpenError::ProjectionResetManualIntervention)?;
    validate_projection_reset_marker_payload(&marker)
        .map_err(|_| LedgerStorageOpenError::ProjectionResetManualIntervention)?;
    Ok(marker)
}

fn preflight_projection_reset_protocol(
    vault_root: &Path,
    marker: &ProjectionResetMarker,
) -> Result<(), LedgerStorageOpenError> {
    let binding = marker_binding(marker);
    let transition_path = vault_root.join(&binding.transition_evidence_basename);
    match std::fs::symlink_metadata(&transition_path) {
        Ok(metadata) => {
            read_projection_reset_marker_file(&transition_path, &metadata)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let container_path = vault_root.join(&binding.staging_container_basename);
    let (container, identity) = match std::fs::symlink_metadata(&container_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.file_type().is_dir()
                || projection_mode(&metadata) != 0o700
                || reset_marker_entry_identity(projection_pair_entry_identity(&metadata))
                    != binding.staging_container_identity
            {
                return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
            }
            open_private_directory_identity(&container_path)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return if matches!(
                marker.payload.phase,
                ProjectionResetMarkerPhase::AppliedMaintenance { .. }
            ) {
                Ok(())
            } else {
                Err(LedgerStorageOpenError::ProjectionResetManualIntervention)
            };
        }
        Err(error) => return Err(error.into()),
    };
    if reset_marker_directory_identity(identity) != binding.staging_container_identity {
        return Err(LedgerStorageOpenError::ProjectionResetManualIntervention);
    }
    let seal = read_optional_projection_reset_seal(&container).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => LedgerStorageOpenError::UnsupportedProjectionFormat,
        ProjectionResetProtocolError::Invalid => LedgerStorageOpenError::ProjectionResetManualIntervention,
        ProjectionResetProtocolError::Io(error) => LedgerStorageOpenError::Io(error),
    })?;
    let pair_exists = projection_reset_container_entry_exists(&container, PROJECTION_STORE_DIRECTORY)?;
    match (&marker.payload.phase, pair_exists, seal.is_some()) {
        (ProjectionResetMarkerPhase::Prepared { .. }, true, true)
        | (ProjectionResetMarkerPhase::AppliedMaintenance { .. }, true, true)
        | (ProjectionResetMarkerPhase::AppliedMaintenance { .. }, false, true)
        | (ProjectionResetMarkerPhase::AppliedMaintenance { .. }, false, false) => {}
        _ => return Err(LedgerStorageOpenError::ProjectionResetManualIntervention),
    }
    if matches!(
        marker.payload.phase,
        ProjectionResetMarkerPhase::AppliedMaintenance { .. }
    ) && pair_exists
    {
        validate_projection_reset_quarantine_cleanup(&container, marker).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound
            ) || matches!(error.kind(), std::io::ErrorKind::PermissionDenied) && error.raw_os_error().is_none()
            {
                LedgerStorageOpenError::ProjectionResetManualIntervention
            } else {
                LedgerStorageOpenError::Io(error)
            }
        })?;
    }
    Ok(())
}

fn validate_projection_reset_marker_payload(marker: &ProjectionResetMarker) -> std::io::Result<()> {
    let payload_bytes = serde_json_canonicalizer::to_vec(&marker.payload)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if marker.payload.schema != PROJECTION_RESET_MARKER_SCHEMA
        || marker.marker_hash != domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes)
        || !is_lower_hex_hash(&marker.marker_hash)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid projection reset marker envelope",
        ));
    }
    let transaction = uuid::Uuid::parse_str(&marker.payload.transaction_id).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid projection reset transaction identity",
        )
    })?;
    if transaction.is_nil()
        || transaction.get_version() != Some(uuid::Version::Random)
        || transaction.hyphenated().to_string() != marker.payload.transaction_id
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "non-canonical projection reset transaction identity",
        ));
    }
    let binding = match &marker.payload.phase {
        ProjectionResetMarkerPhase::Prepared { binding }
        | ProjectionResetMarkerPhase::AppliedMaintenance { binding, .. } => binding,
    };
    let expected_transition_basename = format!(
        "{PROJECTION_RESET_MARKER_TRANSITION_PREFIX}{}",
        marker.payload.transaction_id
    );
    if !is_strict_reset_basename(&binding.staging_container_basename, ".plico-projection-pair-staging.")
        || binding.transition_evidence_basename != expected_transition_basename
        || !is_strict_reset_basename(
            &binding.transition_evidence_basename,
            PROJECTION_RESET_MARKER_TRANSITION_PREFIX,
        )
        || binding.staging_container_basename.len() > 255
        || !is_lower_hex_hash(&binding.target_seal_hash)
        || !is_lower_hex_hash(&binding.target_tree_digest)
        || !is_lower_hex_hash(&binding.target_active_pointer_digest)
        || !is_lower_hex_hash(&binding.expected_live_fingerprint)
        || !matches!(
            binding.reset_reason.as_str(),
            "manifest_incomplete"
                | "manifest_integrity_invalid"
                | "storage_layout_invalid"
                | "canonical_lineage_invalid"
        )
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid projection reset marker binding",
        ));
    }
    validate_reset_marker_identity(&binding.staging_container_identity)?;
    validate_reset_marker_identity(&binding.target_pair_identity)?;
    validate_reset_marker_identity(&binding.expected_live_identity)?;
    if binding.staging_container_identity.mode != 0o700
        || binding.staging_container_identity.kind != 1
        || binding.target_pair_identity.mode != 0o700
        || binding.target_pair_identity.kind != 1
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker directories are not private",
        ));
    }
    if let ProjectionResetMarkerPhase::AppliedMaintenance {
        new_live_identity,
        quarantine_container_identity,
        quarantined_entry_identity,
        ..
    } = &marker.payload.phase
    {
        validate_reset_marker_identity(new_live_identity)?;
        validate_reset_marker_identity(quarantine_container_identity)?;
        validate_reset_marker_identity(quarantined_entry_identity)?;
        if new_live_identity != &binding.target_pair_identity
            || quarantine_container_identity != &binding.staging_container_identity
            || quarantined_entry_identity != &binding.expected_live_identity
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset marker topology is inconsistent",
            ));
        }
    }
    Ok(())
}

fn is_strict_reset_basename(value: &str, prefix: &str) -> bool {
    Path::new(value).components().count() == 1
        && value != "."
        && value != ".."
        && value.starts_with(prefix)
        && value.len() > prefix.len()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validate_reset_marker_identity(identity: &ProjectionResetMarkerEntryIdentity) -> std::io::Result<()> {
    if identity.mode > 0o777 || !(1..=7).contains(&identity.kind) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid projection reset entry metadata",
        ));
    }
    for value in [&identity.device, &identity.inode] {
        let parsed = value.parse::<u64>().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid projection reset entry identity",
            )
        })?;
        if parsed.to_string() != *value {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "non-canonical projection reset entry identity",
            ));
        }
    }
    Ok(())
}

fn is_lower_hex_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn persist_projection_reset_marker(
    target: &ProjectionPairTarget,
    seal: &ProjectionPairSeal,
) -> Result<ProjectionResetMarker, ProjectionResetMarkerWriteError> {
    let container = target.container.as_ref().ok_or_else(|| {
        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::other(
            "projection reset target lost its staging container",
        ))
    })?;
    let basename = container
        .path()
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| value.starts_with(".plico-projection-pair-staging."))
        .ok_or_else(|| {
            ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid projection staging basename",
            ))
        })?;
    let payload = ProjectionResetMarkerPayload {
        schema: PROJECTION_RESET_MARKER_SCHEMA.to_string(),
        transaction_id: target
            .reset_transaction_id
            .ok_or_else(|| {
                ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "projection reset target has no transaction identity",
                ))
            })?
            .hyphenated()
            .to_string(),
        phase: ProjectionResetMarkerPhase::Prepared {
            binding: ProjectionResetPreparedBinding {
                staging_container_basename: basename.to_string(),
                transition_evidence_basename: format!(
                    "{PROJECTION_RESET_MARKER_TRANSITION_PREFIX}{}",
                    target
                        .reset_transaction_id
                        .expect("projection reset transaction was checked above")
                        .hyphenated()
                ),
                staging_container_identity: reset_marker_directory_identity(target.directory_identity),
                target_pair_identity: reset_marker_directory_identity(target.pair_identity),
                target_seal_hash: seal.seal_hash.clone(),
                target_tree_digest: seal.payload.tree_digest.clone(),
                target_active_pointer_digest: seal.payload.active_pointer_digest.clone(),
                expected_live_identity: {
                    let identity = target.expected_live_identity.ok_or_else(|| {
                        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "projection reset target has no expected live identity",
                        ))
                    })?;
                    reset_marker_entry_identity(identity)
                },
                expected_live_fingerprint: target.expected_live_fingerprint.clone().ok_or_else(|| {
                    ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "projection reset target has no live fingerprint",
                    ))
                })?,
                reset_reason: target
                    .reset_reason
                    .ok_or_else(|| {
                        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "projection reset target has no typed reset reason",
                        ))
                    })?
                    .as_str()
                    .to_string(),
            },
        },
    };
    let payload_bytes = serde_json_canonicalizer::to_vec(&payload).map_err(|error| {
        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    let marker = ProjectionResetMarker {
        marker_hash: domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes),
        payload,
    };
    let bytes = serde_json_canonicalizer::to_vec(&marker).map_err(|error| {
        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    let path = target.lease.vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let mut temporary = tempfile::NamedTempFile::new_in(&target.lease.vault_root)
        .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary
        .write_all(&bytes)
        .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary.persist_noclobber(&path).map_err(|error| {
        if std::fs::symlink_metadata(&path).is_ok() {
            ProjectionResetMarkerWriteError::Pending
        } else {
            ProjectionResetMarkerWriteError::PrePublish(error.error)
        }
    })?;
    set_file_mode(&path).map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    sync_directory(&target.lease.vault_root).map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    Ok(marker)
}

fn persist_applied_projection_reset_marker(
    topology: &ProjectionResetAppliedTopology<'_>,
    prepared: &ProjectionResetMarker,
) -> Result<ProjectionResetMarker, ProjectionResetMarkerWriteError> {
    let ProjectionResetMarkerPhase::Prepared { .. } = &prepared.payload.phase else {
        return Err(ProjectionResetMarkerWriteError::Pending);
    };
    let live_path = topology.vault_root.join(PROJECTION_STORE_DIRECTORY);
    let quarantine_path = topology.container_path.join(PROJECTION_STORE_DIRECTORY);
    let live_identity = projection_pair_entry_identity(
        &std::fs::symlink_metadata(&live_path).map_err(|_| ProjectionResetMarkerWriteError::Pending)?,
    );
    let quarantine_identity = projection_pair_entry_identity(
        &std::fs::symlink_metadata(&quarantine_path).map_err(|_| ProjectionResetMarkerWriteError::Pending)?,
    );
    if live_identity != directory_entry_identity(topology.target_pair_identity)
        || quarantine_identity != topology.expected_live_identity
    {
        return Err(ProjectionResetMarkerWriteError::Pending);
    }
    let marker = build_applied_projection_reset_marker(
        prepared,
        topology.container_identity,
        live_identity,
        quarantine_identity,
    )
    .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    replace_projection_reset_marker(topology, prepared, &marker)?;
    Ok(marker)
}

fn build_applied_projection_reset_marker(
    prepared: &ProjectionResetMarker,
    container_identity: ProjectionPairDirectoryIdentity,
    live_identity: ProjectionPairEntryIdentity,
    quarantine_identity: ProjectionPairEntryIdentity,
) -> std::io::Result<ProjectionResetMarker> {
    let ProjectionResetMarkerPhase::Prepared { binding } = &prepared.payload.phase else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "projection reset marker is not prepared",
        ));
    };
    let payload = ProjectionResetMarkerPayload {
        schema: PROJECTION_RESET_MARKER_SCHEMA.to_string(),
        transaction_id: prepared.payload.transaction_id.clone(),
        phase: ProjectionResetMarkerPhase::AppliedMaintenance {
            binding: binding.clone(),
            new_live_identity: reset_marker_entry_identity(live_identity),
            quarantine_container_identity: reset_marker_directory_identity(container_identity),
            quarantined_entry_identity: reset_marker_entry_identity(quarantine_identity),
        },
    };
    build_projection_reset_marker(payload)
}

fn build_projection_reset_marker(payload: ProjectionResetMarkerPayload) -> std::io::Result<ProjectionResetMarker> {
    let payload_bytes = serde_json_canonicalizer::to_vec(&payload)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(ProjectionResetMarker {
        marker_hash: domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes),
        payload,
    })
}

struct ProjectionResetMarkerFileEvidence {
    file: File,
    identity: ArtifactIdentity,
    bytes: Vec<u8>,
}

#[cfg(target_os = "linux")]
type ProjectionResetSealStat = rustix::fs::Stat;
#[cfg(not(target_os = "linux"))]
type ProjectionResetSealStat = ();

struct ProjectionResetSealEvidence {
    seal: ProjectionPairSeal,
    stat: ProjectionResetSealStat,
    file: File,
    bytes: Vec<u8>,
}

fn replace_projection_reset_marker(
    topology: &ProjectionResetAppliedTopology<'_>,
    expected: &ProjectionResetMarker,
    replacement: &ProjectionResetMarker,
) -> Result<(), ProjectionResetMarkerWriteError> {
    let vault_root = topology.vault_root;
    let binding = marker_binding(expected);
    let active_path = vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let transition_path = vault_root.join(&binding.transition_evidence_basename);
    let active_evidence = open_projection_reset_marker_evidence(&active_path, expected)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    let bytes = serde_json_canonicalizer::to_vec(replacement).map_err(|error| {
        ProjectionResetMarkerWriteError::PrePublish(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(vault_root).map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary
        .write_all(&bytes)
        .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(ProjectionResetMarkerWriteError::PrePublish)?;
    temporary
        .persist_noclobber(&transition_path)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    sync_directory(vault_root).map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    let transition_evidence = open_projection_reset_marker_evidence(&transition_path, replacement)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    validate_projection_reset_marker_evidence_path(&active_path, &active_evidence, expected)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    #[cfg(test)]
    if take_projection_reset_fault(topology.lease, RESET_FAULT_AFTER_TRANSITION_PERSIST) {
        return Err(ProjectionResetMarkerWriteError::Pending);
    }
    renameat_with(CWD, &active_path, CWD, &transition_path, RenameFlags::EXCHANGE)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    validate_projection_reset_marker_evidence_path(&active_path, &transition_evidence, replacement)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    validate_projection_reset_marker_evidence_path(&transition_path, &active_evidence, expected)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    #[cfg(test)]
    if take_projection_reset_fault(topology.lease, RESET_FAULT_AFTER_MARKER_EXCHANGE) {
        return Err(ProjectionResetMarkerWriteError::Pending);
    }
    sync_directory(vault_root).map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    validate_applied_projection_topology(topology, replacement)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)?;
    unlink_projection_reset_marker_evidence(vault_root, &transition_path, &active_evidence, expected)
        .map_err(|_| ProjectionResetMarkerWriteError::Pending)
}

fn marker_binding(marker: &ProjectionResetMarker) -> &ProjectionResetPreparedBinding {
    match &marker.payload.phase {
        ProjectionResetMarkerPhase::Prepared { binding }
        | ProjectionResetMarkerPhase::AppliedMaintenance { binding, .. } => binding,
    }
}

fn resolve_applied_transition_evidence(
    vault_root: &Path,
    applied: &ProjectionResetMarker,
) -> Result<bool, ProjectionResetEvidenceMutationError> {
    let binding = marker_binding(applied);
    let transition_path = vault_root.join(&binding.transition_evidence_basename);
    match std::fs::symlink_metadata(&transition_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(ProjectionResetEvidenceMutationError::Unavailable),
        Ok(_) => {}
    }
    let prepared = build_projection_reset_marker(ProjectionResetMarkerPayload {
        schema: PROJECTION_RESET_MARKER_SCHEMA.to_string(),
        transaction_id: applied.payload.transaction_id.clone(),
        phase: ProjectionResetMarkerPhase::Prepared {
            binding: binding.clone(),
        },
    })
    .map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    let evidence = open_projection_reset_marker_evidence(&transition_path, &prepared).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetEvidenceMutationError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetEvidenceMutationError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetEvidenceMutationError::Unavailable,
    })?;
    unlink_projection_reset_marker_evidence_recoverable(vault_root, &transition_path, &evidence, &prepared)?;
    Ok(true)
}

fn recover_prepared_projection_reset(
    lease: &Arc<PersonalVaultLease>,
    prepared: &ProjectionResetMarker,
) -> Result<ProjectionResetMarker, ProjectionResetRecoveryError> {
    let binding = marker_binding(prepared);
    let container_path = lease.vault_root.join(&binding.staging_container_basename);
    let (container_directory, container_identity) =
        open_private_directory_identity(&container_path).map_err(projection_reset_recovery_pre_mutation_error)?;
    if reset_marker_directory_identity(container_identity) != binding.staging_container_identity {
        return Err(ProjectionResetRecoveryError::Manual);
    }
    let mut seal_evidence = read_projection_reset_seal(&container_directory).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetRecoveryError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetRecoveryError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetRecoveryError::Unavailable,
    })?;
    validate_projection_reset_seal_evidence(&container_directory, &mut seal_evidence).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetRecoveryError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetRecoveryError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetRecoveryError::Unavailable,
    })?;
    let target_pair_identity =
        marker_directory_identity(&binding.target_pair_identity).ok_or(ProjectionResetRecoveryError::Manual)?;
    let expected_live_identity =
        marker_entry_identity(&binding.expected_live_identity).map_err(projection_reset_recovery_pre_mutation_error)?;
    let topology = ProjectionResetAppliedTopology {
        lease,
        vault_root: &lease.vault_root,
        container_path: &container_path,
        container_identity,
        target_pair_identity,
        expected_live_identity,
    };
    let applied = build_applied_projection_reset_marker(
        prepared,
        container_identity,
        directory_entry_identity(target_pair_identity),
        expected_live_identity,
    )
    .map_err(projection_reset_recovery_pre_mutation_error)?;
    let transition_path = lease.vault_root.join(&binding.transition_evidence_basename);
    match std::fs::symlink_metadata(&transition_path) {
        Ok(_) => {
            validate_prepared_reset_post_exchange(&topology, binding, &seal_evidence.seal)
                .map_err(projection_reset_recovery_pre_mutation_error)?;
            finish_existing_projection_reset_marker_exchange(&topology, prepared, &applied).map_err(
                |error| match error {
                    ProjectionResetEvidenceMutationError::Manual => ProjectionResetRecoveryError::Manual,
                    ProjectionResetEvidenceMutationError::Indeterminate => ProjectionResetRecoveryError::Indeterminate,
                    ProjectionResetEvidenceMutationError::UnsupportedFormat => {
                        ProjectionResetRecoveryError::UnsupportedFormat
                    }
                    ProjectionResetEvidenceMutationError::Unavailable => ProjectionResetRecoveryError::Unavailable,
                },
            )?;
            verify_projection_reset_live(&lease.vault_root, &applied, Some(&seal_evidence.seal))
                .map_err(|_| ProjectionResetRecoveryError::Indeterminate)?;
            return Ok(applied);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(projection_reset_recovery_pre_mutation_error(error)),
    }

    let live_path = lease.vault_root.join(PROJECTION_STORE_DIRECTORY);
    let staged_pair = container_path.join(PROJECTION_STORE_DIRECTORY);
    let live_identity = projection_pair_entry_identity(
        &std::fs::symlink_metadata(&live_path).map_err(projection_reset_recovery_pre_mutation_error)?,
    );
    let staged_identity = projection_pair_entry_identity(
        &std::fs::symlink_metadata(&staged_pair).map_err(projection_reset_recovery_pre_mutation_error)?,
    );
    let pair_already_exchanged =
        live_identity == directory_entry_identity(target_pair_identity) && staged_identity == expected_live_identity;
    let pair_not_exchanged =
        live_identity == expected_live_identity && staged_identity == directory_entry_identity(target_pair_identity);
    let mut mutated_this_attempt = false;
    if pair_not_exchanged {
        verify_reset_target_binding(&staged_pair, &seal_evidence.seal, binding)
            .map_err(projection_reset_recovery_pre_mutation_error)?;
        if projection_reset_live_fingerprint(&live_path)
            .map_err(projection_reset_recovery_pre_mutation_error)?
            .digest
            != binding.expected_live_fingerprint
        {
            return Err(ProjectionResetRecoveryError::Manual);
        }
        validate_projection_reset_seal_evidence(&container_directory, &mut seal_evidence).map_err(
            |error| match error {
                ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetRecoveryError::UnsupportedFormat,
                ProjectionResetProtocolError::Invalid => ProjectionResetRecoveryError::Manual,
                ProjectionResetProtocolError::Io(_) => ProjectionResetRecoveryError::Unavailable,
            },
        )?;
        renameat_with(CWD, &staged_pair, CWD, &live_path, RenameFlags::EXCHANGE)
            .map_err(rustix_io)
            .map_err(projection_reset_recovery_pre_mutation_error)?;
        mutated_this_attempt = true;
        let live_sync = sync_directory(&lease.vault_root);
        let container_sync = container_directory.sync_all();
        if live_sync.is_err() || container_sync.is_err() {
            return Err(ProjectionResetRecoveryError::Indeterminate);
        }
    } else if !pair_already_exchanged {
        return Err(ProjectionResetRecoveryError::Manual);
    }
    validate_projection_reset_seal_evidence(&container_directory, &mut seal_evidence).map_err(|error| {
        if mutated_this_attempt {
            ProjectionResetRecoveryError::Indeterminate
        } else {
            match error {
                ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetRecoveryError::UnsupportedFormat,
                ProjectionResetProtocolError::Invalid => ProjectionResetRecoveryError::Manual,
                ProjectionResetProtocolError::Io(_) => ProjectionResetRecoveryError::Unavailable,
            }
        }
    })?;
    validate_prepared_reset_post_exchange(&topology, binding, &seal_evidence.seal)
        .map_err(|error| projection_reset_recovery_after_boundary_error(error, mutated_this_attempt))?;
    persist_applied_projection_reset_marker(&topology, prepared)
        .map_err(|_| ProjectionResetRecoveryError::Indeterminate)
}

fn validate_prepared_reset_post_exchange(
    topology: &ProjectionResetAppliedTopology<'_>,
    binding: &ProjectionResetPreparedBinding,
    seal: &ProjectionPairSeal,
) -> std::io::Result<()> {
    if topology.vault_root != topology.lease.vault_root {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset recovery vault lease mismatch",
        ));
    }
    let live = topology.vault_root.join(PROJECTION_STORE_DIRECTORY);
    let quarantine = topology.container_path.join(PROJECTION_STORE_DIRECTORY);
    if projection_pair_entry_identity(&std::fs::symlink_metadata(&live)?)
        != directory_entry_identity(topology.target_pair_identity)
        || projection_pair_entry_identity(&std::fs::symlink_metadata(&quarantine)?) != topology.expected_live_identity
        || projection_reset_live_fingerprint(&quarantine)?.digest != binding.expected_live_fingerprint
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset recovery post-exchange topology mismatch",
        ));
    }
    verify_reset_target_binding(&live, seal, binding)
}

fn verify_reset_target_binding(
    pair_path: &Path,
    seal: &ProjectionPairSeal,
    binding: &ProjectionResetPreparedBinding,
) -> std::io::Result<()> {
    if seal.seal_hash != binding.target_seal_hash
        || seal.payload.tree_digest != binding.target_tree_digest
        || seal.payload.active_pointer_digest != binding.target_active_pointer_digest
        || projection_pair_tree_digest(pair_path, Some(&seal.payload.genesis))? != binding.target_tree_digest
        || active_pointer_digest(pair_path)? != binding.target_active_pointer_digest
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset target no longer matches its prepared binding",
        ));
    }
    Ok(())
}

fn finish_existing_projection_reset_marker_exchange(
    topology: &ProjectionResetAppliedTopology<'_>,
    prepared: &ProjectionResetMarker,
    applied: &ProjectionResetMarker,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    let active_path = topology.vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let transition_path = topology
        .vault_root
        .join(&marker_binding(prepared).transition_evidence_basename);
    let active = open_projection_reset_marker_evidence(&active_path, prepared).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetEvidenceMutationError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetEvidenceMutationError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetEvidenceMutationError::Unavailable,
    })?;
    let transition = open_projection_reset_marker_evidence(&transition_path, applied).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetEvidenceMutationError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetEvidenceMutationError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetEvidenceMutationError::Unavailable,
    })?;
    renameat_with(CWD, &active_path, CWD, &transition_path, RenameFlags::EXCHANGE)
        .map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    validate_projection_reset_marker_evidence_path(&active_path, &transition, applied)
        .map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)?;
    validate_projection_reset_marker_evidence_path(&transition_path, &active, prepared)
        .map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)?;
    sync_directory(topology.vault_root).map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)?;
    validate_applied_projection_topology(topology, applied)
        .map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)?;
    unlink_projection_reset_marker_evidence_recoverable(topology.vault_root, &transition_path, &active, prepared)
        .map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)
}

fn open_projection_reset_marker_evidence(
    path: &Path,
    expected: &ProjectionResetMarker,
) -> Result<ProjectionResetMarkerFileEvidence, ProjectionResetProtocolError> {
    let path_metadata = std::fs::symlink_metadata(path).map_err(ProjectionResetProtocolError::Io)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.file_type().is_file()
        || projection_mode(&path_metadata) != 0o600
        || path_metadata.len() > 4096
    {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let mut file: File = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(|error| ProjectionResetProtocolError::Io(std::io::Error::from_raw_os_error(error.raw_os_error())))?
    .into();
    let identity = artifact_identity(&file.metadata().map_err(ProjectionResetProtocolError::Io)?);
    if identity != artifact_identity(&path_metadata) {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(ProjectionResetProtocolError::Io)?;
    if bytes.len() > 4096 {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    match value.get("payload").and_then(|payload| payload.get("schema")) {
        Some(serde_json::Value::String(schema)) if schema == PROJECTION_RESET_MARKER_SCHEMA => {}
        Some(serde_json::Value::String(_)) => return Err(ProjectionResetProtocolError::UnsupportedFormat),
        _ => return Err(ProjectionResetProtocolError::Invalid),
    }
    let canonical = serde_json_canonicalizer::to_vec(&value).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    let expected_bytes =
        serde_json_canonicalizer::to_vec(expected).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    if bytes != canonical || bytes != expected_bytes {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    validate_projection_reset_marker_payload(expected).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    Ok(ProjectionResetMarkerFileEvidence { file, identity, bytes })
}

fn validate_projection_reset_marker_evidence_path(
    path: &Path,
    evidence: &ProjectionResetMarkerFileEvidence,
    expected: &ProjectionResetMarker,
) -> Result<(), ProjectionResetProtocolError> {
    let metadata = std::fs::symlink_metadata(path).map_err(ProjectionResetProtocolError::Io)?;
    if artifact_identity(&metadata) != evidence.identity {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let mut reopened = open_projection_reset_marker_evidence(path, expected)?;
    if artifact_identity(&reopened.file.metadata().map_err(ProjectionResetProtocolError::Io)?) != evidence.identity
        || reopened.bytes != evidence.bytes
    {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    reopened.file.rewind().map_err(ProjectionResetProtocolError::Io)?;
    Ok(())
}

fn validate_applied_projection_topology(
    topology: &ProjectionResetAppliedTopology<'_>,
    marker: &ProjectionResetMarker,
) -> std::io::Result<()> {
    let ProjectionResetMarkerPhase::AppliedMaintenance {
        new_live_identity,
        quarantine_container_identity,
        quarantined_entry_identity,
        ..
    } = &marker.payload.phase
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker is not applied",
        ));
    };
    let live = std::fs::symlink_metadata(topology.vault_root.join(PROJECTION_STORE_DIRECTORY))?;
    let container_metadata = std::fs::symlink_metadata(topology.container_path)?;
    let quarantine = std::fs::symlink_metadata(topology.container_path.join(PROJECTION_STORE_DIRECTORY))?;
    if reset_marker_entry_identity(projection_pair_entry_identity(&live)) != *new_live_identity
        || reset_marker_entry_identity(projection_pair_entry_identity(&container_metadata))
            != *quarantine_container_identity
        || reset_marker_entry_identity(projection_pair_entry_identity(&quarantine)) != *quarantined_entry_identity
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset applied topology mismatch",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn read_projection_reset_seal(container: &File) -> Result<ProjectionResetSealEvidence, ProjectionResetProtocolError> {
    let opened = openat2(
        container,
        PROJECTION_PAIR_SEAL_FILE,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOATIME,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
    )
    .map_err(|error| ProjectionResetProtocolError::Io(rustix_io(error)))?;
    let stat = fstat(&opened).map_err(|error| ProjectionResetProtocolError::Io(rustix_io(error)))?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size < 0
        || stat.st_size as u64 > 4096
    {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let mut file: File = opened.into();
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(ProjectionResetProtocolError::Io)?;
    if bytes.len() > 4096 {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    match value.get("payload").and_then(|payload| payload.get("schema")) {
        Some(serde_json::Value::String(schema)) if schema == PROJECTION_PAIR_SEAL_SCHEMA => {}
        Some(serde_json::Value::String(_)) => return Err(ProjectionResetProtocolError::UnsupportedFormat),
        _ => return Err(ProjectionResetProtocolError::Invalid),
    }
    let canonical_value =
        serde_json_canonicalizer::to_vec(&value).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    if canonical_value != bytes {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    let seal: ProjectionPairSeal = serde_json::from_value(value).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    let canonical = serde_json_canonicalizer::to_vec(&seal).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    let payload_bytes =
        serde_json_canonicalizer::to_vec(&seal.payload).map_err(|_| ProjectionResetProtocolError::Invalid)?;
    if bytes != canonical
        || seal.payload.schema != PROJECTION_PAIR_SEAL_SCHEMA
        || seal.payload.artifact_count != 0
        || seal.seal_hash != domain_hash(PROJECTION_PAIR_SEAL_DOMAIN, &payload_bytes)
        || !is_lower_hex_hash(&seal.seal_hash)
        || seal.payload.genesis.validate().is_err()
    {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    Ok(ProjectionResetSealEvidence {
        seal,
        stat,
        file,
        bytes,
    })
}

#[cfg(target_os = "linux")]
fn read_optional_projection_reset_seal(
    container: &File,
) -> Result<Option<ProjectionResetSealEvidence>, ProjectionResetProtocolError> {
    match statat(container, PROJECTION_PAIR_SEAL_FILE, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => read_projection_reset_seal(container).map(Some),
        Err(rustix::io::Errno::NOENT) => Ok(None),
        Err(error) => Err(ProjectionResetProtocolError::Io(rustix_io(error))),
    }
}

#[cfg(target_os = "linux")]
fn projection_reset_container_entry_exists(container: &File, name: &str) -> std::io::Result<bool> {
    match statat(container, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(rustix::io::Errno::NOENT) => Ok(false),
        Err(error) => Err(rustix_io(error)),
    }
}

#[cfg(not(target_os = "linux"))]
fn read_projection_reset_seal(_container: &File) -> Result<ProjectionResetSealEvidence, ProjectionResetProtocolError> {
    Err(ProjectionResetProtocolError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset maintenance requires Linux openat2",
    )))
}

#[cfg(not(target_os = "linux"))]
fn read_optional_projection_reset_seal(
    _container: &File,
) -> Result<Option<ProjectionResetSealEvidence>, ProjectionResetProtocolError> {
    Err(ProjectionResetProtocolError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset maintenance requires Linux openat2",
    )))
}

#[cfg(not(target_os = "linux"))]
fn projection_reset_container_entry_exists(_container: &File, _name: &str) -> std::io::Result<bool> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset maintenance requires Linux openat2",
    ))
}

fn verify_projection_reset_live(
    vault_root: &Path,
    marker: &ProjectionResetMarker,
    seal: Option<&ProjectionPairSeal>,
) -> std::io::Result<()> {
    let ProjectionResetMarkerPhase::AppliedMaintenance {
        binding,
        new_live_identity,
        ..
    } = &marker.payload.phase
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker is not applied",
        ));
    };
    let live = vault_root.join(PROJECTION_STORE_DIRECTORY);
    let metadata = std::fs::symlink_metadata(&live)?;
    if reset_marker_entry_identity(projection_pair_entry_identity(&metadata)) != *new_live_identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset live identity mismatch",
        ));
    }
    if let Some(seal) = seal {
        if seal.seal_hash != binding.target_seal_hash
            || seal.payload.tree_digest != binding.target_tree_digest
            || seal.payload.active_pointer_digest != binding.target_active_pointer_digest
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset live seal binding mismatch",
            ));
        }
    }
    let tree = projection_pair_tree_digest(&live, seal.map(|seal| &seal.payload.genesis))?;
    let active = active_pointer_digest(&live)?;
    if tree != binding.target_tree_digest || active != binding.target_active_pointer_digest {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset live tree does not match the applied marker",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_projection_reset_seal_evidence(
    container: &File,
    evidence: &mut ProjectionResetSealEvidence,
) -> Result<(), ProjectionResetProtocolError> {
    let current = statat(container, PROJECTION_PAIR_SEAL_FILE, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|error| ProjectionResetProtocolError::Io(rustix_io(error)))?;
    let opened = fstat(&evidence.file).map_err(|error| ProjectionResetProtocolError::Io(rustix_io(error)))?;
    if !same_reset_leaf_identity(&evidence.stat, &current) || !same_reset_leaf_identity(&evidence.stat, &opened) {
        return Err(ProjectionResetProtocolError::Invalid);
    }
    evidence.file.rewind().map_err(ProjectionResetProtocolError::Io)?;
    let mut current_bytes = Vec::new();
    Read::by_ref(&mut evidence.file)
        .take(4097)
        .read_to_end(&mut current_bytes)
        .map_err(ProjectionResetProtocolError::Io)?;
    if current_bytes != evidence.bytes {
        let value: serde_json::Value =
            serde_json::from_slice(&current_bytes).map_err(|_| ProjectionResetProtocolError::Invalid)?;
        if matches!(
            value.get("payload").and_then(|payload| payload.get("schema")),
            Some(serde_json::Value::String(schema)) if schema != PROJECTION_PAIR_SEAL_SCHEMA
        ) {
            return Err(ProjectionResetProtocolError::UnsupportedFormat);
        }
        return Err(ProjectionResetProtocolError::Invalid);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_projection_reset_seal_evidence(
    _container: &File,
    _evidence: &mut ProjectionResetSealEvidence,
) -> Result<(), ProjectionResetProtocolError> {
    Err(ProjectionResetProtocolError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset maintenance requires Linux openat2",
    )))
}

#[cfg(target_os = "linux")]
fn remove_projection_reset_seal(
    container: &File,
    expected: &rustix::fs::Stat,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    let current = statat(container, PROJECTION_PAIR_SEAL_FILE, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if !same_reset_leaf_identity(expected, &current) {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    unlinkat(container, PROJECTION_PAIR_SEAL_FILE, AtFlags::empty())
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    fsync(container).map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)
}

#[cfg(not(target_os = "linux"))]
fn remove_projection_reset_seal(_container: &File, _expected: &()) -> Result<(), ProjectionResetEvidenceMutationError> {
    Err(ProjectionResetEvidenceMutationError::Unavailable)
}

fn unlink_projection_reset_marker_evidence(
    vault_root: &Path,
    path: &Path,
    evidence: &ProjectionResetMarkerFileEvidence,
    expected: &ProjectionResetMarker,
) -> std::io::Result<()> {
    validate_projection_reset_marker_evidence_path(path, evidence, expected).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat | ProjectionResetProtocolError::Invalid => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset evidence verification failed",
        ),
        ProjectionResetProtocolError::Io(error) => error,
    })?;
    unlink_projection_reset_marker_file(vault_root, path, evidence.identity)
}

fn unlink_projection_reset_marker_evidence_recoverable(
    vault_root: &Path,
    path: &Path,
    evidence: &ProjectionResetMarkerFileEvidence,
    expected: &ProjectionResetMarker,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    validate_projection_reset_marker_evidence_path(path, evidence, expected).map_err(|error| match error {
        ProjectionResetProtocolError::UnsupportedFormat => ProjectionResetEvidenceMutationError::UnsupportedFormat,
        ProjectionResetProtocolError::Invalid => ProjectionResetEvidenceMutationError::Manual,
        ProjectionResetProtocolError::Io(_) => ProjectionResetEvidenceMutationError::Unavailable,
    })?;
    unlink_projection_reset_marker_file_recoverable(vault_root, path, evidence.identity, None)
}

fn clear_projection_reset_marker(
    lease: &PersonalVaultLease,
    expected: &ProjectionResetMarker,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    let vault_root = &lease.vault_root;
    let path = vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let path_metadata =
        std::fs::symlink_metadata(&path).map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.file_type().is_file()
        || projection_mode(&path_metadata) != 0o600
        || path_metadata.len() > 4096
    {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    let mut file: File = open(
        &path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOATIME,
        Mode::empty(),
    )
    .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?
    .into();
    let identity = artifact_identity(
        &file
            .metadata()
            .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?,
    );
    if identity != artifact_identity(&path_metadata) {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if bytes.len() > 4096 {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    match value.get("payload").and_then(|payload| payload.get("schema")) {
        Some(serde_json::Value::String(schema)) if schema == PROJECTION_RESET_MARKER_SCHEMA => {}
        Some(serde_json::Value::String(_)) => {
            return Err(ProjectionResetEvidenceMutationError::UnsupportedFormat);
        }
        _ => return Err(ProjectionResetEvidenceMutationError::Manual),
    }
    let actual: ProjectionResetMarker =
        serde_json::from_value(value).map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    let canonical =
        serde_json_canonicalizer::to_vec(&actual).map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    let payload_bytes =
        serde_json_canonicalizer::to_vec(&actual.payload).map_err(|_| ProjectionResetEvidenceMutationError::Manual)?;
    if bytes != canonical
        || actual != *expected
        || actual.payload.schema != PROJECTION_RESET_MARKER_SCHEMA
        || actual.marker_hash != domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes)
    {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    if artifact_identity(
        &std::fs::symlink_metadata(&path).map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?,
    ) != identity
    {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    file.rewind()
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    let mut current = Vec::new();
    Read::by_ref(&mut file)
        .take(4097)
        .read_to_end(&mut current)
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if current != bytes {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    unlink_projection_reset_marker_file_recoverable(vault_root, &path, identity, Some(lease))
}

#[cfg(target_os = "linux")]
fn unlink_projection_reset_marker_file(
    vault_root: &Path,
    path: &Path,
    expected: ArtifactIdentity,
) -> std::io::Result<()> {
    unlink_projection_reset_marker_file_recoverable(vault_root, path, expected, None).map_err(|error| match error {
        ProjectionResetEvidenceMutationError::Manual => std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset evidence cleanup rejected",
        ),
        ProjectionResetEvidenceMutationError::Indeterminate => {
            std::io::Error::other("projection reset evidence cleanup durability is indeterminate")
        }
        ProjectionResetEvidenceMutationError::UnsupportedFormat => std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "projection reset evidence format is unsupported",
        ),
        ProjectionResetEvidenceMutationError::Unavailable => {
            std::io::Error::other("projection reset evidence storage is unavailable")
        }
    })
}

#[cfg(target_os = "linux")]
fn unlink_projection_reset_marker_file_recoverable(
    vault_root: &Path,
    path: &Path,
    expected: ArtifactIdentity,
    reset_fault_lease: Option<&PersonalVaultLease>,
) -> Result<(), ProjectionResetEvidenceMutationError> {
    if path.parent() != Some(vault_root) {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    let basename = path.file_name().ok_or(ProjectionResetEvidenceMutationError::Manual)?;
    let vault = open(
        vault_root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    let current = statat(&vault, basename, AtFlags::SYMLINK_NOFOLLOW)
        .map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    if expected.device != current.st_dev as u64
        || expected.inode != current.st_ino as u64
        || expected.mode != current.st_mode
        || expected.length != current.st_size as u64
        || FileType::from_raw_mode(current.st_mode) != FileType::RegularFile
    {
        return Err(ProjectionResetEvidenceMutationError::Manual);
    }
    unlinkat(&vault, basename, AtFlags::empty()).map_err(|_| ProjectionResetEvidenceMutationError::Unavailable)?;
    #[cfg(test)]
    if reset_fault_lease.is_some_and(|lease| take_projection_reset_fault(lease, RESET_FAULT_AFTER_ACTIVE_MARKER_UNLINK))
    {
        return Err(ProjectionResetEvidenceMutationError::Indeterminate);
    }
    #[cfg(not(test))]
    let _ = reset_fault_lease;
    fsync(&vault).map_err(|_| ProjectionResetEvidenceMutationError::Indeterminate)
}

#[cfg(not(target_os = "linux"))]
fn unlink_projection_reset_marker_file(
    _vault_root: &Path,
    _path: &Path,
    _expected: ArtifactIdentity,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection reset marker cleanup requires Linux fd-relative unlink",
    ))
}

fn validate_projection_reset_marker(vault_root: &Path, expected: &ProjectionResetMarker) -> std::io::Result<()> {
    let path = vault_root.join(PROJECTION_RESET_MARKER_FILE);
    let bytes = read_private_file_bounded(&path, 4096)?;
    let actual: ProjectionResetMarker =
        serde_json::from_slice(&bytes).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let canonical = serde_json_canonicalizer::to_vec(&actual)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let payload_bytes = serde_json_canonicalizer::to_vec(&actual.payload)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if bytes != canonical
        || actual != *expected
        || actual.payload.schema != PROJECTION_RESET_MARKER_SCHEMA
        || actual.marker_hash != domain_hash(PROJECTION_RESET_MARKER_DOMAIN, &payload_bytes)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset marker verification failed",
        ));
    }
    Ok(())
}

fn reset_marker_entry_identity(identity: ProjectionPairEntryIdentity) -> ProjectionResetMarkerEntryIdentity {
    ProjectionResetMarkerEntryIdentity {
        device: identity.device.to_string(),
        inode: identity.inode.to_string(),
        mode: identity.mode,
        kind: identity.kind,
    }
}

fn reset_marker_directory_identity(identity: ProjectionPairDirectoryIdentity) -> ProjectionResetMarkerEntryIdentity {
    ProjectionResetMarkerEntryIdentity {
        device: identity.device.to_string(),
        inode: identity.inode.to_string(),
        mode: 0o700,
        kind: 1,
    }
}

fn marker_directory_identity(identity: &ProjectionResetMarkerEntryIdentity) -> Option<ProjectionPairDirectoryIdentity> {
    if identity.mode != 0o700 || identity.kind != 1 {
        return None;
    }
    Some(ProjectionPairDirectoryIdentity {
        device: identity.device.parse().ok()?,
        inode: identity.inode.parse().ok()?,
    })
}

fn marker_entry_identity(
    identity: &ProjectionResetMarkerEntryIdentity,
) -> std::io::Result<ProjectionPairEntryIdentity> {
    Ok(ProjectionPairEntryIdentity {
        device: identity.device.parse().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid projection reset device identity",
            )
        })?,
        inode: identity.inode.parse().map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid projection reset inode identity",
            )
        })?,
        mode: identity.mode,
        kind: identity.kind,
    })
}

fn directory_entry_identity(identity: ProjectionPairDirectoryIdentity) -> ProjectionPairEntryIdentity {
    ProjectionPairEntryIdentity {
        device: identity.device,
        inode: identity.inode,
        mode: 0o700,
        kind: 1,
    }
}

#[cfg(unix)]
fn projection_pair_entry_identity(metadata: &std::fs::Metadata) -> ProjectionPairEntryIdentity {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    let file_type = metadata.file_type();
    let kind = if file_type.is_dir() {
        1
    } else if file_type.is_file() {
        2
    } else if file_type.is_symlink() {
        3
    } else if file_type.is_fifo() {
        4
    } else if file_type.is_socket() {
        5
    } else if file_type.is_block_device() {
        6
    } else if file_type.is_char_device() {
        7
    } else {
        255
    };
    ProjectionPairEntryIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.permissions().mode() & 0o777,
        kind,
    }
}

#[cfg(not(unix))]
fn projection_pair_entry_identity(metadata: &std::fs::Metadata) -> ProjectionPairEntryIdentity {
    ProjectionPairEntryIdentity {
        device: 0,
        inode: 0,
        mode: 0,
        kind: if metadata.is_dir() { 1 } else { 2 },
    }
}

#[cfg(unix)]
fn open_private_directory_identity(path: &Path) -> std::io::Result<(File, ProjectionPairDirectoryIdentity)> {
    use std::os::unix::fs::MetadataExt;

    let file: File = open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
    .into();
    let metadata = file.metadata()?;
    require_private_directory(path)?;
    Ok((
        file,
        ProjectionPairDirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
    ))
}

#[cfg(unix)]
fn validate_private_directory_identity(
    file: &File,
    path: &Path,
    expected: ProjectionPairDirectoryIdentity,
) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    require_private_directory(path)?;
    let opened = file.metadata()?;
    let current = std::fs::symlink_metadata(path)?;
    if !current.file_type().is_dir()
        || current.file_type().is_symlink()
        || opened.dev() != expected.device
        || opened.ino() != expected.inode
        || current.dev() != expected.device
        || current.ino() != expected.inode
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection staging directory identity changed",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn open_private_directory_identity(_path: &Path) -> std::io::Result<(File, ProjectionPairDirectoryIdentity)> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection pair publication requires directory identity support",
    ))
}

#[cfg(not(unix))]
fn validate_private_directory_identity(
    _file: &File,
    _path: &Path,
    _expected: ProjectionPairDirectoryIdentity,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "projection pair publication requires directory identity support",
    ))
}

#[cfg(unix)]
fn restrict_new_staging_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    require_private_directory(path)
}

#[cfg(not(unix))]
fn restrict_new_staging_directory(path: &Path) -> std::io::Result<()> {
    require_private_directory(path)
}

fn verify_projection_pair_target(
    target: &ProjectionPairTarget,
    expected: &ProjectionPairSeal,
) -> Result<(), LedgerStorageError> {
    if expected.payload.schema != PROJECTION_PAIR_SEAL_SCHEMA || expected.payload.artifact_count != 0 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid projection pair seal").into());
    }
    expected.payload.genesis.validate()?;
    let seal_path = target
        .container
        .as_ref()
        .ok_or_else(|| std::io::Error::other("projection target lost its staging container"))?
        .path()
        .join(PROJECTION_PAIR_SEAL_FILE);
    let persisted_bytes = read_private_file_bounded(&seal_path, 4096)?;
    let persisted: ProjectionPairSeal = serde_json::from_slice(&persisted_bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let canonical = serde_json_canonicalizer::to_vec(&persisted)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let payload_bytes = serde_json_canonicalizer::to_vec(&persisted.payload)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if persisted_bytes != canonical
        || persisted != *expected
        || persisted.seal_hash != domain_hash(PROJECTION_PAIR_SEAL_DOMAIN, &payload_bytes)
        || projection_pair_tree_digest(&target.pair_path, Some(&expected.payload.genesis))?
            != expected.payload.tree_digest
        || active_pointer_digest(&target.pair_path)? != expected.payload.active_pointer_digest
        || !target
            .storage()
            .artifacts
            .list_immutable_hashes(8 * 1024 * 1024)?
            .is_empty()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection pair seal verification failed",
        )
        .into());
    }
    Ok(())
}

fn active_pointer_digest(pair_path: &Path) -> std::io::Result<String> {
    let bytes = read_private_file_bounded(
        &pair_path
            .join(PROJECTION_MANIFEST_DIRECTORY)
            .join("roots")
            .join("active"),
        4096,
    )?;
    if bytes.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection target has no active genesis pointer",
        ));
    }
    Ok(domain_hash(PROJECTION_PAIR_ACTIVE_DOMAIN, &bytes))
}

fn projection_reset_live_fingerprint(pair_path: &Path) -> std::io::Result<ProjectionResetLiveFingerprint> {
    use walkdir::WalkDir;

    #[cfg(unix)]
    use std::os::unix::ffi::OsStrExt;

    const MAX_DEPTH: usize = 8;
    const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
    const MAX_TOTAL_BYTES: u64 = MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES as u64 * MAX_FILE_BYTES;

    let mut rows = Vec::new();
    let mut total_bytes = 0_u64;
    let root_device = projection_pair_entry_identity(&std::fs::symlink_metadata(pair_path)?).device;
    let mut storage_layout_invalid = false;
    for entry in WalkDir::new(pair_path).follow_links(false).same_file_system(true) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.depth() > MAX_DEPTH {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset live tree exceeds maximum depth",
            ));
        }
        if rows.len() >= MAX_PROJECTION_MAINTENANCE_INVENTORY_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset live tree exceeds inventory limit",
            ));
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.len() > MAX_JCS_SAFE_INTEGER {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection reset live entry exceeds JCS safe integer range",
            ));
        }
        let relative_path = path
            .strip_prefix(pair_path)
            .map_err(|_| std::io::Error::other("projection reset live path escaped root"))?
            .to_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 projection path"))?
            .to_string();
        let file_type = metadata.file_type();
        let kind = projection_entry_kind(&file_type);
        let identity = projection_pair_entry_identity(&metadata);
        let mode = identity.mode;
        if identity.device != root_device || (file_type.is_dir() && mode != 0o700) {
            storage_layout_invalid = true;
        }
        let content_evidence = if file_type.is_file() {
            if mode != 0o600 {
                storage_layout_invalid = true;
                Some(ProjectionResetContentEvidence::NonPrivateMode)
            } else if metadata.len() > MAX_FILE_BYTES {
                storage_layout_invalid = true;
                Some(ProjectionResetContentEvidence::Oversize)
            } else {
                total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "projection reset live byte count overflow",
                    )
                })?;
                if total_bytes > MAX_TOTAL_BYTES {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "projection reset live tree exceeds aggregate byte limit",
                    ));
                }
                Some(ProjectionResetContentEvidence::Digest {
                    sha256: domain_hash(
                        b"plico.projection.reset-live-file.v1\0",
                        &read_reset_regular_file_bounded(path, MAX_FILE_BYTES)?,
                    ),
                })
            }
        } else if file_type.is_symlink() {
            storage_layout_invalid = true;
            let target = std::fs::read_link(path)?;
            #[cfg(unix)]
            let target_bytes = target.as_os_str().as_bytes();
            #[cfg(not(unix))]
            let target_bytes = target
                .to_str()
                .ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 projection symlink target")
                })?
                .as_bytes();
            Some(ProjectionResetContentEvidence::SymlinkTarget {
                sha256: domain_hash(b"plico.projection.reset-live-link.v1\0", target_bytes),
            })
        } else if file_type.is_dir() {
            None
        } else {
            storage_layout_invalid = true;
            Some(ProjectionResetContentEvidence::Special)
        };
        rows.push(ProjectionResetLiveRow {
            relative_path,
            kind,
            mode,
            size: metadata.len(),
            device: identity.device.to_string(),
            inode: identity.inode.to_string(),
            content_evidence,
        });
    }
    rows.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let bytes = serde_json_canonicalizer::to_vec(&rows)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(ProjectionResetLiveFingerprint {
        digest: domain_hash(PROJECTION_RESET_LIVE_DOMAIN, &bytes),
        storage_layout_invalid,
    })
}

fn projection_entry_kind(file_type: &std::fs::FileType) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        if file_type.is_dir() {
            "directory"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_fifo() {
            "fifo"
        } else if file_type.is_socket() {
            "socket"
        } else if file_type.is_block_device() {
            "block_device"
        } else if file_type.is_char_device() {
            "char_device"
        } else {
            "unknown"
        }
    }
    #[cfg(not(unix))]
    {
        if file_type.is_dir() {
            "directory"
        } else if file_type.is_file() {
            "file"
        } else if file_type.is_symlink() {
            "symlink"
        } else {
            "unknown"
        }
    }
}

fn read_reset_regular_file_bounded(path: &Path, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
    let path_metadata = std::fs::symlink_metadata(path)?;
    if path_metadata.file_type().is_symlink()
        || !path_metadata.file_type().is_file()
        || path_metadata.len() > maximum_bytes
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset live entry is not a bounded regular file",
        ));
    }
    let mut file: File = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
    .into();
    let opened = file.metadata()?;
    if projection_pair_entry_identity(&opened) != projection_pair_entry_identity(&path_metadata) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset live entry identity changed while opening",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum_bytes
        || projection_pair_entry_identity(&std::fs::symlink_metadata(path)?) != projection_pair_entry_identity(&opened)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "projection reset live entry changed while reading",
        ));
    }
    Ok(bytes)
}

fn projection_pair_tree_digest(
    pair_path: &Path,
    clean_genesis: Option<&ProjectionPairGenesisEvidence>,
) -> std::io::Result<String> {
    use walkdir::WalkDir;

    require_private_directory(pair_path).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "projection pair tree root is not private",
        )
    })?;
    let mut rows = Vec::new();
    for entry in WalkDir::new(pair_path).follow_links(false).same_file_system(true) {
        let entry = entry.map_err(std::io::Error::other)?;
        if entry.depth() > MAX_PROJECTION_PAIR_TREE_DEPTH {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair exceeds maximum tree depth",
            ));
        }
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.len() > MAX_JCS_SAFE_INTEGER {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair entry exceeds JCS safe integer range",
            ));
        }
        let entry_type = entry.file_type();
        if entry_type.is_symlink()
            || metadata.file_type().is_symlink()
            || entry_type.is_dir() != metadata.file_type().is_dir()
            || entry_type.is_file() != metadata.file_type().is_file()
            || !(entry_type.is_dir() || entry_type.is_file())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair contains an unsupported file type",
            ));
        }
        let relative_path = path
            .strip_prefix(pair_path)
            .map_err(|_| std::io::Error::other("projection pair path escaped root"))?
            .to_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 projection path"))?
            .to_string();
        let content_digest = if entry_type.is_file() {
            Some(format!(
                "{:x}",
                Sha256::digest(read_private_file_bounded(path, 32 * 1024 * 1024)?)
            ))
        } else {
            None
        };
        let mode = projection_mode(&metadata);
        if (entry_type.is_dir() && mode != 0o700) || (entry_type.is_file() && mode != 0o600) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "projection pair contains a non-private entry",
            ));
        }
        rows.push(ProjectionPairTreeRow {
            relative_path,
            kind: if entry_type.is_dir() { "directory" } else { "file" },
            mode,
            size: metadata.len(),
            content_digest,
        });
        if rows.len() > MAX_PROJECTION_PAIR_TREE_ENTRIES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair exceeds bounded tree inventory",
            ));
        }
    }
    rows.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    for required in [
        "manifest",
        "manifest/objects",
        "manifest/roots",
        "artifacts",
        "artifacts/objects",
    ] {
        if !rows
            .iter()
            .any(|row| row.relative_path == required && row.kind == "directory")
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection pair is missing required storage directories",
            ));
        }
    }
    if let Some(genesis) = clean_genesis {
        let expected_directories = [
            "",
            "manifest",
            "manifest/objects",
            "manifest/roots",
            "artifacts",
            "artifacts/objects",
        ];
        let actual_directories: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == "directory")
            .map(|row| row.relative_path.as_str())
            .collect();
        if actual_directories.len() != expected_directories.len()
            || expected_directories
                .iter()
                .any(|expected| !actual_directories.contains(expected))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "clean projection genesis directory inventory is not exact",
            ));
        }
        let expected_files = [
            "manifest/roots/active".to_string(),
            "manifest/roots/candidate".to_string(),
            format!("manifest/objects/{}", genesis.projection_root_hash),
            format!("manifest/objects/{}", genesis.current_view_hash),
        ];
        let actual_files: Vec<_> = rows
            .iter()
            .filter(|row| row.kind == "file")
            .map(|row| row.relative_path.clone())
            .collect();
        if actual_files.len() != expected_files.len()
            || expected_files.iter().any(|expected| !actual_files.contains(expected))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "clean projection genesis inventory is not exact",
            ));
        }
    }
    let bytes = serde_json_canonicalizer::to_vec(&rows)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(domain_hash(PROJECTION_PAIR_TREE_DOMAIN, &bytes))
}

fn domain_hash(domain: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(domain);
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
fn replace_private_test_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("test protocol file has no parent"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(unix)]
fn projection_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o777
}

#[cfg(not(unix))]
fn projection_mode(_metadata: &std::fs::Metadata) -> u32 {
    0
}

#[cfg(unix)]
fn require_private_directory(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir()
        && !metadata.file_type().is_symlink()
        && metadata.permissions().mode() & 0o777 == 0o700
    {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "projection artifact directory is not private",
        ))
    }
}

#[cfg(not(unix))]
fn require_private_directory(path: &std::path::Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "projection artifact directory is not private",
        ))
    }
}

impl ProjectionArtifactStorage {
    pub(crate) fn put_immutable(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        put_immutable_at(&self.objects_directory, hash, bytes)?;
        #[cfg(test)]
        if self
            .fail_after_durable_put_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            tracing::debug!(
                operation = "projection.artifact_put",
                phase = "artifact_persist",
                result_category = "durable_orphan",
            );
            return Err(std::io::Error::other("injected post-artifact durability failure"));
        }
        Ok(())
    }

    pub(crate) fn get_immutable_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        validate_hash(hash)?;
        read_private_file_bounded(&self.objects_directory.join(hash), maximum_bytes)
    }

    pub(crate) fn list_immutable_hashes(&self, maximum_bytes: u64) -> std::io::Result<Vec<String>> {
        self.list_immutable_hashes_bounded(maximum_bytes, usize::MAX)
    }

    fn list_immutable_hashes_bounded(
        &self,
        maximum_bytes: u64,
        maximum_entries: usize,
    ) -> std::io::Result<Vec<String>> {
        let mut hashes = Vec::new();
        for entry in std::fs::read_dir(&self.objects_directory)? {
            if hashes.len() >= maximum_entries {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection artifact inventory exceeds entry limit",
                ));
            }
            let entry = entry?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 artifact hash"))?;
            validate_hash(&hash)?;
            if !entry.file_type()?.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection artifact is not a regular file",
                ));
            }
            read_private_file_bounded(&self.objects_directory.join(&hash), maximum_bytes)?;
            hashes.push(hash);
        }
        hashes.sort();
        Ok(hashes)
    }

    #[cfg(unix)]
    fn list_cleanup_names_bounded(&self, maximum_entries: usize) -> std::io::Result<Vec<String>> {
        let mut hashes = Vec::new();
        for entry in std::fs::read_dir(&self.objects_directory)? {
            if hashes.len() >= maximum_entries {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection artifact cleanup inventory exceeds entry limit",
                ));
            }
            let entry = entry?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 artifact hash"))?;
            validate_hash(&hash)?;
            if std::fs::symlink_metadata(entry.path())?.file_type().is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection artifact cleanup refuses directory entries",
                ));
            }
            hashes.push(hash);
        }
        hashes.sort();
        Ok(hashes)
    }

    #[cfg(unix)]
    fn cleanup_all_bounded(&self, maximum_bytes: u64, maximum_entries: usize) -> std::io::Result<usize> {
        let hashes = self.list_cleanup_names_bounded(maximum_entries)?;
        let mut removed = 0usize;
        for hash in hashes {
            let snapshot = self.snapshot_for_cleanup(&hash, maximum_bytes)?;
            self.remove_snapshot_entry(snapshot)?;
            removed = removed
                .checked_add(1)
                .ok_or_else(|| std::io::Error::other("projection artifact cleanup count overflow"))?;
        }
        // This fsync is unconditional. A prior cleanup attempt may have
        // unlinked the final orphan and then failed before proving directory
        // durability; the retry must establish that durability before the
        // genesis pointer can advance.
        #[cfg(test)]
        if self
            ._lease
            .fail_projection_artifact_cleanup_sync_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(std::io::Error::other(
                "injected projection artifact cleanup directory sync failure",
            ));
        }
        sync_directory(&self.objects_directory)?;
        Ok(removed)
    }

    pub(crate) fn validate_inventory(
        &self,
        referenced_hashes: &HashSet<String>,
        maximum_bytes: u64,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(&self.objects_directory)? {
            let entry = entry?;
            let hash = entry
                .file_name()
                .into_string()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "non-UTF8 artifact hash"))?;
            validate_hash(&hash)?;
            let path = self.objects_directory.join(&hash);
            if entry.file_type()?.is_file() && read_private_file_bounded(&path, maximum_bytes).is_ok() {
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

    #[cfg(unix)]
    pub(crate) fn snapshot_for_cleanup(
        &self,
        hash: &str,
        maximum_bytes: u64,
    ) -> std::io::Result<ProjectionArtifactSnapshot> {
        validate_hash(hash)?;
        let path = self.objects_directory.join(hash);
        let path_metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ProjectionArtifactSnapshot {
                    hash: hash.to_string(),
                    identity: None,
                    file: None,
                    bytes: None,
                });
            }
            Err(error) => return Err(error),
        };
        let path_identity = artifact_identity(&path_metadata);
        if !path_metadata.file_type().is_file() {
            return Ok(ProjectionArtifactSnapshot {
                hash: hash.to_string(),
                identity: Some(path_identity),
                file: None,
                bytes: None,
            });
        }
        let mut file: File = open(
            &path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?
        .into();
        let opened_identity = artifact_identity(&file.metadata()?);
        if opened_identity != path_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection artifact identity changed while opening",
            ));
        }
        let bytes = if opened_identity.length <= maximum_bytes {
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(maximum_bytes.saturating_add(1))
                .read_to_end(&mut bytes)?;
            (bytes.len() as u64 <= maximum_bytes).then_some(bytes)
        } else {
            None
        };
        if artifact_identity(&std::fs::symlink_metadata(&path)?) != opened_identity {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection artifact identity changed while reading",
            ));
        }
        Ok(ProjectionArtifactSnapshot {
            hash: hash.to_string(),
            identity: Some(opened_identity),
            file: Some(file),
            bytes,
        })
    }

    #[cfg(unix)]
    pub(crate) fn remove_snapshot(&self, snapshot: ProjectionArtifactSnapshot) -> std::io::Result<()> {
        #[cfg(test)]
        if self.fail_cleanup_once.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::other("injected projection artifact cleanup failure"));
        }
        self.remove_snapshot_entry(snapshot)?;
        sync_directory(&self.objects_directory)
    }

    #[cfg(unix)]
    fn remove_snapshot_entry(&self, mut snapshot: ProjectionArtifactSnapshot) -> std::io::Result<()> {
        let Some(expected_identity) = snapshot.identity else {
            return Ok(());
        };
        let path = self.objects_directory.join(&snapshot.hash);
        let current_metadata = std::fs::symlink_metadata(&path)?;
        let current_identity = artifact_identity(&current_metadata);
        if current_identity != expected_identity || current_metadata.file_type().is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "projection artifact changed before cleanup",
            ));
        }
        if let (Some(file), Some(observed_bytes)) = (snapshot.file.as_mut(), snapshot.bytes.as_ref()) {
            file.rewind()?;
            let mut current_bytes = Vec::new();
            Read::by_ref(file)
                .take(observed_bytes.len() as u64 + 1)
                .read_to_end(&mut current_bytes)?;
            if current_bytes != *observed_bytes {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "projection artifact content changed before cleanup",
                ));
            }
        }
        std::fs::remove_file(path)
    }

    pub(crate) fn flush(&self) -> std::io::Result<()> {
        sync_directory(&self.objects_directory)?;
        sync_directory(&self.artifact_directory)
    }

    #[cfg(test)]
    pub(crate) fn inject_post_artifact_durability_failure_once(&self) {
        self.fail_after_durable_put_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_cleanup_failure_once(&self) {
        self.fail_cleanup_once.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_remove_for_test(&self, hash: &str) -> std::io::Result<()> {
        validate_hash(hash)?;
        std::fs::remove_file(self.objects_directory.join(hash))?;
        sync_directory(&self.objects_directory)
    }

    #[cfg(test)]
    pub(crate) fn inject_replace_for_test(&self, hash: &str, bytes: &[u8]) -> std::io::Result<()> {
        validate_hash(hash)?;
        std::fs::remove_file(self.objects_directory.join(hash))?;
        put_immutable_at(&self.objects_directory, hash, bytes)
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_permissive_mode_for_test(&self, hash: &str) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        validate_hash(hash)?;
        std::fs::set_permissions(
            self.objects_directory.join(hash),
            std::fs::Permissions::from_mode(0o644),
        )
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_symlink_for_test(&self, hash: &str) -> std::io::Result<()> {
        use std::os::unix::fs::symlink;

        validate_hash(hash)?;
        std::fs::remove_file(self.objects_directory.join(hash))?;
        symlink("missing-projection-artifact-target", self.objects_directory.join(hash))
    }

    #[cfg(all(test, unix))]
    pub(crate) fn inject_fifo_for_test(&self, hash: &str) -> std::io::Result<()> {
        validate_hash(hash)?;
        std::fs::remove_file(self.objects_directory.join(hash))?;
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            self.objects_directory.join(hash),
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
    }
}

#[cfg(unix)]
fn artifact_identity(metadata: &std::fs::Metadata) -> ArtifactIdentity {
    use std::os::unix::fs::MetadataExt;

    ArtifactIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        length: metadata.len(),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn fifo_bounded_read_child() {
        if std::env::var_os("PLICO_FIFO_BOUNDED_CHILD").is_none() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let fifo = directory.path().join("fifo");
        rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, Mode::RUSR | Mode::WUSR).unwrap();
        assert!(crate::cas::ledger_store::read_private_file_bounded(&fifo, 8).is_err());
    }

    #[test]
    #[ignore = "process deadline gate; run explicitly with one test thread"]
    fn fifo_bounded_read_has_a_process_deadline() {
        let executable = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(executable)
            .arg("--exact")
            .arg("cas::projection_store::tests::fifo_bounded_read_child")
            .env("PLICO_FIFO_BOUNDED_CHILD", "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("bounded FIFO read exceeded its process deadline");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
