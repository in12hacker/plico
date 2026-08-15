//! Provider-agnostic, sealed projection coordinator storage boundary.

use std::sync::Arc;

use crate::cas::PersonalVaultStorage;
use crate::memory::ledger::{AuthorizedCurrentRevisionProof, AuthorizedOwnerProjectionProof};
use crate::memory::{CanonicalProjectionGuard, CanonicalProjectionSnapshot};

use super::model::{BuilderSpec, CanonicalSourceIdentity, CanonicalWatermark, FailureCategory, ProjectionError};
use super::store::{
    ProjectionBuildGuard, ProjectionManifestStore, ProjectionStoreInspection, ProjectionStoreUnavailable,
};
use crate::cas::ProjectionPairResetReason;
use crate::memory::{MemoryContent, MemoryRevisionId, MemoryTier};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionCoreInspection {
    Absent,
    GenesisOnly,
    Existing { repair_count: usize },
    Exact { repair_count: usize },
    BuilderMismatch,
    ResetRequired(ProjectionPairResetReason),
    UnsupportedFormat,
    Unavailable(ProjectionCoreUnavailable),
    ResetPending,
    MaintenanceRequired,
    ManualIntervention,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionCoreUnavailable {
    VaultLocked,
    PermissionDenied,
    StorageIo,
    ResourceExhausted,
}

pub(crate) enum ProjectionCoreOpenError {
    NotInitialized,
    BuilderChangeRequiresOwner,
    ResetRequired,
    UnsupportedFormat,
    Unavailable,
    ResetPending,
    MaintenanceRequired,
    ManualIntervention,
    Projection(ProjectionError),
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProjectionRebuildSelector {
    CurrentRevision(MemoryRevisionId),
    AllEligible,
}

pub(crate) enum ProjectionRebuildError {
    NotFound,
    NotEligible,
    NothingToRebuild,
    Projection(ProjectionError),
}

/// Minimal durable acknowledgement for an owner rebuild. It deliberately
/// omits the manifest root hash and all artifact/provider details.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectionDurableReceipt {
    pub(crate) selected_count: u64,
    pub(crate) manifest_generation: u64,
    pub(crate) event_watermark: u64,
    pub(crate) reconciled_source: CanonicalWatermark,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectionCutoverReceipt {
    pub(crate) selected_count: u64,
    pub(crate) manifest_generation: u64,
    pub(crate) event_watermark: u64,
    pub(crate) reconciled_source: CanonicalWatermark,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProjectionStatusState {
    AbsentByPolicy {
        reason: super::model::AbsentReason,
    },
    Queued {
        reason: super::model::QueueReason,
    },
    Building,
    Ready,
    Failed {
        attempt: u32,
        failure_category: FailureCategory,
        retryable: bool,
        retry_not_before: Option<u64>,
    },
    Stale {
        reason: super::model::StaleReason,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProjectionStatusObservation {
    Observed {
        revision_id: MemoryRevisionId,
        content_hash: crate::memory::CanonicalContentHash,
        state: ProjectionStatusState,
        event_watermark: u64,
        reconciled_source: CanonicalWatermark,
    },
    Unreconciled {
        revision_id: MemoryRevisionId,
        content_hash: crate::memory::CanonicalContentHash,
        event_watermark: u64,
        reconciled_source: CanonicalWatermark,
    },
}

/// Content-free claim; no Debug or serialization implementation by design.
pub(crate) struct ProjectionCoreClaim {
    guard: ProjectionBuildGuard,
}

impl ProjectionCoreClaim {
    pub(crate) fn projection_id(&self) -> uuid::Uuid {
        self.guard.projection_id()
    }
    pub(crate) fn source(&self) -> &CanonicalSourceIdentity {
        self.guard.source()
    }

    pub(crate) fn desired_builder_spec_hash(&self) -> &str {
        self.guard.desired_builder_spec_hash()
    }

    pub(crate) fn attempt(&self) -> u32 {
        self.guard.attempt()
    }

    pub(crate) fn claim_event_watermark(&self) -> u64 {
        self.guard.claim_event_watermark()
    }

    pub(crate) fn canonical_watermark(&self) -> &CanonicalWatermark {
        self.guard.canonical_watermark()
    }
}

/// The only projection manifest writer capability exported from memory.
pub(crate) struct ProjectionCoordinatorCore {
    store: Arc<ProjectionManifestStore>,
}

pub(crate) struct ProjectionRecoveredGenesis {
    store: Arc<ProjectionManifestStore>,
}

impl ProjectionRecoveredGenesis {
    #[cfg(test)]
    pub(crate) fn inject_pre_pointer_failure_once(&self) {
        self.store.inject_pre_pointer_failure_once();
    }

    pub(crate) fn resume_authorized(
        &self,
        builder: BuilderSpec,
        selector: ProjectionRebuildSelector,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(ProjectionCoordinatorCore, ProjectionCutoverReceipt), ProjectionError> {
        require_all_eligible_selector(&selector)?;
        let selected_count = u64::try_from(
            select_rebuild_targets(&selector, owner.snapshot())
                .map_err(|_| ProjectionError::Invalid {
                    category: "invalid_projection_cutover_selector",
                })?
                .len(),
        )
        .map_err(|_| ProjectionError::Invalid {
            category: "projection_cutover_count_overflow",
        })?;
        let reset_operation_id = self
            .store
            .reset_operation_id()
            .ok_or(ProjectionError::Invalid {
                category: "projection_reset_operation_id_missing",
            })?
            .to_string();
        self.store.activate_builder(builder, owner.snapshot())?;
        let (_, durable) = self.store.reconcile_with_receipt(owner.snapshot())?;
        tracing::debug!(
            operation = "projection_reset",
            phase = "complete",
            outcome = "ok",
            result_category = "applied",
            reset_operation_id = reset_operation_id.as_str(),
            selected_count,
            manifest_generation = durable.manifest_generation,
            event_watermark = durable.event_watermark,
            reconciled_revision_watermark = durable.reconciled_source.revision_watermark,
            reconciled_policy_watermark = durable.reconciled_source.policy_watermark,
            reconciled_relation_watermark = durable.reconciled_source.relation_watermark
        );
        Ok((
            ProjectionCoordinatorCore {
                store: Arc::clone(&self.store),
            },
            ProjectionCutoverReceipt {
                selected_count,
                manifest_generation: durable.manifest_generation,
                event_watermark: durable.event_watermark,
                reconciled_source: durable.reconciled_source,
            },
        ))
    }
}

impl ProjectionCoordinatorCore {
    pub(crate) fn recover_reset_authorized(
        vault: Arc<PersonalVaultStorage>,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<ProjectionRecoveredGenesis, ProjectionError> {
        let store = ProjectionManifestStore::recover_reset_maintenance(vault, owner.snapshot())?;
        let reset_operation_id = store
            .reset_operation_id()
            .ok_or(ProjectionError::Invalid {
                category: "projection_reset_operation_id_missing",
            })?
            .to_string();
        tracing::debug!(
            operation = "projection_reset",
            phase = "recovery",
            outcome = "ok",
            result_category = "genesis_only",
            reset_operation_id = reset_operation_id.as_str()
        );
        Ok(ProjectionRecoveredGenesis { store })
    }
    pub(crate) fn inspect_existing(
        vault: &PersonalVaultStorage,
        canonical: &CanonicalProjectionSnapshot,
        expected_builder_spec_hash: Option<&str>,
    ) -> Result<ProjectionCoreInspection, ProjectionError> {
        let health = ProjectionManifestStore::inspect_existing_read_only(vault, canonical)?;
        match health {
            ProjectionStoreInspection::Absent => Ok(ProjectionCoreInspection::Absent),
            ProjectionStoreInspection::GenesisOnly => Ok(ProjectionCoreInspection::GenesisOnly),
            ProjectionStoreInspection::ResetRequired(reason) => Ok(ProjectionCoreInspection::ResetRequired(reason)),
            ProjectionStoreInspection::UnsupportedFormat => Ok(ProjectionCoreInspection::UnsupportedFormat),
            ProjectionStoreInspection::Unavailable(category) => {
                Ok(ProjectionCoreInspection::Unavailable(map_unavailable(category)))
            }
            ProjectionStoreInspection::ResetPending => Ok(ProjectionCoreInspection::ResetPending),
            ProjectionStoreInspection::MaintenanceRequired => Ok(ProjectionCoreInspection::MaintenanceRequired),
            ProjectionStoreInspection::ManualIntervention => Ok(ProjectionCoreInspection::ManualIntervention),
            ProjectionStoreInspection::Valid { .. } | ProjectionStoreInspection::RepairRequired { .. }
                if expected_builder_spec_hash.is_none() =>
            {
                Ok(ProjectionCoreInspection::Existing {
                    repair_count: match health {
                        ProjectionStoreInspection::RepairRequired { count } => count,
                        _ => 0,
                    },
                })
            }
            ProjectionStoreInspection::Valid { .. } | ProjectionStoreInspection::RepairRequired { .. } => {
                let expected = expected_builder_spec_hash.expect("guarded by match arm");
                match ProjectionManifestStore::inspect_existing_builder_match(vault, canonical, expected) {
                    Ok(()) => Ok(ProjectionCoreInspection::Exact {
                        repair_count: match health {
                            ProjectionStoreInspection::RepairRequired { count } => count,
                            _ => 0,
                        },
                    }),
                    Err(ProjectionError::Invalid {
                        category: "builder_change_requires_owner",
                    })
                    | Err(ProjectionError::Invalid {
                        category: "projection_not_initialized",
                    }) => Ok(ProjectionCoreInspection::BuilderMismatch),
                    Err(ProjectionError::Invalid { .. } | ProjectionError::Serialization(_)) => Ok(
                        ProjectionCoreInspection::ResetRequired(ProjectionPairResetReason::ManifestIntegrityInvalid),
                    ),
                    Err(error) => Err(error),
                }
            }
        }
    }

    pub(crate) fn bootstrap_authorized(
        vault: Arc<PersonalVaultStorage>,
        builder: BuilderSpec,
        selector: ProjectionRebuildSelector,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionError> {
        require_all_eligible_selector(&selector)?;
        let canonical = owner.snapshot();
        let selected_count = cutover_selected_count(&selector, canonical)?;
        if !matches!(
            Self::inspect_existing(&vault, canonical, None)?,
            ProjectionCoreInspection::Absent
        ) {
            return Err(ProjectionError::Invalid {
                category: "projection_bootstrap_requires_absent",
            });
        }
        let store = ProjectionManifestStore::bootstrap_new(vault, canonical)?;
        store.activate_builder(builder, canonical)?;
        let (_, durable) = store.reconcile_with_receipt(canonical)?;
        Ok((
            Self { store },
            ProjectionCutoverReceipt {
                selected_count,
                manifest_generation: durable.manifest_generation,
                event_watermark: durable.event_watermark,
                reconciled_source: durable.reconciled_source,
            },
        ))
    }

    pub(crate) fn resume_genesis_authorized(
        vault: Arc<PersonalVaultStorage>,
        builder: BuilderSpec,
        selector: ProjectionRebuildSelector,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionError> {
        require_all_eligible_selector(&selector)?;
        let canonical = owner.snapshot();
        let selected_count = cutover_selected_count(&selector, canonical)?;
        match Self::inspect_existing(&vault, canonical, None)? {
            ProjectionCoreInspection::GenesisOnly => {}
            ProjectionCoreInspection::Unavailable(ProjectionCoreUnavailable::VaultLocked) => {
                return Err(ProjectionError::Invalid {
                    category: "projection_namespace_already_claimed",
                });
            }
            _ => {
                return Err(ProjectionError::Invalid {
                    category: "projection_resume_requires_genesis_only",
                });
            }
        }
        let store = ProjectionManifestStore::open_existing_genesis_only(vault, canonical)?;
        store.activate_builder(builder, canonical)?;
        let (_, durable) = store.reconcile_with_receipt(canonical)?;
        Ok((
            Self { store },
            ProjectionCutoverReceipt {
                selected_count,
                manifest_generation: durable.manifest_generation,
                event_watermark: durable.event_watermark,
                reconciled_source: durable.reconciled_source,
            },
        ))
    }

    pub(crate) fn open_existing(
        vault: Arc<PersonalVaultStorage>,
        canonical: &CanonicalProjectionSnapshot,
        expected_builder_spec_hash: &str,
    ) -> Result<Self, ProjectionCoreOpenError> {
        match Self::inspect_existing(&vault, canonical, Some(expected_builder_spec_hash))
            .map_err(ProjectionCoreOpenError::Projection)?
        {
            ProjectionCoreInspection::Absent | ProjectionCoreInspection::GenesisOnly => {
                return Err(ProjectionCoreOpenError::NotInitialized);
            }
            ProjectionCoreInspection::BuilderMismatch => {
                return Err(ProjectionCoreOpenError::BuilderChangeRequiresOwner);
            }
            ProjectionCoreInspection::ResetRequired(_) => {
                return Err(ProjectionCoreOpenError::ResetRequired);
            }
            ProjectionCoreInspection::UnsupportedFormat => {
                return Err(ProjectionCoreOpenError::UnsupportedFormat);
            }
            ProjectionCoreInspection::Unavailable(_) => {
                return Err(ProjectionCoreOpenError::Unavailable);
            }
            ProjectionCoreInspection::ResetPending => return Err(ProjectionCoreOpenError::ResetPending),
            ProjectionCoreInspection::MaintenanceRequired => {
                return Err(ProjectionCoreOpenError::MaintenanceRequired);
            }
            ProjectionCoreInspection::ManualIntervention => {
                return Err(ProjectionCoreOpenError::ManualIntervention);
            }
            ProjectionCoreInspection::Existing { .. } => {
                return Err(ProjectionCoreOpenError::ResetRequired);
            }
            ProjectionCoreInspection::Exact { .. } => {}
        }
        let store =
            ProjectionManifestStore::open_existing_matching_builder(vault, canonical, expected_builder_spec_hash)
                .map_err(|error| match error {
                    ProjectionError::Invalid {
                        category: "builder_change_requires_owner",
                    } => ProjectionCoreOpenError::BuilderChangeRequiresOwner,
                    ProjectionError::Invalid {
                        category: "projection_not_initialized",
                    } => ProjectionCoreOpenError::NotInitialized,
                    other => ProjectionCoreOpenError::Projection(other),
                })?;
        store
            .reconcile(canonical)
            .map_err(ProjectionCoreOpenError::Projection)?;
        Ok(Self { store })
    }

    pub(crate) fn change_builder_authorized(
        vault: Arc<PersonalVaultStorage>,
        builder: BuilderSpec,
        selector: ProjectionRebuildSelector,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionError> {
        require_all_eligible_selector(&selector)?;
        let canonical = owner.snapshot();
        let selected_count = cutover_selected_count(&selector, canonical)?;
        if !matches!(
            Self::inspect_existing(&vault, canonical, None)?,
            ProjectionCoreInspection::Existing { .. }
        ) {
            return Err(ProjectionError::Invalid {
                category: "projection_builder_change_requires_initialized",
            });
        }
        let store = ProjectionManifestStore::open_existing_initialized(vault, canonical)?;
        store.activate_builder(builder, canonical)?;
        let (_, durable) = store.reconcile_with_receipt(canonical)?;
        Ok((
            Self { store },
            ProjectionCutoverReceipt {
                selected_count,
                manifest_generation: durable.manifest_generation,
                event_watermark: durable.event_watermark,
                reconciled_source: durable.reconciled_source,
            },
        ))
    }

    pub(crate) fn reset_authorized(
        vault: Arc<PersonalVaultStorage>,
        builder: BuilderSpec,
        selector: ProjectionRebuildSelector,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionError> {
        require_all_eligible_selector(&selector)?;
        let canonical = owner.snapshot();
        let selected = match select_rebuild_targets(&selector, canonical) {
            Ok(selected) => selected,
            Err(ProjectionRebuildError::NothingToRebuild) => Vec::new(),
            Err(_) => {
                return Err(ProjectionError::Invalid {
                    category: "invalid_projection_cutover_selector",
                });
            }
        };
        let selected_count = u64::try_from(selected.len()).map_err(|_| ProjectionError::Invalid {
            category: "projection_cutover_count_overflow",
        })?;
        let builder_hash = super::hash::builder_spec_bytes_and_hash(&builder)?.1;
        super::validate::validate_builder_hash(&builder, &builder_hash)?;
        let (store, _) = ProjectionManifestStore::reset_required(vault, canonical)?;
        let reset_operation_id = store
            .reset_operation_id()
            .ok_or(ProjectionError::Invalid {
                category: "projection_reset_operation_id_missing",
            })?
            .to_string();
        store.activate_builder(builder, canonical)?;
        let (_, durable) = store.reconcile_with_receipt(canonical)?;
        tracing::debug!(
            operation = "projection_reset",
            phase = "complete",
            outcome = "ok",
            result_category = "applied",
            reset_operation_id = reset_operation_id.as_str(),
            selected_count,
            manifest_generation = durable.manifest_generation,
            event_watermark = durable.event_watermark,
            reconciled_revision_watermark = durable.reconciled_source.revision_watermark,
            reconciled_policy_watermark = durable.reconciled_source.policy_watermark,
            reconciled_relation_watermark = durable.reconciled_source.relation_watermark
        );
        Ok((
            Self { store },
            ProjectionCutoverReceipt {
                selected_count,
                manifest_generation: durable.manifest_generation,
                event_watermark: durable.event_watermark,
                reconciled_source: durable.reconciled_source,
            },
        ))
    }

    pub(crate) fn reconcile(&self, canonical: &CanonicalProjectionSnapshot) -> Result<String, ProjectionError> {
        self.store.reconcile(canonical)
    }

    pub(crate) fn status(
        &self,
        proof: &AuthorizedCurrentRevisionProof<'_>,
    ) -> Result<ProjectionStatusObservation, ProjectionError> {
        let source = proof.source();
        let view = self.store.status_view()?;
        if !proof.reconciled_source_is_ancestor(&view.reconciled_source) {
            return Err(ProjectionError::Invalid {
                category: "projection_status_source_not_canonical_ancestor",
            });
        }
        if let Some(entry) = view.entries.iter().find(|entry| entry.source == *source) {
            return Ok(ProjectionStatusObservation::Observed {
                revision_id: source.revision_id.clone(),
                content_hash: source.content_hash.clone(),
                state: status_state(&entry.state),
                event_watermark: view.event_watermark,
                reconciled_source: view.reconciled_source,
            });
        }
        if source.revision_sequence <= view.reconciled_source.revision_watermark {
            return Err(ProjectionError::Invalid {
                category: "projection_status_covered_revision_missing",
            });
        }
        Ok(ProjectionStatusObservation::Unreconciled {
            revision_id: source.revision_id.clone(),
            content_hash: source.content_hash.clone(),
            event_watermark: view.event_watermark,
            reconciled_source: view.reconciled_source,
        })
    }

    pub(crate) fn owner_rebuild_authorized(
        &self,
        selector: ProjectionRebuildSelector,
        proof: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<ProjectionDurableReceipt, ProjectionRebuildError> {
        let canonical = proof.snapshot();
        let selected = select_rebuild_targets(&selector, canonical)?;
        let selected_count = u64::try_from(selected.len()).map_err(|_| {
            ProjectionRebuildError::Projection(ProjectionError::Invalid {
                category: "projection_rebuild_count_overflow",
            })
        })?;
        self.store
            .reconcile(canonical)
            .map_err(ProjectionRebuildError::Projection)?;
        let receipt = self
            .store
            .owner_rebuild(&selected, canonical)
            .map_err(ProjectionRebuildError::Projection)?;
        Ok(ProjectionDurableReceipt {
            selected_count,
            manifest_generation: receipt.manifest_generation,
            event_watermark: receipt.event_watermark,
            reconciled_source: receipt.reconciled_source,
        })
    }

    pub(crate) fn claim_next(
        &self,
        canonical: &CanonicalProjectionSnapshot,
    ) -> Result<Option<ProjectionCoreClaim>, ProjectionError> {
        Ok(self
            .store
            .claim_next(canonical)?
            .map(|guard| ProjectionCoreClaim { guard }))
    }

    pub(crate) fn claim_is_current(&self, claim: &ProjectionCoreClaim) -> Result<bool, ProjectionError> {
        self.store.guard_is_current(&claim.guard)
    }

    pub(crate) fn output_is_valid(&self, claim: &ProjectionCoreClaim, vector: &[f32]) -> Result<bool, ProjectionError> {
        self.store.worker_output_is_valid(&claim.guard, vector)
    }

    pub(crate) fn complete_ready(
        &self,
        claim: &ProjectionCoreClaim,
        vector: Vec<f32>,
        canonical: &CanonicalProjectionGuard<'_>,
    ) -> Result<String, ProjectionError> {
        self.store.complete_ready(&claim.guard, vector, canonical)
    }

    pub(crate) fn complete_failed(
        &self,
        claim: &ProjectionCoreClaim,
        category: FailureCategory,
        canonical: &CanonicalProjectionGuard<'_>,
    ) -> Result<String, ProjectionError> {
        self.store.complete_failed(&claim.guard, category, canonical)
    }

    #[cfg(test)]
    pub(crate) fn current_view_for_test(&self) -> Result<super::model::ProjectionCurrentView, ProjectionError> {
        self.store.current_view_for_test()
    }

    #[cfg(test)]
    pub(crate) fn root_chain_for_test(&self) -> Result<Vec<super::model::ProjectionRoot>, ProjectionError> {
        self.store.root_chain_for_test()
    }

    #[cfg(test)]
    pub(crate) fn inject_post_artifact_durability_failure_once(&self) {
        self.store.inject_post_artifact_durability_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_pre_pointer_failure_once(&self) {
        self.store.inject_pre_pointer_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_post_exchange_sync_failure_once(&self) {
        self.store.inject_post_exchange_sync_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_status_reconciled_source_for_test(&self, source: CanonicalWatermark) {
        self.store.inject_status_reconciled_source_for_test(source);
    }

    #[cfg(test)]
    pub(crate) fn bootstrap_genesis_only_for_test(
        vault: Arc<PersonalVaultStorage>,
        owner: &AuthorizedOwnerProjectionProof<'_>,
    ) -> Result<(), ProjectionError> {
        let canonical = owner.snapshot();
        let store = ProjectionManifestStore::bootstrap_new(vault, canonical)?;
        drop(store);
        Ok(())
    }
}

fn map_unavailable(category: ProjectionStoreUnavailable) -> ProjectionCoreUnavailable {
    match category {
        ProjectionStoreUnavailable::VaultLocked => ProjectionCoreUnavailable::VaultLocked,
        ProjectionStoreUnavailable::PermissionDenied => ProjectionCoreUnavailable::PermissionDenied,
        ProjectionStoreUnavailable::StorageIo => ProjectionCoreUnavailable::StorageIo,
        ProjectionStoreUnavailable::ResourceExhausted => ProjectionCoreUnavailable::ResourceExhausted,
    }
}

fn require_all_eligible_selector(selector: &ProjectionRebuildSelector) -> Result<(), ProjectionError> {
    if matches!(selector, ProjectionRebuildSelector::AllEligible) {
        Ok(())
    } else {
        Err(ProjectionError::AllEligibleRequired)
    }
}

fn cutover_selected_count(
    selector: &ProjectionRebuildSelector,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<u64, ProjectionError> {
    let selected = match select_rebuild_targets(selector, canonical) {
        Ok(selected) => selected,
        Err(ProjectionRebuildError::NothingToRebuild) => Vec::new(),
        Err(_) => {
            return Err(ProjectionError::Invalid {
                category: "invalid_projection_cutover_selector",
            });
        }
    };
    u64::try_from(selected.len()).map_err(|_| ProjectionError::Invalid {
        category: "projection_cutover_count_overflow",
    })
}

fn select_rebuild_targets(
    selector: &ProjectionRebuildSelector,
    canonical: &CanonicalProjectionSnapshot,
) -> Result<Vec<MemoryRevisionId>, ProjectionRebuildError> {
    match selector {
        ProjectionRebuildSelector::CurrentRevision(revision_id) => {
            let revision = current_revision(canonical, revision_id).ok_or(ProjectionRebuildError::NotFound)?;
            if revision.deleted_at.is_some() {
                return Err(ProjectionRebuildError::NotFound);
            }
            if !revision_is_eligible(revision) {
                return Err(ProjectionRebuildError::NotEligible);
            }
            Ok(vec![revision.revision_id.clone()])
        }
        ProjectionRebuildSelector::AllEligible => {
            let child_revision_ids: std::collections::HashSet<_> = canonical
                .revisions
                .iter()
                .filter_map(|revision| revision.parent_revision_id.clone())
                .collect();
            let selected: Vec<_> = canonical
                .revisions
                .iter()
                .filter(|revision| !child_revision_ids.contains(&revision.revision_id))
                .filter(|revision| revision_is_eligible(revision))
                .map(|revision| revision.revision_id.clone())
                .collect();
            if selected.is_empty() {
                Err(ProjectionRebuildError::NothingToRebuild)
            } else {
                Ok(selected)
            }
        }
    }
}

fn current_revision<'a>(
    canonical: &'a CanonicalProjectionSnapshot,
    revision_id: &MemoryRevisionId,
) -> Option<&'a crate::memory::CanonicalRevision> {
    let revision = canonical
        .revisions
        .iter()
        .find(|revision| &revision.revision_id == revision_id)?;
    (!canonical
        .revisions
        .iter()
        .any(|candidate| candidate.parent_revision_id.as_ref() == Some(revision_id)))
    .then_some(revision)
}

fn revision_is_eligible(revision: &crate::memory::CanonicalRevision) -> bool {
    revision.deleted_at.is_none()
        && matches!(revision.cognitive_tier, MemoryTier::Working | MemoryTier::LongTerm)
        && matches!(&revision.content, MemoryContent::Text(text) if !text.trim().is_empty())
}

fn status_state(state: &super::model::ProjectionState) -> ProjectionStatusState {
    match state {
        super::model::ProjectionState::AbsentByPolicy { reason } => {
            ProjectionStatusState::AbsentByPolicy { reason: *reason }
        }
        super::model::ProjectionState::Queued { reason } => ProjectionStatusState::Queued { reason: *reason },
        super::model::ProjectionState::Building { .. } => ProjectionStatusState::Building,
        super::model::ProjectionState::Ready { .. } => ProjectionStatusState::Ready,
        super::model::ProjectionState::Failed {
            attempt,
            failure_category,
            retryable,
            retry_not_before,
            ..
        } => ProjectionStatusState::Failed {
            attempt: *attempt,
            failure_category: *failure_category,
            retryable: *retryable,
            retry_not_before: *retry_not_before,
        },
        super::model::ProjectionState::Stale { reason, .. } => ProjectionStatusState::Stale { reason: *reason },
    }
}
