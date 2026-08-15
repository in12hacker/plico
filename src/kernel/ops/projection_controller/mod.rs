//! Kernel orchestration around the provider-agnostic projection core.

use std::sync::{mpsc, Arc, Mutex};

use uuid::Uuid;

use crate::cas::PersonalVaultStorage;
use crate::fs::{
    EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingInputContract as ProviderInputContract,
    EmbeddingNormalization as ProviderNormalization, VerifiedDocumentProviderSnapshot,
};
use crate::memory::projection::{
    builder_spec_bytes_and_hash, BuilderSpec, CanonicalWatermark, EmbeddingInputContract, EmbeddingNormalization,
    EmbeddingOperationContract, FailureCategory, ProjectionCoordinatorCore, ProjectionCoreClaim,
    ProjectionCoreInspection, ProjectionCoreOpenError, ProjectionCutoverReceipt, ProjectionDurableReceipt,
    ProjectionError, ProjectionKind, ProjectionRebuildError, ProjectionRebuildSelector, ProjectionRecoveredGenesis,
    ProjectionStatusObservation, BUILDER_SPEC_SCHEMA, EMBEDDING_ARTIFACT_SCHEMA,
};
use crate::memory::{
    CASCanonicalLedger, CanonicalContentHash, CanonicalProjectionSource, LedgerError, MemoryId, MemoryRevisionId,
};

const WAKE_CAPACITY: usize = 1;
const BUILDER_ID: &str = "plico.memory-embedding";
const BUILDER_VERSION: &str = "p3a-controller-v1";

