use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{de::DeserializeOwned, Serialize};

use super::current_view::rebuild_current_view;
use super::hash::{
    artifact_bytes_and_hash, parse_canonical_with_schema, pointer_bytes, require_schema, root_bytes_and_hash,
    segment_bytes_and_hash, verify_hash, view_bytes_and_hash, HashKind,
};
use super::model::{
    AbsentReason, CanonicalWatermark, EmbeddingArtifact, ManifestEvent, ManifestRecord, ProjectionCurrentView,
    ProjectionError, ProjectionRoot, ProjectionSegment, ProjectionState, QueueReason, RootPointer, BUILDER_SPEC_SCHEMA,
    CURRENT_VIEW_SCHEMA, EMBEDDING_ARTIFACT_SCHEMA, MANIFEST_RECORD_SCHEMA, MANIFEST_ROOT_SCHEMA,
    MANIFEST_SEGMENT_SCHEMA, MAX_EMBEDDING_ARTIFACT_BYTES, MAX_PROJECTION_MANIFEST_OBJECT_BYTES,
    MAX_PROJECTION_POINTER_BYTES, ROOT_POINTER_SCHEMA,
};
use super::validate::{
    invalid, validate_artifact, validate_embedding_output, validate_reconciled_coverage,
    validate_source_against_canonical,
};
use crate::cas::projection_store::{ProjectionPairPublication, ProjectionResetMaintenanceError};
use crate::cas::{
    ExistingProjectionReadOnly, LedgerStorageError, PersonalVaultStorage, ProjectionClaimedLiveInspection,
    ProjectionPairGenesisEvidence, ProjectionPairPublishMode, ProjectionPairResetReason, ProjectionStorageBundle,
};
use crate::memory::CanonicalProjectionSnapshot;
use crate::memory::{MemoryContent, MemoryTier};

trait ProjectionManifestReader {
    fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>>;
    fn read_object_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>>;
}

impl ProjectionManifestReader for ProjectionStorageBundle {
    fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        self.read_manifest_active_bounded(maximum_bytes)
    }

    fn read_object_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.read_manifest_object_bounded(hash, maximum_bytes)
    }
}

impl ProjectionManifestReader for ExistingProjectionReadOnly<'_> {
    fn read_active_bounded(&self, maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>> {
        self.read_manifest_active_bounded(maximum_bytes)
    }

    fn read_object_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        self.read_manifest_object_bounded(hash, maximum_bytes)
    }
}

trait ProjectionArtifactReader {
    fn read_artifact_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>>;
    fn validate_inventory(&self, referenced: &HashSet<String>, maximum_bytes: u64) -> std::io::Result<()>;
}

trait ProjectionStoredObject: DeserializeOwned + Serialize {
    const SCHEMA: &'static str;
    const COMPONENT: &'static str;

    fn validate_nested_schemas(_value: &serde_json::Value) -> Result<(), ProjectionError> {
        Ok(())
    }
}

impl ProjectionStoredObject for ProjectionRoot {
    const SCHEMA: &'static str = MANIFEST_ROOT_SCHEMA;
    const COMPONENT: &'static str = "manifest_root";
}

impl ProjectionStoredObject for ProjectionSegment {
    const SCHEMA: &'static str = MANIFEST_SEGMENT_SCHEMA;
    const COMPONENT: &'static str = "manifest_segment";

    fn validate_nested_schemas(value: &serde_json::Value) -> Result<(), ProjectionError> {
        validate_segment_schema_tree(value)
    }
}

impl ProjectionStoredObject for ProjectionCurrentView {
    const SCHEMA: &'static str = CURRENT_VIEW_SCHEMA;
    const COMPONENT: &'static str = "current_view";

    fn validate_nested_schemas(value: &serde_json::Value) -> Result<(), ProjectionError> {
        validate_current_view_schema_tree(value)
    }
}

impl ProjectionArtifactReader for ProjectionStorageBundle {
    fn read_artifact_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        ProjectionStorageBundle::read_artifact_bounded(self, hash, maximum_bytes)
    }

    fn validate_inventory(&self, referenced: &HashSet<String>, maximum_bytes: u64) -> std::io::Result<()> {
        self.validate_artifact_inventory(referenced, maximum_bytes)
    }
}

impl ProjectionArtifactReader for ExistingProjectionReadOnly<'_> {
    fn read_artifact_bounded(&self, hash: &str, maximum_bytes: u64) -> std::io::Result<Vec<u8>> {
        ExistingProjectionReadOnly::read_artifact_bounded(self, hash, maximum_bytes)
    }

    fn validate_inventory(&self, referenced: &HashSet<String>, maximum_bytes: u64) -> std::io::Result<()> {
        self.validate_artifact_inventory(referenced, maximum_bytes)
    }
}

struct ProjectionStoreState {
    poisoned: bool,
    root_hash: String,
    root: ProjectionRoot,
    view: ProjectionCurrentView,
    records: Vec<ManifestRecord>,
    genesis_source: CanonicalWatermark,
    artifact_issues: Vec<ArtifactIssue>,
}

#[derive(Debug, Clone)]
struct ArtifactIssue {
    projection_id: uuid::Uuid,
    reason: super::model::StaleReason,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(super) enum ProjectionUnknownSchemaTarget {
    RootPointer,
    CurrentRoot,
    HistoricalRoot,
    CurrentSegment,
    CurrentRecord,
    RecordBuilder,
    CurrentView,
    ViewBuilder,
    ArtifactDescriptor,
    Artifact,
}

pub(super) struct ProjectionOwnerRebuildReceipt {
    pub(super) manifest_generation: u64,
    pub(super) event_watermark: u64,
    pub(super) reconciled_source: CanonicalWatermark,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum ProjectionCommitActor {
    PersonalOwner,
    Worker,
}

impl ProjectionCommitActor {
    fn role_id(self) -> &'static str {
        match self {
            Self::PersonalOwner => crate::PERSONAL_OWNER_ROLE_ID,
            Self::Worker => "projection-worker",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectionStoreInspection {
    Absent,
    GenesisOnly,
    Valid { event_watermark: u64 },
    RepairRequired { count: usize },
    ResetRequired(ProjectionPairResetReason),
    UnsupportedFormat,
    Unavailable(ProjectionStoreUnavailable),
    ResetPending,
    MaintenanceRequired,
    ManualIntervention,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectionStoreUnavailable {
    VaultLocked,
    PermissionDenied,
    StorageIo,
    ResourceExhausted,
}

/// Durable claim token. It deliberately has no `Debug` or serialization
/// implementation so it cannot leak into traces, queues, or wire payloads.
pub(super) struct ProjectionBuildGuard {
    entry: super::model::ProjectionEntry,
    attempt: u32,
    attempt_id: uuid::Uuid,
    claim_event_watermark: u64,
    canonical_watermark: CanonicalWatermark,
}

impl ProjectionBuildGuard {
    pub(super) fn projection_id(&self) -> uuid::Uuid {
        self.entry.projection_id
    }
    pub(super) fn source(&self) -> &super::model::CanonicalSourceIdentity {
        &self.entry.source
    }

    pub(super) fn desired_builder_spec_hash(&self) -> &str {
        &self.entry.desired_builder_spec_hash
    }

    pub(super) fn attempt(&self) -> u32 {
        self.attempt
    }

    pub(super) fn claim_event_watermark(&self) -> u64 {
        self.claim_event_watermark
    }

    pub(super) fn canonical_watermark(&self) -> &CanonicalWatermark {
        &self.canonical_watermark
    }
}

/// Sole durable writer for the memory-embedding projection manifest.
pub(super) struct ProjectionManifestStore {
    _vault: Arc<PersonalVaultStorage>,
    storage: ProjectionStorageBundle,
    state: Mutex<ProjectionStoreState>,
    operation: Mutex<()>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    fail_before_pointer_once: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    cleanup_barriers: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
}

impl ProjectionManifestStore {
    pub(super) fn reset_operation_id(&self) -> Option<&str> {
        self.storage.reset_operation_id()
    }

    pub(super) fn inspect_existing_read_only(
        vault: &PersonalVaultStorage,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<ProjectionStoreInspection, ProjectionError> {
        let inspection = match vault.with_existing_projection_readonly(|existing| {
            let Some(reader) = existing else {
                return Ok(ProjectionStoreInspection::Absent);
            };
            inspect_projection_reader(&reader, canonical)
        }) {
            Ok(result) => result,
            Err(crate::cas::LedgerStorageOpenError::ProjectionResetPending) => {
                Ok(ProjectionStoreInspection::ResetPending)
            }
            Err(crate::cas::LedgerStorageOpenError::ProjectionResetMaintenanceRequired) => {
                Ok(ProjectionStoreInspection::MaintenanceRequired)
            }
            Err(crate::cas::LedgerStorageOpenError::ProjectionResetIndeterminate) => {
                Ok(ProjectionStoreInspection::ResetPending)
            }
            Err(crate::cas::LedgerStorageOpenError::ProjectionResetManualIntervention) => {
                Ok(ProjectionStoreInspection::ManualIntervention)
            }
            Err(crate::cas::LedgerStorageOpenError::NamespaceAlreadyClaimed) => Ok(
                ProjectionStoreInspection::Unavailable(ProjectionStoreUnavailable::VaultLocked),
            ),
            Err(crate::cas::LedgerStorageOpenError::RejectedMarker) => Ok(ProjectionStoreInspection::UnsupportedFormat),
            Err(crate::cas::LedgerStorageOpenError::UnsupportedProjectionFormat) => {
                Ok(ProjectionStoreInspection::UnsupportedFormat)
            }
            Err(crate::cas::LedgerStorageOpenError::Io(error)) => Ok(classify_projection_io(error, true)),
        };
        if let Ok(health) = &inspection {
            let result_category = match health {
                ProjectionStoreInspection::Absent => "absent",
                ProjectionStoreInspection::GenesisOnly => "genesis_only",
                ProjectionStoreInspection::Valid { .. } => "valid",
                ProjectionStoreInspection::RepairRequired { .. } => "repair_required",
                ProjectionStoreInspection::ResetRequired(_) => "reset_required",
                ProjectionStoreInspection::UnsupportedFormat => "unsupported_format",
                ProjectionStoreInspection::Unavailable(_) => "unavailable",
                ProjectionStoreInspection::ResetPending => "reset_pending",
                ProjectionStoreInspection::MaintenanceRequired => "maintenance_required",
                ProjectionStoreInspection::ManualIntervention => "manual_intervention",
            };
            tracing::debug!(
                operation = "projection_reset",
                phase = "inspection",
                outcome = "observed",
                result_category
            );
        }
        inspection
    }

    fn inspect_claimed_reset_live(
        target: &mut crate::cas::projection_store::ProjectionPairTarget,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<ProjectionStoreInspection, ProjectionError> {
        let inspected = target
            .inspect_and_bind_reset_live(|reader| {
                let inspection = inspect_projection_reader(&reader, canonical);
                let reason = inspection.as_ref().ok().and_then(|health| match health {
                    ProjectionStoreInspection::ResetRequired(reason) => Some(*reason),
                    _ => None,
                });
                (reason, inspection)
            })
            .map_err(map_open_error)?;
        match inspected {
            ProjectionClaimedLiveInspection::StorageLayoutInvalid => Ok(ProjectionStoreInspection::ResetRequired(
                ProjectionPairResetReason::StorageLayoutInvalid,
            )),
            ProjectionClaimedLiveInspection::Readable(result) => result,
        }
    }

    pub(super) fn bootstrap_new(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::bootstrap_new_with_clock_inner(vault, canonical, Arc::new(crate::util::now_ms))
    }

    fn bootstrap_new_with_clock_inner(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::publish_clean_genesis_target(vault, canonical, clock, ProjectionPairPublishMode::CreateAbsent, None)
    }

    pub(super) fn reset_required(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<(Arc<Self>, ProjectionPairResetReason), ProjectionError> {
        let expected_reason = match Self::inspect_existing_read_only(&vault, canonical)? {
            ProjectionStoreInspection::ResetRequired(reason) => reason,
            ProjectionStoreInspection::UnsupportedFormat => {
                return Err(ProjectionError::UnsupportedFormat {
                    component: "projection_store",
                });
            }
            ProjectionStoreInspection::ResetPending => return Err(ProjectionError::ResetPending),
            ProjectionStoreInspection::MaintenanceRequired => {
                return Err(ProjectionError::ProjectionMaintenanceRequired);
            }
            ProjectionStoreInspection::ManualIntervention => {
                return Err(ProjectionError::ManualInterventionRequired);
            }
            ProjectionStoreInspection::Unavailable(_) => {
                return Err(ProjectionError::Io(std::io::Error::other(
                    "projection reset source is unavailable",
                )));
            }
            _ => {
                return Err(ProjectionError::Invalid {
                    category: "projection_reset_not_required",
                });
            }
        };
        let store = Self::publish_clean_genesis_target(
            vault,
            canonical,
            Arc::new(crate::util::now_ms),
            ProjectionPairPublishMode::ReplaceExisting,
            Some(expected_reason),
        )?;
        Ok((store, expected_reason))
    }

    pub(super) fn recover_reset_maintenance(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        let maintenance = vault.recover_projection_reset_maintenance().map_err(map_open_error)?;
        let storage = maintenance.finish().map_err(|error| match error {
            ProjectionResetMaintenanceError::ManualIntervention => ProjectionError::ManualInterventionRequired,
            ProjectionResetMaintenanceError::CommitIndeterminate => ProjectionError::CommitIndeterminate,
            ProjectionResetMaintenanceError::UnsupportedFormat => ProjectionError::UnsupportedFormat {
                component: "projection_reset_protocol",
            },
            ProjectionResetMaintenanceError::Unavailable => ProjectionError::Io(std::io::Error::other(
                "projection reset protocol storage is unavailable",
            )),
        })?;
        let state = load_state(&storage, &storage, canonical)
            .and_then(|state| {
                validate_state_against_canonical(&state, canonical)?;
                require_canonical_genesis(&state, canonical)?;
                if !projection_state_is_genesis_only(&state) || !state.artifact_issues.is_empty() {
                    return Err(invalid("recovered_projection_target_is_not_clean_genesis"));
                }
                Ok(state)
            })
            .map_err(|_| ProjectionError::CommitIndeterminate)?;
        Ok(Arc::new(Self {
            _vault: vault,
            storage,
            state: Mutex::new(state),
            operation: Mutex::new(()),
            clock: Arc::new(crate::util::now_ms),
            #[cfg(test)]
            fail_before_pointer_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            cleanup_barriers: Mutex::new(None),
        }))
    }

    #[cfg(test)]
    pub(super) fn open_existing_and_repair(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::open_existing_and_repair_with_clock_inner(vault, canonical, Arc::new(crate::util::now_ms))
    }

    pub(super) fn open_existing_matching_builder(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        expected_builder_spec_hash: &str,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::inspect_existing_builder_match(&vault, canonical, expected_builder_spec_hash)?;
        let store = Self::open_existing_unrepaired_inner(vault, canonical, Arc::new(crate::util::now_ms))?;
        store.require_active_builder_hash(expected_builder_spec_hash)?;
        store.repair_invalid_artifacts(canonical)?;
        Ok(store)
    }

    pub(super) fn open_existing_initialized(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        let store = Self::open_existing_unrepaired_inner(vault, canonical, Arc::new(crate::util::now_ms))?;
        {
            let state = store.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            if state.view.active_builder_specs.is_empty() {
                return Err(invalid("projection_builder_change_requires_initialized"));
            }
        }
        store.repair_invalid_artifacts(canonical)?;
        Ok(store)
    }

    pub(super) fn open_existing_genesis_only(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        let store = Self::open_existing_unrepaired_inner(vault, canonical, Arc::new(crate::util::now_ms))?;
        store.cleanup_genesis_orphans(canonical)?;
        Ok(store)
    }

    pub(super) fn inspect_existing_builder_match(
        vault: &PersonalVaultStorage,
        canonical: &CanonicalProjectionSnapshot,
        expected_builder_spec_hash: &str,
    ) -> Result<(), ProjectionError> {
        vault
            .with_existing_projection_readonly(|existing| {
                let reader = existing.ok_or_else(|| invalid("projection_not_initialized"))?;
                let state = load_state(&reader, &reader, canonical)?;
                validate_state_against_canonical(&state, canonical)?;
                require_canonical_genesis(&state, canonical)?;
                require_builder_hash_in_state(&state, expected_builder_spec_hash)
            })
            .map_err(map_open_error)?
    }

    fn require_active_builder_hash(&self, expected_builder_spec_hash: &str) -> Result<(), ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        require_builder_hash_in_state(&state, expected_builder_spec_hash)
    }

    fn open_existing_unrepaired_inner(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Self>, ProjectionError> {
        let storage = vault.open_existing_projection_writer().map_err(map_open_error)?;
        let state = load_state(&storage, &storage, canonical)?;
        validate_state_against_canonical(&state, canonical)?;
        require_canonical_genesis(&state, canonical)?;
        Ok(Arc::new(Self {
            _vault: vault,
            storage,
            state: Mutex::new(state),
            operation: Mutex::new(()),
            clock,
            #[cfg(test)]
            fail_before_pointer_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            cleanup_barriers: Mutex::new(None),
        }))
    }

    fn publish_clean_genesis_target(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        publish_mode: ProjectionPairPublishMode,
        expected_reset_reason: Option<ProjectionPairResetReason>,
    ) -> Result<Arc<Self>, ProjectionError> {
        let mut target = vault
            .prepare_projection_pair_target(publish_mode)
            .map_err(map_open_error)?;
        match (publish_mode, expected_reset_reason) {
            (ProjectionPairPublishMode::CreateAbsent, None) => {}
            (ProjectionPairPublishMode::ReplaceExisting, Some(expected)) => {
                match Self::inspect_claimed_reset_live(&mut target, canonical)? {
                    ProjectionStoreInspection::ResetRequired(actual) if actual == expected => {}
                    ProjectionStoreInspection::UnsupportedFormat => {
                        return Err(ProjectionError::UnsupportedFormat {
                            component: "projection_store",
                        });
                    }
                    ProjectionStoreInspection::ResetPending => return Err(ProjectionError::ResetPending),
                    ProjectionStoreInspection::MaintenanceRequired => {
                        return Err(ProjectionError::ProjectionMaintenanceRequired);
                    }
                    ProjectionStoreInspection::ManualIntervention => {
                        return Err(ProjectionError::ManualInterventionRequired);
                    }
                    ProjectionStoreInspection::Unavailable(_) => {
                        return Err(ProjectionError::Io(std::io::Error::other(
                            "projection reset source became unavailable",
                        )));
                    }
                    _ => return Err(ProjectionError::HeadConflict),
                }
            }
            _ => {
                return Err(ProjectionError::Invalid {
                    category: "invalid_projection_pair_publish_mode",
                });
            }
        }
        let genesis_source = watermark(&canonical.genesis_root_hash, &canonical.genesis_root);
        ensure_genesis(target.storage(), &genesis_source).map_err(|error| match error {
            ProjectionError::CommitIndeterminate => ProjectionError::Io(std::io::Error::other(
                "staging projection genesis publication failed before live cutover",
            )),
            other => other,
        })?;
        let staged = load_state(target.storage(), target.storage(), canonical)?;
        validate_state_against_canonical(&staged, canonical)?;
        require_canonical_genesis(&staged, canonical)?;
        if !projection_state_is_genesis_only(&staged) || !staged.artifact_issues.is_empty() {
            return Err(invalid("projection_target_is_not_clean_genesis"));
        }
        let evidence = ProjectionPairGenesisEvidence::new(
            staged.root_hash.clone(),
            staged.root.current_view_hash.clone(),
            staged.genesis_source.root_hash.clone(),
            staged.genesis_source.generation,
            staged.genesis_source.revision_watermark,
            staged.genesis_source.policy_watermark,
            staged.genesis_source.relation_watermark,
        )
        .map_err(map_pair_publish_error)?;
        target.seal_clean_genesis(evidence).map_err(map_pair_publish_error)?;
        let publication = vault
            .publish_projection_pair_target(target)
            .map_err(map_pair_publish_error)?;
        let storage = match publication {
            ProjectionPairPublication::ReadyStorage(storage) => {
                if publish_mode != ProjectionPairPublishMode::CreateAbsent {
                    return Err(ProjectionError::CommitIndeterminate);
                }
                *storage
            }
            ProjectionPairPublication::ResetMaintenance(maintenance) => {
                if publish_mode != ProjectionPairPublishMode::ReplaceExisting {
                    return Err(ProjectionError::CommitIndeterminate);
                }
                maintenance.finish().map_err(|error| match error {
                    ProjectionResetMaintenanceError::ManualIntervention => ProjectionError::ManualInterventionRequired,
                    ProjectionResetMaintenanceError::CommitIndeterminate => ProjectionError::CommitIndeterminate,
                    ProjectionResetMaintenanceError::UnsupportedFormat => ProjectionError::UnsupportedFormat {
                        component: "projection_reset_protocol",
                    },
                    ProjectionResetMaintenanceError::Unavailable => ProjectionError::Io(std::io::Error::other(
                        "projection reset protocol storage is unavailable",
                    )),
                })?
            }
        };
        let state = load_state(&storage, &storage, canonical)
            .and_then(|state| {
                validate_state_against_canonical(&state, canonical)?;
                require_canonical_genesis(&state, canonical)?;
                if !projection_state_is_genesis_only(&state) || !state.artifact_issues.is_empty() {
                    return Err(invalid("published_projection_target_is_not_clean_genesis"));
                }
                Ok(state)
            })
            .map_err(|_| ProjectionError::CommitIndeterminate)?;
        Ok(Arc::new(Self {
            _vault: vault,
            storage,
            state: Mutex::new(state),
            operation: Mutex::new(()),
            clock,
            #[cfg(test)]
            fail_before_pointer_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            cleanup_barriers: Mutex::new(None),
        }))
    }

    fn cleanup_genesis_orphans(&self, canonical: &CanonicalProjectionSnapshot) -> Result<(), ProjectionError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        let (root_hash, current_view_hash) = {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            if !projection_state_is_genesis_only(&state) || !state.artifact_issues.is_empty() {
                return Err(invalid("projection_resume_requires_clean_genesis_only"));
            }
            (state.root_hash.clone(), state.root.current_view_hash.clone())
        };
        let reachable = HashSet::from([root_hash.clone(), current_view_hash]);
        self.storage
            .cleanup_unreferenced_manifest_objects(&reachable)
            .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?;
        self.storage
            .cleanup_all_artifact_orphans(MAX_EMBEDDING_ARTIFACT_BYTES)
            .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?;
        if !self
            .storage
            .manifest_inventory_matches(&reachable)
            .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?
            || !self
                .storage
                .artifact_inventory_is_empty(MAX_EMBEDDING_ARTIFACT_BYTES)
                .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?
        {
            return Err(ProjectionError::ProjectionMaintenanceRequired);
        }
        let restored = load_state(&self.storage, &self.storage, canonical)
            .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?;
        validate_state_against_canonical(&restored, canonical)
            .map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?;
        require_canonical_genesis(&restored, canonical).map_err(|_| ProjectionError::ProjectionMaintenanceRequired)?;
        if !projection_state_is_genesis_only(&restored) || restored.root_hash != root_hash {
            return Err(ProjectionError::ProjectionMaintenanceRequired);
        }
        *self.state.lock().map_err(|_| invalid("projection_state_poisoned"))? = restored;
        Ok(())
    }

    #[cfg(test)]
    fn open_existing_and_repair_with_clock_inner(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Self>, ProjectionError> {
        let storage = vault.open_existing_projection_writer().map_err(map_open_error)?;
        let state = load_state(&storage, &storage, canonical)?;
        validate_state_against_canonical(&state, canonical)?;
        require_canonical_genesis(&state, canonical)?;
        let store = Arc::new(Self {
            _vault: vault,
            storage,
            state: Mutex::new(state),
            operation: Mutex::new(()),
            clock,
            #[cfg(test)]
            fail_before_pointer_once: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            cleanup_barriers: Mutex::new(None),
        });
        store.repair_invalid_artifacts(canonical)?;
        Ok(store)
    }

    #[cfg(test)]
    pub(super) fn bootstrap_new_with_clock(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::bootstrap_new_with_clock_inner(vault, canonical, clock)
    }

    #[cfg(test)]
    pub(super) fn open_existing_and_repair_with_clock_for_test(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::open_existing_and_repair_with_clock_inner(vault, canonical, clock)
    }

    #[cfg(test)]
    pub(super) fn open_existing_unrepaired_for_test(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Arc<Self>, ProjectionError> {
        Self::open_existing_unrepaired_inner(vault, canonical, Arc::new(crate::util::now_ms))
    }

    pub(super) fn current_view(&self) -> Result<ProjectionCurrentView, ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        if state.poisoned {
            return Err(ProjectionError::WriterPoisoned);
        }
        if !state.artifact_issues.is_empty() {
            return Err(ProjectionError::ArtifactRepairRequired {
                count: state.artifact_issues.len(),
            });
        }
        Ok(state.view.clone())
    }

    pub(super) fn status_view(&self) -> Result<ProjectionCurrentView, ProjectionError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        let view = self.current_view()?;
        let issues = inspect_ready_artifacts(&self.storage, &view)?;
        if issues.is_empty() {
            Ok(view)
        } else {
            Err(ProjectionError::ArtifactRepairRequired { count: issues.len() })
        }
    }

    #[cfg(test)]
    pub(super) fn current_view_for_test(&self) -> Result<ProjectionCurrentView, ProjectionError> {
        self.current_view()
    }

    #[cfg(test)]
    pub(super) fn root_chain_for_test(&self) -> Result<Vec<ProjectionRoot>, ProjectionError> {
        let mut root = {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            state.root.clone()
        };
        let mut roots = Vec::new();
        loop {
            roots.push(root.clone());
            let Some(previous) = root.previous_root_hash.clone() else {
                break;
            };
            root = read_object(&self.storage, &previous, HashKind::Root)?;
        }
        roots.reverse();
        Ok(roots)
    }

    pub(super) fn activate_builder(
        &self,
        builder_spec: super::model::BuilderSpec,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<String, ProjectionError> {
        let builder_spec_hash = super::hash::builder_spec_bytes_and_hash(&builder_spec)?.1;
        super::validate::validate_builder_hash(&builder_spec, &builder_spec_hash)?;
        let view = self.current_view()?;
        let root_hash = self.root_hash()?;
        let previous = view
            .active_builder_specs
            .iter()
            .find(|builder| builder.projection_kind == super::model::ProjectionKind::MemoryEmbedding);
        if previous.is_some_and(|active| active.builder_spec_hash == builder_spec_hash) {
            return Ok(root_hash);
        }
        let mut events = vec![ManifestEvent::BuilderActivated {
            projection_kind: super::model::ProjectionKind::MemoryEmbedding,
            builder_spec,
            builder_spec_hash: builder_spec_hash.clone(),
            previous_builder_spec_hash: previous.map(|active| active.builder_spec_hash.clone()),
        }];
        if previous.is_some() {
            for entry in &view.entries {
                let state = match &entry.state {
                    ProjectionState::AbsentByPolicy { .. } => continue,
                    ProjectionState::Ready { artifact, .. } => ProjectionState::Stale {
                        reason: super::model::StaleReason::BuilderSpecChanged,
                        artifact: artifact.clone(),
                    },
                    _ => ProjectionState::Queued {
                        reason: QueueReason::BuilderChanged,
                    },
                };
                events.push(transition_event(entry, builder_spec_hash.clone(), state)?);
            }
        }
        self.commit_expected(
            &root_hash,
            ProjectionCommitActor::PersonalOwner,
            events,
            Vec::new(),
            canonical,
        )
    }

    pub(super) fn reconcile(&self, canonical: &CanonicalProjectionSnapshot) -> Result<String, ProjectionError> {
        self.reconcile_with_receipt(canonical).map(|(root_hash, _)| root_hash)
    }

    pub(super) fn reconcile_with_receipt(
        &self,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<(String, ProjectionOwnerRebuildReceipt), ProjectionError> {
        let view = self.current_view()?;
        let root_hash = self.root_hash()?;
        let active_builder = view
            .active_builder_specs
            .iter()
            .find(|builder| builder.projection_kind == super::model::ProjectionKind::MemoryEmbedding)
            .ok_or_else(|| invalid("missing_active_memory_embedding_builder"))?;
        let now = (self.clock)();
        let mut events = Vec::new();
        for revision in &canonical.revisions {
            let source = canonical_source(revision);
            let projection_id = super::hash::projection_id(&revision.revision_id)?;
            let existing = view.entries.iter().find(|entry| entry.projection_id == projection_id);
            let absent = expected_absent_reason(revision, canonical.revisions.iter());
            let next = match (existing.map(|entry| &entry.state), absent) {
                (Some(ProjectionState::AbsentByPolicy { reason }), Some(expected)) if *reason == expected => None,
                (Some(ProjectionState::AbsentByPolicy { .. }), Some(expected)) | (Some(_), Some(expected)) => {
                    Some(ProjectionState::AbsentByPolicy { reason: expected })
                }
                (None, Some(expected)) => Some(ProjectionState::AbsentByPolicy { reason: expected }),
                (None, None) | (Some(ProjectionState::AbsentByPolicy { .. }), None) => Some(ProjectionState::Queued {
                    reason: QueueReason::Reconciliation,
                }),
                (Some(ProjectionState::Building { lease_expires_at, .. }), None) if *lease_expires_at <= now => {
                    Some(ProjectionState::Queued {
                        reason: QueueReason::LeaseExpired,
                    })
                }
                (
                    Some(ProjectionState::Failed {
                        retryable: true,
                        retry_not_before: Some(retry_at),
                        ..
                    }),
                    None,
                ) if *retry_at <= now => Some(ProjectionState::Queued {
                    reason: QueueReason::Retry,
                }),
                (Some(ProjectionState::Stale { .. }), None) => Some(ProjectionState::Queued {
                    reason: QueueReason::BuilderChanged,
                }),
                (Some(_), None) => None,
            };
            let Some(state) = next else {
                continue;
            };
            events.push(match existing {
                Some(entry) => transition_event(entry, active_builder.builder_spec_hash.clone(), state)?,
                None => ManifestEvent::ProjectionTransition {
                    projection_id,
                    projection_kind: super::model::ProjectionKind::MemoryEmbedding,
                    projection_version: 1,
                    previous_sequence: None,
                    source,
                    desired_builder_spec_hash: active_builder.builder_spec_hash.clone(),
                    state,
                },
            });
        }
        let target = watermark(&canonical.root_hash, &canonical.root);
        if view.reconciled_source != target {
            events.push(ManifestEvent::ReconciliationAdvanced {
                previous_source: view.reconciled_source.clone(),
                reconciled_source: target.clone(),
                classified_revision_count: canonical.root.revision_watermark,
            });
        }
        if events.is_empty() {
            return Ok((
                root_hash,
                ProjectionOwnerRebuildReceipt {
                    manifest_generation: view.generation,
                    event_watermark: view.event_watermark,
                    reconciled_source: view.reconciled_source,
                },
            ));
        }
        let manifest_generation = view
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("projection_generation_overflow"))?;
        let event_watermark = view
            .event_watermark
            .checked_add(u64::try_from(events.len()).map_err(|_| invalid("projection_sequence_overflow"))?)
            .ok_or_else(|| invalid("projection_sequence_overflow"))?;
        let reconciled_source = target;
        let published =
            self.commit_expected(&root_hash, ProjectionCommitActor::Worker, events, Vec::new(), canonical)?;
        Ok((
            published,
            ProjectionOwnerRebuildReceipt {
                manifest_generation,
                event_watermark,
                reconciled_source,
            },
        ))
    }

    pub(super) fn owner_rebuild(
        &self,
        selected_revision_ids: &[crate::memory::MemoryRevisionId],
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<ProjectionOwnerRebuildReceipt, ProjectionError> {
        if selected_revision_ids.is_empty() {
            return Err(invalid("projection_rebuild_requires_target"));
        }
        let view = self.current_view()?;
        let root_hash = self.root_hash()?;
        let selected: HashSet<_> = selected_revision_ids.iter().collect();
        if selected.len() != selected_revision_ids.len() {
            return Err(invalid("duplicate_projection_rebuild_target"));
        }
        let mut entries = Vec::with_capacity(selected_revision_ids.len());
        for revision_id in selected_revision_ids {
            let entry = view
                .entries
                .iter()
                .find(|entry| &entry.source.revision_id == revision_id)
                .ok_or_else(|| invalid("projection_rebuild_target_unreconciled"))?;
            if matches!(entry.state, ProjectionState::AbsentByPolicy { .. }) {
                return Err(invalid("projection_rebuild_target_not_eligible"));
            }
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.source.revision_sequence);
        let mut events = Vec::new();
        for entry in entries {
            if let ProjectionState::Ready { artifact, .. } = &entry.state {
                let stale_sequence = view
                    .event_watermark
                    .checked_add(events.len() as u64)
                    .and_then(|sequence| sequence.checked_add(1))
                    .ok_or_else(|| invalid("projection_sequence_overflow"))?;
                events.push(transition_event(
                    entry,
                    entry.desired_builder_spec_hash.clone(),
                    ProjectionState::Stale {
                        reason: super::model::StaleReason::OwnerRebuild,
                        artifact: artifact.clone(),
                    },
                )?);
                events.push(ManifestEvent::ProjectionTransition {
                    projection_id: entry.projection_id,
                    projection_kind: entry.projection_kind,
                    projection_version: entry
                        .projection_version
                        .checked_add(2)
                        .ok_or_else(|| invalid("projection_version_overflow"))?,
                    previous_sequence: Some(stale_sequence),
                    source: entry.source.clone(),
                    desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                    state: ProjectionState::Queued {
                        reason: QueueReason::OwnerRebuild,
                    },
                });
            } else {
                events.push(transition_event(
                    entry,
                    entry.desired_builder_spec_hash.clone(),
                    ProjectionState::Queued {
                        reason: QueueReason::OwnerRebuild,
                    },
                )?);
            }
        }
        let manifest_generation = view
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("projection_generation_overflow"))?;
        let event_watermark = view
            .event_watermark
            .checked_add(u64::try_from(events.len()).map_err(|_| invalid("projection_sequence_overflow"))?)
            .ok_or_else(|| invalid("projection_sequence_overflow"))?;
        let reconciled_source = view.reconciled_source.clone();
        self.commit_expected(
            &root_hash,
            ProjectionCommitActor::PersonalOwner,
            events,
            Vec::new(),
            canonical,
        )?;
        Ok(ProjectionOwnerRebuildReceipt {
            manifest_generation,
            event_watermark,
            reconciled_source,
        })
    }

    pub(super) fn claim_next(
        &self,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Option<ProjectionBuildGuard>, ProjectionError> {
        let view = self.current_view()?;
        let Some(entry) = view
            .entries
            .iter()
            .find(|entry| matches!(entry.state, ProjectionState::Queued { .. }))
            .cloned()
        else {
            return Ok(None);
        };
        let root_hash = self.root_hash()?;
        let attempt = entry
            .attempt_count
            .checked_add(1)
            .ok_or_else(|| invalid("projection_attempt_overflow"))?;
        let attempt_id = uuid::Uuid::new_v4();
        let committed_floor = {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            state
                .root
                .committed_at
                .checked_add(1)
                .ok_or_else(|| invalid("projection_commit_time_overflow"))?
        };
        let lease_expires_at = (self.clock)()
            .max(committed_floor)
            .checked_add(60_000)
            .ok_or_else(|| invalid("projection_lease_overflow"))?;
        let event = transition_event(
            &entry,
            entry.desired_builder_spec_hash.clone(),
            ProjectionState::Building {
                attempt,
                attempt_id,
                lease_expires_at,
            },
        )?;
        self.commit_expected(
            &root_hash,
            ProjectionCommitActor::Worker,
            vec![event],
            Vec::new(),
            canonical,
        )?;
        let claimed_view = self.current_view()?;
        let claim_event_watermark = claimed_view.event_watermark;
        let claimed_entry = claimed_view
            .entries
            .into_iter()
            .find(|candidate| candidate.projection_id == entry.projection_id)
            .ok_or_else(|| invalid("claimed_projection_missing"))?;
        Ok(Some(ProjectionBuildGuard {
            entry: claimed_entry,
            attempt,
            attempt_id,
            claim_event_watermark,
            canonical_watermark: watermark(&canonical.root_hash, &canonical.root),
        }))
    }

    pub(super) fn guard_is_current(&self, guard: &ProjectionBuildGuard) -> Result<bool, ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        if state.poisoned {
            return Err(ProjectionError::WriterPoisoned);
        }
        let Some(entry) = state
            .view
            .entries
            .iter()
            .find(|entry| entry.projection_id == guard.entry.projection_id)
        else {
            return Ok(false);
        };
        Ok(entry.projection_version == guard.entry.projection_version
            && entry.last_transition_sequence == guard.entry.last_transition_sequence
            && entry.source == guard.entry.source
            && entry.desired_builder_spec_hash == guard.entry.desired_builder_spec_hash
            && matches!(
                entry.state,
                ProjectionState::Building { attempt, attempt_id, .. }
                    if attempt == guard.attempt && attempt_id == guard.attempt_id
            ))
    }

    pub(super) fn worker_output_is_valid(
        &self,
        guard: &ProjectionBuildGuard,
        vector: &[f32],
    ) -> Result<bool, ProjectionError> {
        let (_, active) = self.current_guarded_builder(guard)?;
        Ok(validate_embedding_output(&active.builder_spec, vector).is_ok())
    }

    pub(super) fn complete_ready(
        &self,
        guard: &ProjectionBuildGuard,
        vector: Vec<f32>,
        canonical: &crate::memory::CanonicalProjectionGuard<'_>,
    ) -> Result<String, ProjectionError> {
        if !canonical.authorizes(&guard.entry.source) {
            return Err(ProjectionError::HeadConflict);
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        let (root_hash, active) = self.current_guarded_builder(guard)?;
        let artifact = EmbeddingArtifact {
            schema: super::model::EMBEDDING_ARTIFACT_SCHEMA.to_string(),
            projection_id: guard.entry.projection_id,
            source_revision_id: guard.entry.source.revision_id.clone(),
            source_content_hash: guard.entry.source.content_hash.clone(),
            builder_spec_hash: guard.entry.desired_builder_spec_hash.clone(),
            dimension: active.builder_spec.dimension,
            encoding: "f32-json/v1".to_string(),
            vector,
        };
        let (bytes, artifact_hash) = super::hash::artifact_bytes_and_hash(&artifact)?;
        let descriptor = super::model::ArtifactDescriptor {
            artifact_hash,
            byte_length: bytes.len() as u64,
            artifact_schema: super::model::EMBEDDING_ARTIFACT_SCHEMA.to_string(),
            dimension: active.builder_spec.dimension,
            source_revision_id: guard.entry.source.revision_id.clone(),
            source_content_hash: guard.entry.source.content_hash.clone(),
            builder_spec_hash: guard.entry.desired_builder_spec_hash.clone(),
        };
        let event = transition_event(
            &guard.entry,
            guard.entry.desired_builder_spec_hash.clone(),
            ProjectionState::Ready {
                attempt: guard.attempt,
                attempt_id: guard.attempt_id,
                artifact: descriptor,
            },
        )?;
        self.commit(
            &root_hash,
            ProjectionCommitActor::Worker,
            vec![event],
            vec![artifact],
            canonical.snapshot(),
            false,
        )
    }

    pub(super) fn complete_failed(
        &self,
        guard: &ProjectionBuildGuard,
        failure_category: super::model::FailureCategory,
        canonical: &crate::memory::CanonicalProjectionGuard<'_>,
    ) -> Result<String, ProjectionError> {
        if !canonical.authorizes(&guard.entry.source) {
            return Err(ProjectionError::HeadConflict);
        }
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        let (root_hash, _) = self.current_guarded_builder(guard)?;
        let retryable = !matches!(
            failure_category,
            super::model::FailureCategory::InvalidProjection | super::model::FailureCategory::ProviderIdentityChanged
        );
        let retry_not_before = if retryable {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            let exponent = guard.attempt.min(8);
            let retry_delay_ms = 1_000_u64.checked_shl(exponent).unwrap_or(256_000).min(300_000);
            Some(
                state
                    .root
                    .committed_at
                    .max((self.clock)())
                    .checked_add(retry_delay_ms.max(1))
                    .ok_or_else(|| invalid("projection_retry_time_overflow"))?,
            )
        } else {
            None
        };
        let event = transition_event(
            &guard.entry,
            guard.entry.desired_builder_spec_hash.clone(),
            ProjectionState::Failed {
                attempt: guard.attempt,
                attempt_id: guard.attempt_id,
                failure_category,
                retryable,
                retry_not_before,
            },
        )?;
        self.commit(
            &root_hash,
            ProjectionCommitActor::Worker,
            vec![event],
            Vec::new(),
            canonical.snapshot(),
            false,
        )
    }

    fn current_guarded_builder(
        &self,
        guard: &ProjectionBuildGuard,
    ) -> Result<(String, super::model::ActiveBuilderSpec), ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        if state.poisoned {
            return Err(ProjectionError::WriterPoisoned);
        }
        let entry = state
            .view
            .entries
            .iter()
            .find(|entry| entry.projection_id == guard.entry.projection_id)
            .ok_or(ProjectionError::HeadConflict)?;
        if entry.projection_version != guard.entry.projection_version
            || entry.last_transition_sequence != guard.entry.last_transition_sequence
            || entry.source != guard.entry.source
            || entry.desired_builder_spec_hash != guard.entry.desired_builder_spec_hash
            || !matches!(
                entry.state,
                ProjectionState::Building { attempt, attempt_id, .. }
                    if attempt == guard.attempt && attempt_id == guard.attempt_id
            )
        {
            return Err(ProjectionError::HeadConflict);
        }
        let active = state
            .view
            .active_builder_specs
            .iter()
            .find(|builder| builder.projection_kind == super::model::ProjectionKind::MemoryEmbedding)
            .filter(|builder| builder.builder_spec_hash == guard.entry.desired_builder_spec_hash)
            .cloned()
            .ok_or(ProjectionError::HeadConflict)?;
        Ok((state.root_hash.clone(), active))
    }

    pub(super) fn root_hash(&self) -> Result<String, ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        if state.poisoned {
            return Err(ProjectionError::WriterPoisoned);
        }
        if !state.artifact_issues.is_empty() {
            return Err(ProjectionError::ArtifactRepairRequired {
                count: state.artifact_issues.len(),
            });
        }
        Ok(state.root_hash.clone())
    }

    pub(super) fn repair_invalid_artifacts(
        &self,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Option<String>, ProjectionError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        let (root_hash, issues, view) = {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            if state.poisoned {
                return Err(ProjectionError::WriterPoisoned);
            }
            (
                state.root_hash.clone(),
                state.artifact_issues.clone(),
                state.view.clone(),
            )
        };
        if issues.is_empty() {
            self.cleanup_invalid_stale_artifacts_locked()?;
            return Ok(None);
        }
        let mut events = Vec::with_capacity(issues.len());
        for issue in issues {
            let entry = view
                .entries
                .iter()
                .find(|entry| entry.projection_id == issue.projection_id)
                .ok_or_else(|| invalid("artifact_repair_entry_missing"))?;
            let ProjectionState::Ready { artifact, .. } = &entry.state else {
                return Err(invalid("artifact_repair_entry_not_ready"));
            };
            events.push(ManifestEvent::ProjectionTransition {
                projection_id: entry.projection_id,
                projection_kind: entry.projection_kind,
                projection_version: entry
                    .projection_version
                    .checked_add(1)
                    .ok_or_else(|| invalid("projection_version_overflow"))?,
                previous_sequence: Some(entry.last_transition_sequence),
                source: entry.source.clone(),
                desired_builder_spec_hash: entry.desired_builder_spec_hash.clone(),
                state: ProjectionState::Stale {
                    reason: issue.reason,
                    artifact: artifact.clone(),
                },
            });
        }
        let published = self.commit(
            &root_hash,
            ProjectionCommitActor::Worker,
            events,
            Vec::new(),
            canonical,
            true,
        )?;
        {
            let mut state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            state.artifact_issues.clear();
        }
        self.cleanup_invalid_stale_artifacts_locked()?;
        Ok(Some(published))
    }

    fn cleanup_invalid_stale_artifacts_locked(&self) -> Result<usize, ProjectionError> {
        let view = {
            let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
            if state.poisoned {
                return Err(ProjectionError::WriterPoisoned);
            }
            if !state.artifact_issues.is_empty() {
                return Err(ProjectionError::ArtifactRepairRequired {
                    count: state.artifact_issues.len(),
                });
            }
            state.view.clone()
        };
        let mut removed = 0;
        for entry in &view.entries {
            let ProjectionState::Stale { reason, artifact } = &entry.state else {
                continue;
            };
            if !matches!(
                reason,
                super::model::StaleReason::ArtifactHashMismatch | super::model::StaleReason::ArtifactInvalid
            ) {
                continue;
            }
            let builder = view
                .active_builder_specs
                .iter()
                .find(|builder| builder.builder_spec_hash == artifact.builder_spec_hash)
                .ok_or_else(|| invalid("stale_artifact_builder_missing"))?;
            let snapshot = self
                .storage
                .snapshot_artifact_for_cleanup(&artifact.artifact_hash, MAX_EMBEDDING_ARTIFACT_BYTES)
                .map_err(|_| ProjectionError::ArtifactMaintenanceRequired)?;
            if snapshot.is_missing() {
                continue;
            }
            let invalid_on_disk = if !snapshot.is_private_regular() {
                true
            } else if let Some(bytes) = snapshot.bytes() {
                match parse_embedding_artifact(bytes) {
                    Ok(restored) => validate_artifact(&restored, artifact, &builder.builder_spec).is_err(),
                    Err(error @ ProjectionError::UnsupportedFormat { .. }) => return Err(error),
                    Err(_) => true,
                }
            } else {
                true
            };
            if !invalid_on_disk {
                return Err(ProjectionError::ArtifactMaintenanceRequired);
            }
            #[cfg(test)]
            if let Some((snapshot_ready, continue_cleanup)) = self
                .cleanup_barriers
                .lock()
                .map_err(|_| invalid("projection_cleanup_barrier_poisoned"))?
                .take()
            {
                snapshot_ready.wait();
                continue_cleanup.wait();
            }
            self.storage
                .remove_artifact_snapshot(snapshot)
                .map_err(|_| ProjectionError::ArtifactMaintenanceRequired)?;
            removed += 1;
        }
        Ok(removed)
    }

    pub(super) fn commit_expected(
        &self,
        expected_root_hash: &str,
        actor: ProjectionCommitActor,
        events: Vec<ManifestEvent>,
        artifacts: Vec<EmbeddingArtifact>,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<String, ProjectionError> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| invalid("projection_operation_poisoned"))?;
        self.commit(expected_root_hash, actor, events, artifacts, canonical, false)
    }

    fn commit(
        &self,
        expected_root_hash: &str,
        actor: ProjectionCommitActor,
        events: Vec<ManifestEvent>,
        artifacts: Vec<EmbeddingArtifact>,
        canonical: &CanonicalProjectionSnapshot,
        repairing_invalid_artifacts: bool,
    ) -> Result<String, ProjectionError> {
        let mut state = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.poisoned {
            return Err(ProjectionError::WriterPoisoned);
        }
        if !state.artifact_issues.is_empty() && !repairing_invalid_artifacts {
            return Err(ProjectionError::ArtifactRepairRequired {
                count: state.artifact_issues.len(),
            });
        }
        if state.root_hash != expected_root_hash {
            return Err(ProjectionError::HeadConflict);
        }
        if events.is_empty() {
            return Err(invalid("invalid_projection_commit_boundary"));
        }
        let committed_at = (self.clock)().max(
            state
                .root
                .committed_at
                .checked_add(1)
                .ok_or_else(|| invalid("projection_commit_time_overflow"))?,
        );
        if committed_at > 9_007_199_254_740_991 {
            return Err(invalid("projection_commit_time_overflow"));
        }
        let committed_by_role = actor.role_id();
        let first_sequence = (state.records.len() as u64)
            .checked_add(1)
            .ok_or_else(|| invalid("projection_sequence_overflow"))?;
        let records: Vec<_> = events
            .into_iter()
            .enumerate()
            .map(|(offset, event)| {
                Ok(ManifestRecord {
                    schema: MANIFEST_RECORD_SCHEMA.to_string(),
                    sequence: first_sequence
                        .checked_add(offset as u64)
                        .ok_or_else(|| invalid("projection_sequence_overflow"))?,
                    committed_at,
                    committed_by_role: committed_by_role.to_string(),
                    event,
                })
            })
            .collect::<Result<Vec<_>, ProjectionError>>()?;
        let mut candidate_records = state.records.clone();
        candidate_records.extend(records.iter().cloned());
        let generation = state
            .root
            .generation
            .checked_add(1)
            .ok_or_else(|| invalid("projection_generation_overflow"))?;
        let candidate_view = rebuild_current_view(generation, &state.genesis_source, &candidate_records)?;
        validate_candidate(&candidate_view, &candidate_records, canonical)?;
        let failed_transition = records.iter().any(|record| {
            matches!(
                &record.event,
                ManifestEvent::ProjectionTransition {
                    state: ProjectionState::Failed { .. },
                    ..
                }
            )
        });
        let artifact_count =
            u64::try_from(artifacts.len()).map_err(|_| invalid("projection_artifact_count_overflow"))?;
        let commit_kind = if artifact_count > 0 {
            "ready"
        } else if failed_transition {
            "failed"
        } else {
            "control"
        };
        tracing::debug!(
            operation = "projection.manifest_commit",
            phase = "manifest_transition",
            result_category = "validated",
            commit_kind,
            generation,
            event_watermark = candidate_records.len() as u64,
        );
        persist_ready_artifacts(&self.storage, &candidate_view, &records, artifacts)?;
        if artifact_count > 0 {
            tracing::debug!(
                operation = "projection.manifest_commit",
                phase = "artifact_verify",
                result_category = "durable",
                artifact_count,
            );
        }

        let segment = ProjectionSegment {
            schema: MANIFEST_SEGMENT_SCHEMA.to_string(),
            first_sequence,
            last_sequence: candidate_records.len() as u64,
            previous_segment_hash: state.root.manifest_head.clone(),
            records,
        };
        let (segment_bytes, segment_hash) = segment_bytes_and_hash(&segment)?;
        let (view_bytes, view_hash) = view_bytes_and_hash(&candidate_view)?;
        let root = ProjectionRoot {
            schema: MANIFEST_ROOT_SCHEMA.to_string(),
            generation,
            previous_root_hash: Some(state.root_hash.clone()),
            manifest_head: Some(segment_hash.clone()),
            event_watermark: candidate_records.len() as u64,
            current_view_hash: view_hash.clone(),
            reconciled_source: candidate_view.reconciled_source.clone(),
            committed_at,
            committed_by_role: committed_by_role.to_string(),
        };
        let (root_bytes, root_hash) = root_bytes_and_hash(&root)?;
        require_size(&segment_bytes, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
        require_size(&view_bytes, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
        require_size(&root_bytes, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
        self.storage.put_manifest_object(&segment_hash, &segment_bytes)?;
        self.storage.put_manifest_object(&view_hash, &view_bytes)?;
        self.storage.put_manifest_object(&root_hash, &root_bytes)?;
        self.storage.flush_manifest()?;
        #[cfg(test)]
        if self
            .fail_before_pointer_once
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            tracing::debug!(
                operation = "projection.manifest_commit",
                phase = "root_publish",
                result_category = "pre_exchange_failed",
                commit_kind,
                generation,
                event_watermark = candidate_records.len() as u64,
            );
            return Err(ProjectionError::Io(std::io::Error::other(
                "injected pre-pointer failure",
            )));
        }
        let pointer = RootPointer {
            schema: ROOT_POINTER_SCHEMA.to_string(),
            root_hash: root_hash.clone(),
        };
        let pointer_bytes = pointer_bytes(&pointer)?;
        require_size(&pointer_bytes, MAX_PROJECTION_POINTER_BYTES)?;
        match self.storage.publish_manifest_active(&pointer_bytes) {
            Ok(()) => {}
            Err(LedgerStorageError::Io(error)) => {
                tracing::debug!(
                    operation = "projection.manifest_commit",
                    phase = "root_publish",
                    result_category = "pre_exchange_failed",
                    commit_kind,
                    generation,
                    event_watermark = candidate_records.len() as u64,
                );
                return Err(ProjectionError::Io(error));
            }
            Err(LedgerStorageError::PublishedButUnsynced(_)) => {
                state.poisoned = true;
                tracing::debug!(
                    operation = "projection.manifest_commit",
                    phase = "root_publish",
                    result_category = "indeterminate",
                    commit_kind,
                    generation,
                    event_watermark = candidate_records.len() as u64,
                );
                return Err(ProjectionError::CommitIndeterminate);
            }
        }
        state.root_hash = root_hash.clone();
        state.root = root;
        state.view = candidate_view;
        state.records = candidate_records;
        tracing::debug!(
            operation = "projection.manifest_commit",
            phase = "root_publish",
            result_category = "published",
            commit_kind,
            generation,
            event_watermark = state.records.len() as u64,
        );
        Ok(root_hash)
    }

    #[cfg(test)]
    pub(super) fn inject_post_artifact_durability_failure_once(&self) {
        self.storage.inject_post_artifact_durability_failure_once();
    }

    #[cfg(test)]
    pub(super) fn inject_pre_pointer_failure_once(&self) {
        self.fail_before_pointer_once
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(super) fn inject_post_exchange_sync_failure_once(&self) {
        self.storage.inject_post_exchange_sync_failure_once();
    }

    #[cfg(test)]
    pub(super) fn inject_status_reconciled_source_for_test(&self, source: CanonicalWatermark) {
        self.state.lock().unwrap().view.reconciled_source = source;
    }

    #[cfg(test)]
    pub(super) fn inject_unknown_schema_for_test(
        &self,
        target: ProjectionUnknownSchemaTarget,
    ) -> Result<(), ProjectionError> {
        const FUTURE_SCHEMA: &str = "plico.projection.future/v999";

        fn set_string_field(
            value: &mut serde_json::Value,
            pointer: &str,
            field: &'static str,
        ) -> Result<(), ProjectionError> {
            let slot = value.pointer_mut(pointer).ok_or_else(|| invalid(field))?;
            *slot = serde_json::Value::String(FUTURE_SCHEMA.to_string());
            Ok(())
        }

        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        let publish_root = |root: &ProjectionRoot| -> Result<(), ProjectionError> {
            let (bytes, hash) = root_bytes_and_hash(root)?;
            self.storage.put_manifest_object(&hash, &bytes)?;
            self.storage.flush_manifest()?;
            self.storage
                .publish_manifest_active(&pointer_bytes(&RootPointer {
                    schema: ROOT_POINTER_SCHEMA.to_string(),
                    root_hash: hash,
                })?)
                .map_err(map_publish_error)
        };

        match target {
            ProjectionUnknownSchemaTarget::RootPointer => {
                let pointer = serde_json::json!({
                    "schema": FUTURE_SCHEMA,
                    "root_hash": state.root_hash,
                });
                self.storage
                    .publish_manifest_active(&pointer_bytes(&pointer)?)
                    .map_err(map_publish_error)?;
            }
            ProjectionUnknownSchemaTarget::CurrentRoot => {
                let mut value = serde_json::to_value(&state.root)?;
                set_string_field(&mut value, "/schema", "missing_root_schema_fixture")?;
                let (bytes, hash) = root_bytes_and_hash(&value)?;
                self.storage.put_manifest_object(&hash, &bytes)?;
                self.storage.flush_manifest()?;
                self.storage
                    .publish_manifest_active(&pointer_bytes(&RootPointer {
                        schema: ROOT_POINTER_SCHEMA.to_string(),
                        root_hash: hash,
                    })?)
                    .map_err(map_publish_error)?;
            }
            ProjectionUnknownSchemaTarget::HistoricalRoot => {
                let mut roots = load_root_chain(&self.storage, &state.root_hash, state.root.clone())?;
                if roots.len() < 2 {
                    return Err(invalid("historical_root_fixture_missing"));
                }
                let mut oldest = serde_json::to_value(&roots[0])?;
                set_string_field(&mut oldest, "/schema", "missing_historical_root_schema_fixture")?;
                let (bytes, mut previous_hash) = root_bytes_and_hash(&oldest)?;
                self.storage.put_manifest_object(&previous_hash, &bytes)?;
                for root in roots.iter_mut().skip(1) {
                    root.previous_root_hash = Some(previous_hash);
                    let (bytes, hash) = root_bytes_and_hash(root)?;
                    self.storage.put_manifest_object(&hash, &bytes)?;
                    previous_hash = hash;
                }
                self.storage.flush_manifest()?;
                self.storage
                    .publish_manifest_active(&pointer_bytes(&RootPointer {
                        schema: ROOT_POINTER_SCHEMA.to_string(),
                        root_hash: previous_hash,
                    })?)
                    .map_err(map_publish_error)?;
            }
            ProjectionUnknownSchemaTarget::CurrentSegment
            | ProjectionUnknownSchemaTarget::CurrentRecord
            | ProjectionUnknownSchemaTarget::RecordBuilder => {
                let segment_hash = state
                    .root
                    .manifest_head
                    .as_ref()
                    .ok_or_else(|| invalid("segment_fixture_missing"))?;
                let bytes = self
                    .storage
                    .read_manifest_object_bounded(segment_hash, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
                let mut value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let pointer = match target {
                    ProjectionUnknownSchemaTarget::CurrentSegment => "/schema",
                    ProjectionUnknownSchemaTarget::CurrentRecord => "/records/0/schema",
                    ProjectionUnknownSchemaTarget::RecordBuilder => "/records/0/event/builder_spec/schema",
                    _ => unreachable!("guarded by projection schema target match"),
                };
                set_string_field(&mut value, pointer, "missing_segment_schema_fixture")?;
                let (bytes, hash) = segment_bytes_and_hash(&value)?;
                self.storage.put_manifest_object(&hash, &bytes)?;
                let mut root = state.root.clone();
                root.manifest_head = Some(hash);
                publish_root(&root)?;
            }
            ProjectionUnknownSchemaTarget::CurrentView
            | ProjectionUnknownSchemaTarget::ViewBuilder
            | ProjectionUnknownSchemaTarget::ArtifactDescriptor
            | ProjectionUnknownSchemaTarget::Artifact => {
                let bytes = self.storage.read_manifest_object_bounded(
                    &state.root.current_view_hash,
                    MAX_PROJECTION_MANIFEST_OBJECT_BYTES,
                )?;
                let mut value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let mut root = state.root.clone();
                match target {
                    ProjectionUnknownSchemaTarget::CurrentView => {
                        set_string_field(&mut value, "/schema", "missing_view_schema_fixture")?;
                    }
                    ProjectionUnknownSchemaTarget::ViewBuilder => {
                        set_string_field(
                            &mut value,
                            "/active_builder_specs/0/builder_spec/schema",
                            "missing_view_builder_schema_fixture",
                        )?;
                    }
                    ProjectionUnknownSchemaTarget::ArtifactDescriptor => {
                        set_string_field(
                            &mut value,
                            "/entries/0/state/artifact/artifact_schema",
                            "missing_artifact_descriptor_schema_fixture",
                        )?;
                    }
                    ProjectionUnknownSchemaTarget::Artifact => {
                        let artifact_hash = value
                            .pointer("/entries/0/state/artifact/artifact_hash")
                            .and_then(serde_json::Value::as_str)
                            .ok_or_else(|| invalid("missing_artifact_schema_fixture"))?
                            .to_string();
                        let artifact_bytes = self
                            .storage
                            .read_artifact_bounded(&artifact_hash, MAX_EMBEDDING_ARTIFACT_BYTES)?;
                        let mut artifact: serde_json::Value = serde_json::from_slice(&artifact_bytes)?;
                        set_string_field(&mut artifact, "/schema", "missing_artifact_schema_fixture")?;
                        let (artifact_bytes, _) = artifact_bytes_and_hash(&artifact)?;
                        self.storage.inject_replace_artifact(&artifact_hash, &artifact_bytes)?;
                        return Ok(());
                    }
                    _ => unreachable!("guarded by projection schema target match"),
                }
                let (bytes, hash) = view_bytes_and_hash(&value)?;
                self.storage.put_manifest_object(&hash, &bytes)?;
                root.current_view_hash = hash;
                publish_root(&root)?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn artifact_hashes(&self) -> Result<Vec<String>, ProjectionError> {
        Ok(self.storage.artifact_hashes(MAX_EMBEDDING_ARTIFACT_BYTES)?)
    }

    #[cfg(test)]
    pub(super) fn inject_missing_artifact(&self, hash: &str) -> Result<(), ProjectionError> {
        Ok(self.storage.inject_remove_artifact(hash)?)
    }

    #[cfg(test)]
    pub(super) fn inject_artifact_cleanup_failure_once(&self) {
        self.storage.inject_artifact_cleanup_failure_once();
    }

    #[cfg(test)]
    pub(super) fn inject_cleanup_barriers(
        &self,
        snapshot_ready: Arc<std::sync::Barrier>,
        continue_cleanup: Arc<std::sync::Barrier>,
    ) {
        *self.cleanup_barriers.lock().unwrap() = Some((snapshot_ready, continue_cleanup));
    }

    #[cfg(test)]
    pub(super) fn inject_corrupt_artifact(&self, hash: &str, bytes: &[u8]) -> Result<(), ProjectionError> {
        Ok(self.storage.inject_replace_artifact(hash, bytes)?)
    }

    #[cfg(all(test, unix))]
    pub(super) fn inject_permissive_artifact_mode(&self, hash: &str) -> Result<(), ProjectionError> {
        Ok(self.storage.inject_permissive_artifact_mode(hash)?)
    }

    #[cfg(all(test, unix))]
    pub(super) fn inject_artifact_symlink(&self, hash: &str) -> Result<(), ProjectionError> {
        Ok(self.storage.inject_artifact_symlink(hash)?)
    }

    #[cfg(all(test, unix))]
    pub(super) fn inject_artifact_fifo(&self, hash: &str) -> Result<(), ProjectionError> {
        Ok(self.storage.inject_artifact_fifo(hash)?)
    }

    #[cfg(test)]
    pub(super) fn inject_empty_generation_root(&self) -> Result<(), ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        let root = ProjectionRoot {
            schema: MANIFEST_ROOT_SCHEMA.to_string(),
            generation: state
                .root
                .generation
                .checked_add(1)
                .ok_or_else(|| invalid("projection_generation_overflow"))?,
            previous_root_hash: Some(state.root_hash.clone()),
            manifest_head: state.root.manifest_head.clone(),
            event_watermark: state.root.event_watermark,
            current_view_hash: state.root.current_view_hash.clone(),
            reconciled_source: state.root.reconciled_source.clone(),
            committed_at: state
                .root
                .committed_at
                .checked_add(1)
                .ok_or_else(|| invalid("projection_commit_time_overflow"))?,
            committed_by_role: crate::PERSONAL_OWNER_ROLE_ID.to_string(),
        };
        let (bytes, hash) = root_bytes_and_hash(&root)?;
        self.storage.put_manifest_object(&hash, &bytes)?;
        self.storage.flush_manifest()?;
        self.storage
            .publish_manifest_active(&pointer_bytes(&RootPointer {
                schema: ROOT_POINTER_SCHEMA.to_string(),
                root_hash: hash,
            })?)
            .map_err(map_publish_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn inject_intermediate_source_tamper(&self) -> Result<(), ProjectionError> {
        let state = self.state.lock().map_err(|_| invalid("projection_state_poisoned"))?;
        let mut roots = load_root_chain(&self.storage, &state.root_hash, state.root.clone())?;
        if roots.len() < 3 || roots[1].reconciled_source == state.view.reconciled_source {
            return Err(invalid("projection_tamper_fixture_not_ready"));
        }
        roots[1].reconciled_source = state.view.reconciled_source.clone();
        let (_, mut previous_hash) = root_bytes_and_hash(&roots[0])?;
        for root in roots.iter_mut().skip(1) {
            root.previous_root_hash = Some(previous_hash);
            let (bytes, hash) = root_bytes_and_hash(root)?;
            self.storage.put_manifest_object(&hash, &bytes)?;
            previous_hash = hash;
        }
        self.storage.flush_manifest()?;
        self.storage
            .publish_manifest_active(&pointer_bytes(&RootPointer {
                schema: ROOT_POINTER_SCHEMA.to_string(),
                root_hash: previous_hash,
            })?)
            .map_err(map_publish_error)?;
        Ok(())
    }
}

fn inspect_projection_reader(
    reader: &ExistingProjectionReadOnly<'_>,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<ProjectionStoreInspection, ProjectionError> {
    match load_state(reader, reader, canonical).and_then(|state| {
        validate_state_against_canonical(&state, canonical)?;
        require_canonical_genesis(&state, canonical)?;
        Ok(state)
    }) {
        Ok(state)
            if state.records.is_empty() && state.root.generation == 0 && state.view.active_builder_specs.is_empty() =>
        {
            Ok(ProjectionStoreInspection::GenesisOnly)
        }
        Ok(state) if state.artifact_issues.is_empty() => Ok(ProjectionStoreInspection::Valid {
            event_watermark: state.view.event_watermark,
        }),
        Ok(state) => Ok(ProjectionStoreInspection::RepairRequired {
            count: state.artifact_issues.len(),
        }),
        Err(ProjectionError::UnsupportedFormat { .. }) => Ok(ProjectionStoreInspection::UnsupportedFormat),
        Err(ProjectionError::Invalid { category })
            if category == "projection_genesis_source_mismatch"
                || category.contains("canonical_ancestor")
                || category.contains("canonical_lineage") =>
        {
            Ok(ProjectionStoreInspection::ResetRequired(
                ProjectionPairResetReason::CanonicalLineageInvalid,
            ))
        }
        Err(ProjectionError::Invalid { .. } | ProjectionError::Serialization(_)) => Ok(
            ProjectionStoreInspection::ResetRequired(ProjectionPairResetReason::ManifestIntegrityInvalid),
        ),
        Err(ProjectionError::Io(error)) => Ok(classify_projection_io(error, false)),
        Err(ProjectionError::ResetPending) => Ok(ProjectionStoreInspection::ResetPending),
        Err(ProjectionError::ProjectionMaintenanceRequired) => Ok(ProjectionStoreInspection::MaintenanceRequired),
        Err(ProjectionError::ManualInterventionRequired) => Ok(ProjectionStoreInspection::ManualIntervention),
        Err(
            ProjectionError::HeadConflict
            | ProjectionError::CommitIndeterminate
            | ProjectionError::WriterPoisoned
            | ProjectionError::ArtifactRepairRequired { .. }
            | ProjectionError::ArtifactMaintenanceRequired
            | ProjectionError::AllEligibleRequired
            | ProjectionError::ArtifactStoreUnavailable,
        ) => Ok(ProjectionStoreInspection::Unavailable(
            ProjectionStoreUnavailable::StorageIo,
        )),
    }
}

fn classify_projection_io(error: std::io::Error, layout_boundary: bool) -> ProjectionStoreInspection {
    if error.kind() == std::io::ErrorKind::NotFound {
        return ProjectionStoreInspection::ResetRequired(ProjectionPairResetReason::ManifestIncomplete);
    }
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return if error.raw_os_error().is_some() {
            ProjectionStoreInspection::Unavailable(ProjectionStoreUnavailable::PermissionDenied)
        } else {
            ProjectionStoreInspection::ResetRequired(ProjectionPairResetReason::StorageLayoutInvalid)
        };
    }
    if error.kind() == std::io::ErrorKind::InvalidData {
        return ProjectionStoreInspection::ResetRequired(if layout_boundary {
            ProjectionPairResetReason::StorageLayoutInvalid
        } else {
            ProjectionPairResetReason::ManifestIntegrityInvalid
        });
    }
    if matches!(error.raw_os_error(), Some(12 | 23 | 24 | 28)) {
        return ProjectionStoreInspection::Unavailable(ProjectionStoreUnavailable::ResourceExhausted);
    }
    ProjectionStoreInspection::Unavailable(ProjectionStoreUnavailable::StorageIo)
}

fn persist_ready_artifacts(
    storage: &ProjectionStorageBundle,
    view: &ProjectionCurrentView,
    records: &[ManifestRecord],
    artifacts: Vec<EmbeddingArtifact>,
) -> Result<(), ProjectionError> {
    let mut supplied = HashMap::new();
    for artifact in artifacts {
        let (_, hash) = artifact_bytes_and_hash(&artifact)?;
        if supplied.insert(hash, artifact).is_some() {
            return Err(invalid("duplicate_projection_artifact"));
        }
    }
    let mut required = HashSet::new();
    for record in records {
        if let ManifestEvent::ProjectionTransition {
            state: ProjectionState::Ready { artifact, .. },
            ..
        } = &record.event
        {
            if !required.insert(artifact.artifact_hash.clone()) {
                return Err(invalid("duplicate_ready_artifact"));
            }
        }
    }
    if supplied.keys().cloned().collect::<HashSet<_>>() != required {
        return Err(invalid("ready_artifact_set_mismatch"));
    }
    for record in records {
        let ManifestEvent::ProjectionTransition {
            source,
            desired_builder_spec_hash,
            state: ProjectionState::Ready { artifact, .. },
            ..
        } = &record.event
        else {
            continue;
        };
        let supplied_artifact = supplied
            .remove(&artifact.artifact_hash)
            .ok_or_else(|| invalid("missing_ready_artifact"))?;
        if supplied_artifact.source_revision_id != source.revision_id
            || supplied_artifact.source_content_hash != source.content_hash
            || supplied_artifact.builder_spec_hash != *desired_builder_spec_hash
        {
            return Err(invalid("ready_artifact_binding_mismatch"));
        }
        let builder = view
            .active_builder_specs
            .iter()
            .find(|builder| builder.builder_spec_hash == artifact.builder_spec_hash)
            .ok_or_else(|| invalid("ready_artifact_builder_missing"))?;
        let bytes = validate_artifact(&supplied_artifact, artifact, &builder.builder_spec)?;
        storage
            .put_artifact(&artifact.artifact_hash, &bytes)
            .map_err(|_| ProjectionError::ArtifactStoreUnavailable)?;
        let restored: EmbeddingArtifact = parse_embedding_artifact(
            &storage
                .read_artifact_bounded(&artifact.artifact_hash, MAX_EMBEDDING_ARTIFACT_BYTES)
                .map_err(|_| ProjectionError::ArtifactStoreUnavailable)?,
        )?;
        validate_artifact(&restored, artifact, &builder.builder_spec)?;
    }
    if supplied.is_empty() {
        storage
            .flush_artifacts()
            .map_err(|_| ProjectionError::ArtifactStoreUnavailable)?;
        Ok(())
    } else {
        Err(invalid("unused_projection_artifact"))
    }
}

pub(super) fn ensure_genesis(
    storage: &ProjectionStorageBundle,
    genesis_source: &CanonicalWatermark,
) -> Result<(), ProjectionError> {
    if storage
        .read_manifest_active_bounded(MAX_PROJECTION_POINTER_BYTES)?
        .is_some()
    {
        return Ok(());
    }
    let view = rebuild_current_view(0, genesis_source, &[])?;
    let (view_bytes, view_hash) = view_bytes_and_hash(&view)?;
    let root = ProjectionRoot {
        schema: MANIFEST_ROOT_SCHEMA.to_string(),
        generation: 0,
        previous_root_hash: None,
        manifest_head: None,
        event_watermark: 0,
        current_view_hash: view_hash.clone(),
        reconciled_source: genesis_source.clone(),
        committed_at: 0,
        committed_by_role: crate::PERSONAL_OWNER_ROLE_ID.to_string(),
    };
    let (root_bytes, root_hash) = root_bytes_and_hash(&root)?;
    require_size(&view_bytes, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
    require_size(&root_bytes, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
    let allowed = HashSet::from([view_hash.clone(), root_hash.clone()]);
    if storage
        .list_manifest_objects()?
        .into_iter()
        .any(|hash| !allowed.contains(&hash))
    {
        return Err(invalid("projection_genesis_has_unpublished_objects"));
    }
    storage.put_manifest_object(&view_hash, &view_bytes)?;
    storage.put_manifest_object(&root_hash, &root_bytes)?;
    let pointer = RootPointer {
        schema: ROOT_POINTER_SCHEMA.to_string(),
        root_hash,
    };
    let pointer_bytes = pointer_bytes(&pointer)?;
    require_size(&pointer_bytes, MAX_PROJECTION_POINTER_BYTES)?;
    storage
        .publish_manifest_active(&pointer_bytes)
        .map_err(|error| match error {
            LedgerStorageError::Io(error) => ProjectionError::Io(error),
            LedgerStorageError::PublishedButUnsynced(_) => ProjectionError::CommitIndeterminate,
        })
}

fn load_state<M: ProjectionManifestReader, A: ProjectionArtifactReader>(
    storage: &M,
    artifacts: &A,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<ProjectionStoreState, ProjectionError> {
    let pointer_bytes_value = storage
        .read_active_bounded(MAX_PROJECTION_POINTER_BYTES)?
        .ok_or_else(|| invalid("missing_projection_pointer"))?;
    let pointer: RootPointer =
        parse_canonical_with_schema(&pointer_bytes_value, ROOT_POINTER_SCHEMA, "root_pointer", |_| Ok(()))?;
    let active_root: ProjectionRoot = read_object(storage, &pointer.root_hash, HashKind::Root)?;
    let roots = load_root_chain(storage, &pointer.root_hash, active_root.clone())?;
    let genesis_root = roots.first().ok_or_else(|| invalid("missing_projection_genesis"))?;
    let genesis_view: ProjectionCurrentView = read_object(storage, &genesis_root.current_view_hash, HashKind::View)?;
    let genesis_source = genesis_view.reconciled_source.clone();
    validate_historical_roots(storage, &roots, &genesis_source, canonical)?;
    let records = load_segment_chain(
        storage,
        active_root.manifest_head.as_deref(),
        active_root.event_watermark,
    )?;
    let rebuilt = rebuild_current_view(active_root.generation, &genesis_source, &records)?;
    let stored_view: ProjectionCurrentView = read_object(storage, &active_root.current_view_hash, HashKind::View)?;
    if stored_view != rebuilt || active_root.reconciled_source != stored_view.reconciled_source {
        return Err(invalid("projection_current_view_rebuild_mismatch"));
    }
    let referenced_hashes = stored_view
        .entries
        .iter()
        .filter_map(|entry| match &entry.state {
            ProjectionState::Ready { artifact, .. } | ProjectionState::Stale { artifact, .. } => {
                Some(artifact.artifact_hash.clone())
            }
            _ => None,
        })
        .collect();
    let is_genesis_only = active_root.generation == 0
        && active_root.manifest_head.is_none()
        && active_root.event_watermark == 0
        && records.is_empty()
        && stored_view.generation == 0
        && stored_view.event_watermark == 0
        && stored_view.active_builder_specs.is_empty()
        && stored_view.entries.is_empty();
    if !is_genesis_only {
        artifacts.validate_inventory(&referenced_hashes, MAX_EMBEDDING_ARTIFACT_BYTES)?;
    }
    let artifact_issues = inspect_ready_artifacts(artifacts, &stored_view)?;
    Ok(ProjectionStoreState {
        poisoned: false,
        root_hash: pointer.root_hash,
        root: active_root,
        view: stored_view,
        records,
        genesis_source,
        artifact_issues,
    })
}

fn load_root_chain<M: ProjectionManifestReader>(
    storage: &M,
    active_hash: &str,
    active_root: ProjectionRoot,
) -> Result<Vec<ProjectionRoot>, ProjectionError> {
    let mut roots = Vec::new();
    let mut seen = HashSet::new();
    let mut hash = active_hash.to_string();
    let mut root = active_root;
    let mut expected_generation = root.generation;
    loop {
        if !seen.insert(hash.clone()) || root.schema != MANIFEST_ROOT_SCHEMA || root.generation != expected_generation {
            return Err(invalid("invalid_projection_root_chain"));
        }
        roots.push(root.clone());
        match root.previous_root_hash.clone() {
            Some(previous_hash) => {
                expected_generation = expected_generation
                    .checked_sub(1)
                    .ok_or_else(|| invalid("invalid_projection_root_generation"))?;
                let previous: ProjectionRoot = read_object(storage, &previous_hash, HashKind::Root)?;
                if root.event_watermark < previous.event_watermark {
                    return Err(invalid("projection_watermark_regression"));
                }
                hash = previous_hash;
                root = previous;
            }
            None if expected_generation == 0
                && root.manifest_head.is_none()
                && root.event_watermark == 0
                && root.committed_at == 0
                && root.committed_by_role == crate::PERSONAL_OWNER_ROLE_ID =>
            {
                roots.reverse();
                return Ok(roots);
            }
            None => return Err(invalid("truncated_projection_root_chain")),
        }
    }
}

fn validate_historical_roots<M: ProjectionManifestReader>(
    storage: &M,
    roots: &[ProjectionRoot],
    genesis_source: &CanonicalWatermark,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    let mut prior_records: Vec<ManifestRecord> = Vec::new();
    let mut prior_root: Option<&ProjectionRoot> = None;
    for root in roots {
        if root.generation > 9_007_199_254_740_991
            || root.event_watermark > 9_007_199_254_740_991
            || root.committed_at > 9_007_199_254_740_991
            || (root.generation > 0 && (root.committed_at == 0 || root.committed_by_role.trim().is_empty()))
        {
            return Err(invalid("invalid_projection_root_boundary"));
        }
        let records = load_segment_chain(storage, root.manifest_head.as_deref(), root.event_watermark)?;
        if !records.starts_with(&prior_records) {
            return Err(invalid("projection_history_not_prefix"));
        }
        if let Some(previous) = prior_root {
            if root.event_watermark <= previous.event_watermark || root.committed_at <= previous.committed_at {
                return Err(invalid("empty_projection_generation"));
            }
            let suffix = &records[prior_records.len()..];
            if suffix.is_empty()
                || suffix.iter().any(|record| {
                    record.committed_at != root.committed_at || record.committed_by_role != root.committed_by_role
                })
            {
                return Err(invalid("projection_root_record_boundary_mismatch"));
            }
            let head = root
                .manifest_head
                .as_deref()
                .ok_or_else(|| invalid("missing_projection_segment_head"))?;
            let segment: ProjectionSegment = read_object(storage, head, HashKind::Segment)?;
            if segment.previous_segment_hash != previous.manifest_head
                || previous.event_watermark.checked_add(1) != Some(segment.first_sequence)
                || segment.last_sequence != root.event_watermark
                || segment.records != suffix
            {
                return Err(invalid("projection_generation_segment_mismatch"));
            }
        }
        let rebuilt = rebuild_current_view(root.generation, genesis_source, &records)?;
        validate_candidate(&rebuilt, &records, canonical)?;
        let stored: ProjectionCurrentView = read_object(storage, &root.current_view_hash, HashKind::View)?;
        if stored != rebuilt || root.reconciled_source != rebuilt.reconciled_source {
            return Err(invalid("historical_projection_view_mismatch"));
        }
        prior_records = records;
        prior_root = Some(root);
    }
    Ok(())
}

fn load_segment_chain<M: ProjectionManifestReader>(
    storage: &M,
    head: Option<&str>,
    watermark: u64,
) -> Result<Vec<ManifestRecord>, ProjectionError> {
    if watermark == 0 {
        return if head.is_none() {
            Ok(Vec::new())
        } else {
            Err(invalid("empty_projection_log_has_segment"))
        };
    }
    let mut segments = Vec::new();
    let mut next = head.map(str::to_string);
    let mut seen = HashSet::new();
    while let Some(hash) = next {
        if !seen.insert(hash.clone()) {
            return Err(invalid("projection_segment_cycle"));
        }
        let segment: ProjectionSegment = read_object(storage, &hash, HashKind::Segment)?;
        if segment.schema != MANIFEST_SEGMENT_SCHEMA || segment.records.is_empty() {
            return Err(invalid("invalid_projection_segment"));
        }
        next = segment.previous_segment_hash.clone();
        segments.push(segment);
    }
    segments.reverse();
    let mut records = Vec::new();
    let mut expected_sequence = 1;
    for segment in segments {
        if segment.first_sequence != expected_sequence
            || segment.last_sequence != segment.first_sequence + segment.records.len() as u64 - 1
        {
            return Err(invalid("non_contiguous_projection_segment"));
        }
        expected_sequence = segment.last_sequence + 1;
        records.extend(segment.records);
    }
    if records.len() as u64 != watermark {
        return Err(invalid("projection_log_watermark_mismatch"));
    }
    Ok(records)
}

pub(super) fn validate_candidate(
    view: &ProjectionCurrentView,
    records: &[ManifestRecord],
    canonical: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    for record in records {
        if let ManifestEvent::ProjectionTransition { source, .. } = &record.event {
            validate_source_against_canonical(source, canonical)?;
        }
    }
    let source = &view.reconciled_source;
    let matches_ancestor = canonical.root_chain.iter().any(|(hash, root)| {
        source.root_hash == *hash
            && source.generation == root.generation
            && source.revision_watermark == root.revision_watermark
            && source.policy_watermark == root.policy_watermark
            && source.relation_watermark == root.relation_watermark
    });
    if !matches_ancestor {
        return Err(invalid("projection_reconciliation_not_canonical_ancestor"));
    }
    let covered: Vec<_> = canonical
        .revisions
        .iter()
        .filter(|revision| revision.sequence <= source.revision_watermark)
        .collect();
    if covered.len() as u64 != source.revision_watermark
        || covered.iter().any(|revision| {
            !view
                .entries
                .iter()
                .any(|entry| entry.source.revision_id == revision.revision_id)
        })
    {
        return Err(invalid("projection_reconciliation_incomplete"));
    }
    for entry in &view.entries {
        if entry.source.revision_sequence > source.revision_watermark {
            if !matches!(
                entry.state,
                ProjectionState::Queued {
                    reason: QueueReason::CanonicalCommit
                }
            ) {
                return Err(invalid("invalid_write_through_projection_state"));
            }
            continue;
        }
        let revision = canonical
            .revisions
            .iter()
            .find(|revision| revision.revision_id == entry.source.revision_id)
            .ok_or_else(|| invalid("projection_source_not_canonical"))?;
        let expected_absent = expected_absent_reason(
            revision,
            canonical
                .revisions
                .iter()
                .filter(|candidate| candidate.sequence <= source.revision_watermark),
        );
        match (&entry.state, expected_absent) {
            (ProjectionState::AbsentByPolicy { reason }, Some(expected)) if *reason == expected => {}
            (ProjectionState::AbsentByPolicy { .. }, None) => {
                return Err(invalid("eligible_revision_marked_absent"));
            }
            (_, Some(_)) => return Err(invalid("ineligible_revision_has_projection_state")),
            (_, None) => {}
        }
    }
    if source.root_hash == canonical.root_hash {
        validate_reconciled_coverage(source, canonical)?;
    }
    Ok(())
}

fn expected_absent_reason<'a>(
    revision: &crate::memory::CanonicalRevision,
    revisions: impl Iterator<Item = &'a crate::memory::CanonicalRevision>,
) -> Option<AbsentReason> {
    if revisions
        .into_iter()
        .any(|candidate| candidate.parent_revision_id.as_ref() == Some(&revision.revision_id))
    {
        Some(AbsentReason::Superseded)
    } else if revision.deleted_at.is_some() {
        Some(AbsentReason::Deleted)
    } else if !matches!(revision.cognitive_tier, MemoryTier::Working | MemoryTier::LongTerm) {
        Some(AbsentReason::UnsupportedTier)
    } else {
        match &revision.content {
            MemoryContent::Text(text) if text.trim().is_empty() => Some(AbsentReason::BlankText),
            MemoryContent::Text(_) => None,
            _ => Some(AbsentReason::UnsupportedContent),
        }
    }
}

fn validate_state_against_canonical(
    state: &ProjectionStoreState,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    validate_candidate(&state.view, &state.records, canonical)?;
    Ok(())
}

fn inspect_ready_artifacts<A: ProjectionArtifactReader>(
    storage: &A,
    view: &ProjectionCurrentView,
) -> Result<Vec<ArtifactIssue>, ProjectionError> {
    let mut issues = Vec::new();
    for entry in &view.entries {
        let ProjectionState::Ready { artifact, .. } = &entry.state else {
            continue;
        };
        let bytes = match storage.read_artifact_bounded(&artifact.artifact_hash, MAX_EMBEDDING_ARTIFACT_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                issues.push(ArtifactIssue {
                    projection_id: entry.projection_id,
                    reason: super::model::StaleReason::ArtifactMissing,
                });
                continue;
            }
            Err(_) => {
                issues.push(ArtifactIssue {
                    projection_id: entry.projection_id,
                    reason: super::model::StaleReason::ArtifactInvalid,
                });
                continue;
            }
        };
        let restored: EmbeddingArtifact = match parse_embedding_artifact(&bytes) {
            Ok(artifact) => artifact,
            Err(error @ ProjectionError::UnsupportedFormat { .. }) => return Err(error),
            Err(_) => {
                issues.push(ArtifactIssue {
                    projection_id: entry.projection_id,
                    reason: super::model::StaleReason::ArtifactInvalid,
                });
                continue;
            }
        };
        let restored_hash = match artifact_bytes_and_hash(&restored) {
            Ok((_, hash)) => hash,
            Err(_) => {
                issues.push(ArtifactIssue {
                    projection_id: entry.projection_id,
                    reason: super::model::StaleReason::ArtifactInvalid,
                });
                continue;
            }
        };
        if restored_hash != artifact.artifact_hash {
            issues.push(ArtifactIssue {
                projection_id: entry.projection_id,
                reason: super::model::StaleReason::ArtifactHashMismatch,
            });
        } else if view
            .active_builder_specs
            .iter()
            .find(|builder| builder.builder_spec_hash == artifact.builder_spec_hash)
            .is_none_or(|builder| validate_artifact(&restored, artifact, &builder.builder_spec).is_err())
        {
            issues.push(ArtifactIssue {
                projection_id: entry.projection_id,
                reason: super::model::StaleReason::ArtifactInvalid,
            });
        }
    }
    Ok(issues)
}

fn read_object<T: ProjectionStoredObject, M: ProjectionManifestReader>(
    storage: &M,
    hash: &str,
    kind: HashKind,
) -> Result<T, ProjectionError> {
    let bytes = storage.read_object_bounded(hash, MAX_PROJECTION_MANIFEST_OBJECT_BYTES)?;
    let value: T = parse_canonical_with_schema(&bytes, T::SCHEMA, T::COMPONENT, T::validate_nested_schemas)?;
    verify_hash(&value, hash, kind)?;
    Ok(value)
}

fn parse_embedding_artifact(bytes: &[u8]) -> Result<EmbeddingArtifact, ProjectionError> {
    parse_canonical_with_schema(bytes, EMBEDDING_ARTIFACT_SCHEMA, "embedding_artifact", |_| Ok(()))
}

fn validate_segment_schema_tree(value: &serde_json::Value) -> Result<(), ProjectionError> {
    let Some(records) = value.get("records").and_then(serde_json::Value::as_array) else {
        return Ok(());
    };
    for record in records {
        require_schema(record, MANIFEST_RECORD_SCHEMA, "manifest_record")?;
        let Some(event) = record.get("event") else {
            continue;
        };
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("builder_activated") => {
                if let Some(builder) = event.get("builder_spec") {
                    validate_builder_schema_tree(builder)?;
                }
            }
            Some("projection_transition") => {
                if let Some(state) = event.get("state") {
                    validate_state_artifact_schema(state)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_current_view_schema_tree(value: &serde_json::Value) -> Result<(), ProjectionError> {
    if let Some(builders) = value.get("active_builder_specs").and_then(serde_json::Value::as_array) {
        for active in builders {
            if let Some(builder) = active.get("builder_spec") {
                validate_builder_schema_tree(builder)?;
            }
        }
    }
    if let Some(entries) = value.get("entries").and_then(serde_json::Value::as_array) {
        for entry in entries {
            if let Some(state) = entry.get("state") {
                validate_state_artifact_schema(state)?;
            }
        }
    }
    Ok(())
}

fn validate_builder_schema_tree(value: &serde_json::Value) -> Result<(), ProjectionError> {
    require_schema(value, BUILDER_SPEC_SCHEMA, "builder_spec")?;
    require_named_schema_if_present(
        value,
        "artifact_schema",
        EMBEDDING_ARTIFACT_SCHEMA,
        "embedding_artifact",
    )
}

fn validate_state_artifact_schema(value: &serde_json::Value) -> Result<(), ProjectionError> {
    if let Some(artifact) = value.get("artifact") {
        require_named_schema_if_present(
            artifact,
            "artifact_schema",
            EMBEDDING_ARTIFACT_SCHEMA,
            "embedding_artifact",
        )?;
    }
    Ok(())
}

fn require_named_schema_if_present(
    value: &serde_json::Value,
    field: &str,
    expected: &str,
    component: &'static str,
) -> Result<(), ProjectionError> {
    if let Some(actual) = value.get(field).and_then(serde_json::Value::as_str) {
        if actual != expected {
            return Err(ProjectionError::UnsupportedFormat { component });
        }
    }
    Ok(())
}

fn watermark(hash: &str, root: &crate::memory::LedgerRoot) -> CanonicalWatermark {
    CanonicalWatermark {
        root_hash: hash.to_string(),
        generation: root.generation,
        revision_watermark: root.revision_watermark,
        policy_watermark: root.policy_watermark,
        relation_watermark: root.relation_watermark,
    }
}

fn require_canonical_genesis(
    state: &ProjectionStoreState,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<(), ProjectionError> {
    if state.genesis_source == watermark(&canonical.genesis_root_hash, &canonical.genesis_root) {
        Ok(())
    } else {
        Err(invalid("projection_genesis_source_mismatch"))
    }
}

fn require_builder_hash_in_state(
    state: &ProjectionStoreState,
    expected_builder_spec_hash: &str,
) -> Result<(), ProjectionError> {
    let active = state
        .view
        .active_builder_specs
        .iter()
        .find(|builder| builder.projection_kind == super::model::ProjectionKind::MemoryEmbedding)
        .ok_or_else(|| invalid("projection_not_initialized"))?;
    if active.builder_spec_hash == expected_builder_spec_hash {
        Ok(())
    } else {
        Err(invalid("builder_change_requires_owner"))
    }
}

fn projection_state_is_genesis_only(state: &ProjectionStoreState) -> bool {
    state.records.is_empty()
        && state.root.generation == 0
        && state.root.event_watermark == 0
        && state.view.active_builder_specs.is_empty()
        && state.view.entries.is_empty()
        && state.artifact_issues.is_empty()
}

fn canonical_source(revision: &crate::memory::CanonicalRevision) -> super::model::CanonicalSourceIdentity {
    super::model::CanonicalSourceIdentity {
        canonical_kind: "memory_revision".to_string(),
        memory_id: revision.memory_id.clone(),
        revision_id: revision.revision_id.clone(),
        revision_sequence: revision.sequence,
        content_hash: revision.content_hash.clone(),
    }
}

fn transition_event(
    entry: &super::model::ProjectionEntry,
    desired_builder_spec_hash: String,
    state: ProjectionState,
) -> Result<ManifestEvent, ProjectionError> {
    Ok(ManifestEvent::ProjectionTransition {
        projection_id: entry.projection_id,
        projection_kind: entry.projection_kind,
        projection_version: entry
            .projection_version
            .checked_add(1)
            .ok_or_else(|| invalid("projection_version_overflow"))?,
        previous_sequence: Some(entry.last_transition_sequence),
        source: entry.source.clone(),
        desired_builder_spec_hash,
        state,
    })
}

fn map_open_error(error: crate::cas::LedgerStorageOpenError) -> ProjectionError {
    match error {
        crate::cas::LedgerStorageOpenError::Io(error) => ProjectionError::Io(error),
        crate::cas::LedgerStorageOpenError::RejectedMarker => invalid("unexpected_projection_marker_check"),
        crate::cas::LedgerStorageOpenError::NamespaceAlreadyClaimed => invalid("projection_namespace_already_claimed"),
        crate::cas::LedgerStorageOpenError::ProjectionResetPending => ProjectionError::ResetPending,
        crate::cas::LedgerStorageOpenError::ProjectionResetMaintenanceRequired => {
            ProjectionError::ProjectionMaintenanceRequired
        }
        crate::cas::LedgerStorageOpenError::ProjectionResetIndeterminate => ProjectionError::CommitIndeterminate,
        crate::cas::LedgerStorageOpenError::ProjectionResetManualIntervention => {
            ProjectionError::ManualInterventionRequired
        }
        crate::cas::LedgerStorageOpenError::UnsupportedProjectionFormat => ProjectionError::UnsupportedFormat {
            component: "projection_store",
        },
    }
}

#[cfg(test)]
fn map_publish_error(error: LedgerStorageError) -> ProjectionError {
    match error {
        LedgerStorageError::Io(error) | LedgerStorageError::PublishedButUnsynced(error) => ProjectionError::Io(error),
    }
}

fn map_pair_publish_error(error: LedgerStorageError) -> ProjectionError {
    match error {
        LedgerStorageError::Io(error) => ProjectionError::Io(error),
        LedgerStorageError::PublishedButUnsynced(_) => ProjectionError::CommitIndeterminate,
    }
}

fn require_size(bytes: &[u8], maximum_bytes: u64) -> Result<(), ProjectionError> {
    if bytes.len() as u64 <= maximum_bytes {
        Ok(())
    } else {
        Err(invalid("projection_object_exceeds_size_limit"))
    }
}