pub(crate) struct ProjectionUnavailableInspection {
    pub(crate) inspection: ProjectionCoreInspection,
    pub(crate) identity_category: &'static str,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionControllerError {
    #[error("canonical projection source unavailable: {category}")]
    Canonical { category: &'static str },
    #[error("projection control plane failure")]
    Projection(#[from] ProjectionError),
    #[error("projection is not initialized")]
    NotInitialized,
    #[error("projection builder change requires owner cutover")]
    BuilderChangeRequiresOwner,
    #[error("projection manifest is unavailable")]
    ProjectionUnavailable,
    #[error("projection revision was not found")]
    RebuildNotFound,
    #[error("projection revision is not eligible")]
    RebuildNotEligible,
    #[error("no eligible projection revision exists")]
    NothingToRebuild,
    #[error("projection provider identity changed; restart is required")]
    ProviderIdentityChanged,
}

pub(crate) struct ProjectionResumeRecoveredError {
    pub(crate) recovered: ProjectionRecoveredGenesis,
    pub(crate) error: ProjectionControllerError,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionBuildOutcome {
    Ready,
    Failed(FailureCategory),
    Discarded,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDisposition {
    Queued,
    Full,
    WorkerUnavailable,
}

/// Bounded wake hint. No Debug/serde implementation by design.
pub(crate) struct ProjectionWake {
    run_id: Uuid,
    request_id: Option<Uuid>,
    memory_id: MemoryId,
    revision_id: MemoryRevisionId,
    content_hash: CanonicalContentHash,
    expected_spec_hash: String,
    canonical_watermark: CanonicalWatermark,
}

struct ProjectionTurnContext {
    wake: Option<ProjectionWake>,
    run_id: Uuid,
    request_id: Option<Uuid>,
}

/// Content-free attempt. No Debug/serde implementation by design.
pub(crate) struct ProjectionBuildJob {
    claim: ProjectionCoreClaim,
    provider: VerifiedDocumentProviderSnapshot,
    run_id: Uuid,
    request_id: Option<Uuid>,
    claim_manifest_watermark: u64,
    claim_canonical_watermark: CanonicalWatermark,
}

pub(crate) struct ProjectionCoordinator {
    canonical: Arc<CASCanonicalLedger>,
    core: ProjectionCoordinatorCore,
    provider: VerifiedDocumentProviderSnapshot,
    builder_spec_hash: String,
    wake_sender: mpsc::SyncSender<ProjectionWake>,
    wake_receiver: Mutex<mpsc::Receiver<ProjectionWake>>,
}

impl ProjectionCoordinator {
    pub(crate) fn inspect_verified(
        vault: &PersonalVaultStorage,
        canonical: &CASCanonicalLedger,
        provider: &VerifiedDocumentProviderSnapshot,
    ) -> Result<ProjectionCoreInspection, ProjectionControllerError> {
        let snapshot = canonical.projection_snapshot().map_err(map_canonical_error)?;
        let spec_hash = builder_spec_bytes_and_hash(&builder_spec(provider.identity()))?.1;
        ProjectionCoordinatorCore::inspect_existing(vault, &snapshot, Some(&spec_hash)).map_err(Into::into)
    }

    pub(crate) fn inspect_identity_unavailable(
        vault: &PersonalVaultStorage,
        canonical: &CASCanonicalLedger,
        identity_error: EmbeddingIdentityError,
    ) -> Result<ProjectionUnavailableInspection, ProjectionControllerError> {
        let snapshot = canonical.projection_snapshot().map_err(map_canonical_error)?;
        Ok(ProjectionUnavailableInspection {
            inspection: ProjectionCoordinatorCore::inspect_existing(vault, &snapshot, None)?,
            identity_category: identity_error.category(),
        })
    }

    pub(crate) fn bootstrap_for_owner(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionControllerError> {
        provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)?;
        let spec = builder_spec(provider.identity());
        let spec_hash = builder_spec_bytes_and_hash(&spec)?.1;
        let (core, receipt) = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::bootstrap_authorized(
                    vault,
                    spec,
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)??;
        Ok((Self::new(canonical, core, provider, spec_hash), receipt))
    }

    pub(crate) fn resume_genesis_for_owner(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionControllerError> {
        provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)?;
        let spec = builder_spec(provider.identity());
        let spec_hash = builder_spec_bytes_and_hash(&spec)?.1;
        let (core, receipt) = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::resume_genesis_authorized(
                    vault,
                    spec,
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)??;
        Ok((Self::new(canonical, core, provider, spec_hash), receipt))
    }

    pub(crate) fn open_existing(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<Self, ProjectionControllerError> {
        provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)?;
        let snapshot = canonical.projection_snapshot().map_err(map_canonical_error)?;
        let spec = builder_spec(provider.identity());
        let spec_hash = builder_spec_bytes_and_hash(&spec)?.1;
        let core =
            ProjectionCoordinatorCore::open_existing(vault, &snapshot, &spec_hash).map_err(|error| match error {
                ProjectionCoreOpenError::NotInitialized => ProjectionControllerError::NotInitialized,
                ProjectionCoreOpenError::BuilderChangeRequiresOwner => {
                    ProjectionControllerError::BuilderChangeRequiresOwner
                }
                ProjectionCoreOpenError::ResetRequired
                | ProjectionCoreOpenError::UnsupportedFormat
                | ProjectionCoreOpenError::Unavailable
                | ProjectionCoreOpenError::ResetPending
                | ProjectionCoreOpenError::MaintenanceRequired
                | ProjectionCoreOpenError::ManualIntervention => ProjectionControllerError::ProjectionUnavailable,
                ProjectionCoreOpenError::Projection(error) => ProjectionControllerError::Projection(error),
            })?;
        Ok(Self::new(canonical, core, provider, spec_hash))
    }

    pub(crate) fn change_builder_for_owner(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionControllerError> {
        provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)?;
        let spec = builder_spec(provider.identity());
        let spec_hash = builder_spec_bytes_and_hash(&spec)?.1;
        let (core, receipt) = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::change_builder_authorized(
                    vault,
                    spec,
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)??;
        Ok((Self::new(canonical, core, provider, spec_hash), receipt))
    }

    pub(crate) fn reset_for_owner(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionControllerError> {
        provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)?;
        let spec = builder_spec(provider.identity());
        let spec_hash = builder_spec_bytes_and_hash(&spec)?.1;
        let (core, receipt) = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::reset_authorized(vault, spec, ProjectionRebuildSelector::AllEligible, &proof)
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)??;
        Ok((Self::new(canonical, core, provider, spec_hash), receipt))
    }

    fn new(
        canonical: Arc<CASCanonicalLedger>,
        core: ProjectionCoordinatorCore,
        provider: VerifiedDocumentProviderSnapshot,
        builder_spec_hash: String,
    ) -> Self {
        let (wake_sender, wake_receiver) = mpsc::sync_channel(WAKE_CAPACITY);
        Self {
            canonical,
            core,
            provider,
            builder_spec_hash,
            wake_sender,
            wake_receiver: Mutex::new(wake_receiver),
        }
    }

    pub(crate) fn status_authorized(
        &self,
        trusted_role: &str,
        revision_id: &MemoryRevisionId,
    ) -> Result<Option<ProjectionStatusObservation>, ProjectionControllerError> {
        self.canonical
            .with_authorized_current_revision(trusted_role, revision_id, |proof| self.core.status(&proof))
            .map_err(map_canonical_error)?
            .transpose()
            .map_err(Into::into)
    }

    pub(crate) fn owner_rebuild(
        &self,
        selector: ProjectionRebuildSelector,
    ) -> Result<ProjectionDurableReceipt, ProjectionControllerError> {
        let result = self
            .canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                self.core.owner_rebuild_authorized(selector, &proof)
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)?;
        match result {
            Ok(receipt) => Ok(receipt),
            Err(ProjectionRebuildError::NotFound) => Err(ProjectionControllerError::RebuildNotFound),
            Err(ProjectionRebuildError::NotEligible) => Err(ProjectionControllerError::RebuildNotEligible),
            Err(ProjectionRebuildError::NothingToRebuild) => Err(ProjectionControllerError::NothingToRebuild),
            Err(ProjectionRebuildError::Projection(error)) => Err(error.into()),
        }
    }

    pub(crate) fn revalidate_provider(&self) -> Result<(), ProjectionControllerError> {
        self.provider
            .revalidate()
            .map_err(|_| ProjectionControllerError::ProviderIdentityChanged)
    }

    #[cfg(test)]
    pub(crate) fn inject_manifest_pre_pointer_failure_once(&self) {
        self.core.inject_pre_pointer_failure_once();
    }

    #[cfg(test)]
    pub(crate) fn inject_manifest_post_exchange_sync_failure_once(&self) {
        self.core.inject_post_exchange_sync_failure_once();
    }

    pub(crate) fn recover_reset_for_owner(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
    ) -> Result<ProjectionRecoveredGenesis, ProjectionControllerError> {
        canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                ProjectionCoordinatorCore::recover_reset_authorized(vault, &proof)
            })
            .map_err(map_canonical_error)?
            .ok_or(ProjectionControllerError::ProjectionUnavailable)?
            .map_err(Into::into)
    }

    pub(crate) fn resume_recovered_for_owner(
        recovered: ProjectionRecoveredGenesis,
        canonical: Arc<CASCanonicalLedger>,
        provider: VerifiedDocumentProviderSnapshot,
    ) -> Result<(Self, ProjectionCutoverReceipt), ProjectionResumeRecoveredError> {
        if provider.revalidate().is_err() {
            return Err(ProjectionResumeRecoveredError {
                recovered,
                error: ProjectionControllerError::ProviderIdentityChanged,
            });
        }
        let spec = builder_spec(provider.identity());
        let spec_hash = match builder_spec_bytes_and_hash(&spec) {
            Ok((_, hash)) => hash,
            Err(error) => {
                return Err(ProjectionResumeRecoveredError {
                    recovered,
                    error: error.into(),
                });
            }
        };
        let resumed = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                recovered.resume_authorized(spec, ProjectionRebuildSelector::AllEligible, &proof)
            })
            .map_err(map_canonical_error);
        let (core, receipt) = match resumed {
            Ok(Some(Ok(result))) => result,
            Ok(Some(Err(error))) => {
                return Err(ProjectionResumeRecoveredError {
                    recovered,
                    error: error.into(),
                });
            }
            Ok(None) => {
                return Err(ProjectionResumeRecoveredError {
                    recovered,
                    error: ProjectionControllerError::ProjectionUnavailable,
                });
            }
            Err(error) => {
                return Err(ProjectionResumeRecoveredError { recovered, error });
            }
        };
        Ok((Self::new(canonical, core, provider, spec_hash), receipt))
    }

    pub(crate) fn wake_for_current(
        &self,
        memory_id: MemoryId,
        revision_id: MemoryRevisionId,
        content_hash: CanonicalContentHash,
        request_id: Option<Uuid>,
    ) -> Result<ProjectionWake, ProjectionControllerError> {
        let snapshot = self.canonical.projection_snapshot().map_err(map_canonical_error)?;
        Ok(ProjectionWake {
            run_id: Uuid::new_v4(),
            request_id,
            memory_id,
            revision_id,
            content_hash,
            expected_spec_hash: self.builder_spec_hash.clone(),
            canonical_watermark: watermark(&snapshot),
        })
    }

    pub(crate) fn notify(&self, wake: ProjectionWake) -> WakeDisposition {
        let run_id = wake.run_id;
        let request_id = wake.request_id;
        match self.wake_sender.try_send(wake) {
            Ok(()) => {
                projection_wake_trace("queued", None, run_id, request_id);
                WakeDisposition::Queued
            }
            Err(mpsc::TrySendError::Full(_)) => {
                projection_wake_trace("queue_full", None, run_id, request_id);
                WakeDisposition::Full
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                projection_wake_trace(
                    "worker_unavailable",
                    Some("wake_worker_unavailable"),
                    run_id,
                    request_id,
                );
                WakeDisposition::WorkerUnavailable
            }
        }
    }

    pub(crate) fn reconcile_once(&self) -> Result<(), ProjectionControllerError> {
        let snapshot = self.canonical.projection_snapshot().map_err(map_canonical_error)?;
        self.core.reconcile(&snapshot)?;
        Ok(())
    }

    pub(crate) fn reconcile_startup(&self) -> Result<(), ProjectionControllerError> {
        let turn = ProjectionTurnContext {
            wake: None,
            run_id: Uuid::new_v4(),
            request_id: None,
        };
        let span = projection_turn_span(&turn);
        let _entered = span.enter();
        projection_trace("startup_reconcile", "started", None);
        self.reconcile_once().inspect_err(|error| {
            projection_trace("startup_reconcile", "failed", Some(controller_error_category(error)));
        })?;
        projection_trace("startup_reconcile", "complete", None);
        Ok(())
    }

    pub(crate) fn reconcile_and_claim_one(&self) -> Result<Option<ProjectionBuildJob>, ProjectionControllerError> {
        let turn = self.take_turn_context();
        let span = projection_turn_span(&turn);
        let _entered = span.enter();
        projection_trace("reconcile", "started", None);
        self.reconcile_once().inspect_err(|error| {
            projection_trace("reconcile", "failed", Some(controller_error_category(error)));
        })?;
        projection_trace("reconcile", "complete", None);
        self.claim_one_in_turn(turn)
    }

    #[cfg(test)]
    pub(crate) fn claim_one(&self) -> Result<Option<ProjectionBuildJob>, ProjectionControllerError> {
        let turn = self.take_turn_context();
        let span = projection_turn_span(&turn);
        let _entered = span.enter();
        self.claim_one_in_turn(turn)
    }

    fn take_turn_context(&self) -> ProjectionTurnContext {
        let wake = self
            .wake_receiver
            .lock()
            .ok()
            .and_then(|receiver| receiver.try_recv().ok());
        let turn_run_id = wake.as_ref().map_or_else(Uuid::new_v4, |wake| wake.run_id);
        let turn_request_id = wake.as_ref().and_then(|wake| wake.request_id);
        ProjectionTurnContext {
            wake,
            run_id: turn_run_id,
            request_id: turn_request_id,
        }
    }

    fn claim_one_in_turn(
        &self,
        turn: ProjectionTurnContext,
    ) -> Result<Option<ProjectionBuildJob>, ProjectionControllerError> {
        projection_trace("canonical_snapshot", "started", None);
        let snapshot = self.canonical.projection_snapshot().map_err(|error| {
            projection_trace(
                "canonical_snapshot",
                "unavailable",
                Some(map_canonical_error_category(&error)),
            );
            map_canonical_error(error)
        })?;
        projection_trace("canonical_snapshot", "verified", None);
        projection_trace("manifest_claim", "started", None);
        let Some(claim) = self.core.claim_next(&snapshot).map_err(|error| {
            projection_trace("manifest_claim", "failed", Some(projection_error_category(&error)));
            ProjectionControllerError::Projection(error)
        })?
        else {
            projection_trace("manifest_claim", "idle", None);
            return Ok(None);
        };
        let (run_id, request_id) = match turn.wake {
            Some(wake)
                if wake.memory_id == claim.source().memory_id
                    && wake.revision_id == claim.source().revision_id
                    && wake.content_hash == claim.source().content_hash
                    && wake.expected_spec_hash == claim.desired_builder_spec_hash()
                    && wake.canonical_watermark == *claim.canonical_watermark() =>
            {
                (wake.run_id, wake.request_id)
            }
            Some(wake) => {
                projection_wake_trace("discarded", Some("wake_claim_mismatch"), wake.run_id, wake.request_id);
                (Uuid::new_v4(), None)
            }
            None => (Uuid::new_v4(), None),
        };
        projection_trace_with_claim("manifest_claim", "building", None, &claim, run_id, request_id);
        Ok(Some(ProjectionBuildJob {
            claim_manifest_watermark: claim.claim_event_watermark(),
            claim_canonical_watermark: claim.canonical_watermark().clone(),
            claim,
            provider: self.provider.clone(),
            run_id,
            request_id,
        }))
    }

    #[cfg(test)]
    pub(crate) fn complete_one(
        &self,
        job: ProjectionBuildJob,
    ) -> Result<ProjectionBuildOutcome, ProjectionControllerError> {
        self.complete_one_interruptible(job, || false)
    }

    pub(crate) fn complete_one_interruptible(
        &self,
        job: ProjectionBuildJob,
        cancelled: impl Fn() -> bool,
    ) -> Result<ProjectionBuildOutcome, ProjectionControllerError> {
        let request_id = job
            .request_id
            .map(|value| value.hyphenated().to_string())
            .unwrap_or_else(|| "none".to_string());
        let operation = if job.request_id.is_some() {
            "memory.projection_build"
        } else {
            "memory.projection_worker"
        };
        let span = tracing::info_span!(
            "projection_build",
            operation,
            phase = tracing::field::Empty,
            result_category = tracing::field::Empty,
            run_id = %job.run_id,
            request_id = %request_id,
            revision_id = %job.claim.source().revision_id,
            projection_id = %job.claim.projection_id(),
            attempt = job.claim.attempt(),
            manifest_watermark = job.claim_manifest_watermark,
            canonical_generation = job.claim_canonical_watermark.generation,
            canonical_revision_watermark = job.claim_canonical_watermark.revision_watermark,
        );
        let _entered = span.enter();
        projection_trace("claim_validation", "started", None);
        let claim_is_current = self.core.claim_is_current(&job.claim).map_err(|error| {
            projection_trace("claim_validation", "failed", Some(projection_error_category(&error)));
            ProjectionControllerError::Projection(error)
        })?;
        if !claim_is_current {
            projection_trace("claim_validation", "discarded", Some("claim_not_current"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        projection_trace("claim_validation", "verified", None);
        projection_trace("canonical_document", "started", None);
        let document = self
            .canonical
            .guarded_projection_document(job.claim.source())
            .map_err(|error| {
                projection_trace(
                    "canonical_document",
                    "unavailable",
                    Some(map_canonical_error_category(&error)),
                );
                map_canonical_error(error)
            })?;
        let Some(document) = document else {
            self.reconcile_after_discard("canonical_source_not_current")?;
            return Ok(ProjectionBuildOutcome::Discarded);
        };
        projection_trace("canonical_document", "verified", None);
        projection_trace("provider_precheck", "started", None);
        if cancelled() {
            projection_trace("provider_precheck", "discarded", Some("worker_shutdown"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        if job.provider.revalidate().is_err() {
            if cancelled() {
                projection_trace("provider_precheck", "discarded", Some("worker_shutdown"));
                return Ok(ProjectionBuildOutcome::Discarded);
            }
            projection_trace("provider_precheck", "unavailable", Some("provider_identity_changed"));
            return self.complete_failure(&job.claim, FailureCategory::ProviderIdentityChanged);
        }
        projection_trace("provider_precheck", "verified", None);
        if cancelled() {
            projection_trace("provider_embed", "discarded", Some("worker_shutdown"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        projection_trace("provider_embed", "started", None);
        let vector = match job.provider.embed_document(&document) {
            Ok(result) => {
                projection_trace("provider_embed", "completed", None);
                result.embedding
            }
            Err(_) => {
                if cancelled() {
                    projection_trace("provider_embed", "discarded", Some("worker_shutdown"));
                    return Ok(ProjectionBuildOutcome::Discarded);
                }
                projection_trace("provider_embed", "unavailable", Some("provider_call_failed"));
                return self.complete_failure(&job.claim, FailureCategory::ProviderUnavailable);
            }
        };
        drop(document);
        if cancelled() {
            projection_trace("provider_embed", "discarded", Some("worker_shutdown"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        projection_trace("provider_postcheck", "started", None);
        if job.provider.revalidate().is_err() {
            if cancelled() {
                projection_trace("provider_postcheck", "discarded", Some("worker_shutdown"));
                return Ok(ProjectionBuildOutcome::Discarded);
            }
            projection_trace("provider_postcheck", "unavailable", Some("provider_identity_changed"));
            return self.complete_failure(&job.claim, FailureCategory::ProviderIdentityChanged);
        }
        projection_trace("provider_postcheck", "verified", None);
        if cancelled() {
            projection_trace("output_validation", "discarded", Some("worker_shutdown"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        projection_trace("output_validation", "started", None);
        match self.core.output_is_valid(&job.claim, &vector) {
            Ok(true) => projection_trace("output_validation", "verified", None),
            Ok(false) => {
                projection_trace("output_validation", "invalid", Some("invalid_projection"));
                return self.complete_failure(&job.claim, FailureCategory::InvalidProjection);
            }
            Err(ProjectionError::HeadConflict) => {
                projection_trace(
                    "output_validation",
                    "discarded",
                    Some("claim_changed_before_validation"),
                );
                return Ok(ProjectionBuildOutcome::Discarded);
            }
            Err(error) => {
                projection_trace("output_validation", "failed", Some(projection_error_category(&error)));
                return Err(error.into());
            }
        }
        if cancelled() {
            projection_trace("final_canonical_guard", "discarded", Some("worker_shutdown"));
            return Ok(ProjectionBuildOutcome::Discarded);
        }
        projection_trace("final_canonical_guard", "started", None);
        let completed = self
            .canonical
            .with_current_projection_source(job.claim.source(), |canonical_guard| {
                projection_trace("final_canonical_guard", "acquired", None);
                self.core.complete_ready(&job.claim, vector, &canonical_guard)
            })
            .map_err(|error| {
                projection_trace(
                    "final_canonical_guard",
                    "unavailable",
                    Some(map_canonical_error_category(&error)),
                );
                map_canonical_error(error)
            })?;
        match completed {
            Some(Ok(_)) => {
                projection_trace("complete", "ready", None);
                Ok(ProjectionBuildOutcome::Ready)
            }
            Some(Err(ProjectionError::ArtifactStoreUnavailable)) => {
                self.complete_failure(&job.claim, FailureCategory::ArtifactStoreUnavailable)
            }
            Some(Err(ProjectionError::HeadConflict)) | None => {
                self.reconcile_after_discard("final_guard_changed")?;
                Ok(ProjectionBuildOutcome::Discarded)
            }
            Some(Err(error)) => {
                projection_trace("complete", "unavailable", Some(projection_error_category(&error)));
                Err(error.into())
            }
        }
    }

    fn reconcile_after_discard(&self, reason: &'static str) -> Result<(), ProjectionControllerError> {
        projection_trace("reconcile", "started", Some(reason));
        if let Err(error) = self.reconcile_once() {
            projection_trace("reconcile", "failed", Some(controller_error_category(&error)));
            return Err(error);
        }
        projection_trace("complete", "discarded", Some(reason));
        Ok(())
    }

    fn complete_failure(
        &self,
        claim: &ProjectionCoreClaim,
        category: FailureCategory,
    ) -> Result<ProjectionBuildOutcome, ProjectionControllerError> {
        projection_trace("failure_publish", "started", Some(failure_category(category)));
        let completed = self
            .canonical
            .with_current_projection_source(claim.source(), |canonical_guard| {
                projection_trace("final_canonical_guard", "acquired", None);
                self.core.complete_failed(claim, category, &canonical_guard)
            })
            .map_err(|error| {
                projection_trace(
                    "failure_publish",
                    "unavailable",
                    Some(map_canonical_error_category(&error)),
                );
                map_canonical_error(error)
            })?;
        match completed {
            Some(Ok(_)) => {
                projection_trace("complete", "failed", Some(failure_category(category)));
                Ok(ProjectionBuildOutcome::Failed(category))
            }
            Some(Err(ProjectionError::HeadConflict)) | None => {
                self.reconcile_after_discard("failure_guard_changed")?;
                Ok(ProjectionBuildOutcome::Discarded)
            }
            Some(Err(error)) => {
                projection_trace("failure_publish", "failed", Some(projection_error_category(&error)));
                Err(error.into())
            }
        }
    }
}

fn projection_trace(phase: &'static str, result_category: &'static str, reason: Option<&'static str>) {
    tracing::debug!(phase, result_category, reason = reason.unwrap_or("none"));
}

fn projection_trace_with_claim(
    phase: &'static str,
    result_category: &'static str,
    reason: Option<&'static str>,
    claim: &ProjectionCoreClaim,
    run_id: Uuid,
    request_id: Option<Uuid>,
) {
    let operation = if request_id.is_some() {
        "memory.projection_build"
    } else {
        "memory.projection_worker"
    };
    let request_id = request_id
        .map(|value| value.hyphenated().to_string())
        .unwrap_or_else(|| "none".to_string());
    tracing::debug!(
        operation,
        phase,
        result_category,
        reason = reason.unwrap_or("none"),
        run_id = %run_id,
        request_id = %request_id,
        revision_id = %claim.source().revision_id,
        projection_id = %claim.projection_id(),
        attempt = claim.attempt(),
        manifest_watermark = claim.claim_event_watermark(),
        canonical_generation = claim.canonical_watermark().generation,
        canonical_revision_watermark = claim.canonical_watermark().revision_watermark,
    );
}

fn projection_wake_trace(
    result_category: &'static str,
    reason: Option<&'static str>,
    run_id: Uuid,
    request_id: Option<Uuid>,
) {
    let operation = if request_id.is_some() {
        "memory.projection_build"
    } else {
        "memory.projection_worker"
    };
    let request_id = request_id
        .map(|value| value.hyphenated().to_string())
        .unwrap_or_else(|| "none".to_string());
    tracing::debug!(
        operation,
        phase = "wake",
        result_category,
        reason = reason.unwrap_or("none"),
        run_id = %run_id,
        request_id = %request_id,
    );
}

fn projection_turn_span(turn: &ProjectionTurnContext) -> tracing::Span {
    let request_id = turn
        .request_id
        .map(|value| value.hyphenated().to_string())
        .unwrap_or_else(|| "none".to_string());
    let operation = if turn.request_id.is_some() {
        "memory.projection_build"
    } else {
        "memory.projection_worker"
    };
    tracing::debug_span!(
        "projection_turn",
        operation,
        run_id = %turn.run_id,
        request_id = %request_id,
    )
}

fn projection_error_category(error: &ProjectionError) -> &'static str {
    match error {
        ProjectionError::Io(_) => "projection_io",
        ProjectionError::Serialization(_) | ProjectionError::Invalid { .. } => "projection_invalid",
        ProjectionError::UnsupportedFormat { .. } => "projection_unsupported_format",
        ProjectionError::AllEligibleRequired => "projection_all_eligible_required",
        ProjectionError::ManualInterventionRequired => "projection_manual_intervention_required",
        ProjectionError::HeadConflict => "head_conflict",
        ProjectionError::CommitIndeterminate => "commit_indeterminate",
        ProjectionError::WriterPoisoned => "writer_poisoned",
        ProjectionError::ArtifactRepairRequired { .. } => "artifact_repair_required",
        ProjectionError::ArtifactMaintenanceRequired => "artifact_maintenance_required",
        ProjectionError::ProjectionMaintenanceRequired => "projection_maintenance_required",
        ProjectionError::ResetPending => "projection_reset_pending",
        ProjectionError::ArtifactStoreUnavailable => "artifact_store_unavailable",
    }
}

fn controller_error_category(error: &ProjectionControllerError) -> &'static str {
    match error {
        ProjectionControllerError::Canonical { category } => category,
        ProjectionControllerError::Projection(error) => projection_error_category(error),
        ProjectionControllerError::NotInitialized => "projection_not_initialized",
        ProjectionControllerError::BuilderChangeRequiresOwner => "builder_change_requires_owner",
        ProjectionControllerError::ProjectionUnavailable => "projection_unavailable",
        ProjectionControllerError::RebuildNotFound => "projection_rebuild_not_found",
        ProjectionControllerError::RebuildNotEligible => "projection_rebuild_not_eligible",
        ProjectionControllerError::NothingToRebuild => "projection_nothing_to_rebuild",
        ProjectionControllerError::ProviderIdentityChanged => "provider_changed_restart_required",
    }
}

fn failure_category(category: FailureCategory) -> &'static str {
    match category {
        FailureCategory::ProviderUnavailable => "provider_unavailable",
        FailureCategory::ProviderIdentityChanged => "provider_identity_changed",
        FailureCategory::InvalidProjection => "invalid_projection",
        FailureCategory::ArtifactStoreUnavailable => "artifact_store_unavailable",
    }
}

pub(crate) fn builder_spec(identity: &EmbeddingBuilderIdentity) -> BuilderSpec {
    BuilderSpec {
        schema: BUILDER_SPEC_SCHEMA.to_string(),
        projection_kind: ProjectionKind::MemoryEmbedding,
        builder_id: BUILDER_ID.to_string(),
        builder_version: BUILDER_VERSION.to_string(),
        provider_family: identity.provider_family().as_str().to_string(),
        provider_compatibility_id: identity.provider_compatibility_id().to_string(),
        model_id: identity.model_id().to_string(),
        raw_dimension: identity.raw_dimension(),
        dimension: identity.effective_dimension(),
        input_contract: match identity.input_contract() {
            ProviderInputContract::MemoryTextUtf8V1 => EmbeddingInputContract::MemoryTextUtf8V1,
        },
        operation_contract: EmbeddingOperationContract::DocumentV1,
        normalization: match identity.normalization() {
            ProviderNormalization::ProviderNative => EmbeddingNormalization::ProviderNative,
            ProviderNormalization::L2AfterMatryoshkaTruncationV1 => {
                EmbeddingNormalization::L2AfterMatryoshkaTruncationV1
            }
        },
        transform_contract_id: identity.transform_contract_id().to_string(),
        artifact_schema: EMBEDDING_ARTIFACT_SCHEMA.to_string(),
    }
}

fn watermark(snapshot: &crate::memory::CanonicalProjectionSnapshot) -> CanonicalWatermark {
    CanonicalWatermark {
        root_hash: snapshot.root_hash.clone(),
        generation: snapshot.root.generation,
        revision_watermark: snapshot.root.revision_watermark,
        policy_watermark: snapshot.root.policy_watermark,
        relation_watermark: snapshot.root.relation_watermark,
    }
}

fn map_canonical_error(error: LedgerError) -> ProjectionControllerError {
    let category = map_canonical_error_category(&error);
    ProjectionControllerError::Canonical { category }
}

fn map_canonical_error_category(error: &LedgerError) -> &'static str {
    match error {
        LedgerError::WriterPoisoned | LedgerError::CommitIndeterminate => "canonical_restart_required",
        LedgerError::VaultLocked => "canonical_vault_locked",
        _ => "canonical_unavailable",
    }
}

#[cfg(test)]
mod tests;
