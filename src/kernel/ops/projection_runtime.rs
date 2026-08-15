//! Process-lifetime ownership and bounded execution for the memory projection.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use uuid::Uuid;

use crate::cas::{PersonalVaultStorage, ProjectionPairResetReason};
use crate::kernel::ops::model::HotSwapEmbeddingProvider;
use crate::memory::projection::{
    CanonicalWatermark, ProjectionCoreInspection, ProjectionCoreUnavailable, ProjectionCutoverReceipt,
    ProjectionDurableReceipt, ProjectionRebuildSelector, ProjectionStatusObservation,
};
use crate::memory::{
    CASCanonicalLedger, CanonicalContentHash, CanonicalProjectionSnapshot, MemoryEntry, MemoryRevisionId,
};

use super::projection_controller::{ProjectionControllerError, ProjectionCoordinator};

const RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const MAX_JOBS_PER_TURN: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectionUnavailableReason {
    ProjectionNotInitialized,
    OwnerResumeRequired,
    BuilderChangeRequired,
    ResetRequired(ProjectionPairResetReason),
    UnsupportedFormat,
    VaultLocked,
    PermissionDenied,
    StorageIo,
    ResourceExhausted,
    ResetPending,
    MaintenanceRequired,
    ManualIntervention,
    IdentityUnavailable(&'static str),
    WorkerRestartRequired,
    RuntimeShuttingDown,
    RuntimeStateUnavailable,
}

impl ProjectionUnavailableReason {
    pub(crate) const fn category(self) -> &'static str {
        match self {
            Self::ProjectionNotInitialized => "projection_not_initialized",
            Self::OwnerResumeRequired => "projection_owner_resume_required",
            Self::BuilderChangeRequired => "projection_builder_change_required",
            Self::ResetRequired(_) => "projection_reset_required",
            Self::UnsupportedFormat => "projection_unsupported_format",
            Self::VaultLocked => "projection_vault_locked",
            Self::PermissionDenied => "projection_permission_denied",
            Self::StorageIo => "projection_storage_io",
            Self::ResourceExhausted => "projection_resource_exhausted",
            Self::ResetPending => "projection_reset_pending",
            Self::MaintenanceRequired => "projection_maintenance_required",
            Self::ManualIntervention => "projection_manual_intervention_required",
            Self::IdentityUnavailable(category) => category,
            Self::WorkerRestartRequired => "projection_worker_restart_required",
            Self::RuntimeShuttingDown => "projection_runtime_shutting_down",
            Self::RuntimeStateUnavailable => "projection_runtime_state_unavailable",
        }
    }
}

pub(crate) enum ProjectionRuntimeStatus {
    Projection(ProjectionStatusObservation),
    Unavailable {
        revision_id: MemoryRevisionId,
        content_hash: CanonicalContentHash,
        reason: ProjectionUnavailableReason,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ProjectionRuntimeReceipt {
    pub(crate) selected_count: u64,
    pub(crate) manifest_generation: u64,
    pub(crate) event_watermark: u64,
    pub(crate) reconciled_source: CanonicalWatermark,
}

#[derive(Clone)]
pub(crate) struct ProjectionRuntimeReadinessSnapshot {
    pub(crate) control_plane_ready: bool,
    pub(crate) worker_ready: bool,
    pub(crate) control_plane_reason: Option<ProjectionUnavailableReason>,
    pub(crate) worker_reason: Option<ProjectionUnavailableReason>,
    pub(crate) identity: Result<String, &'static str>,
    pub(crate) shutting_down: bool,
}

impl From<ProjectionCutoverReceipt> for ProjectionRuntimeReceipt {
    fn from(receipt: ProjectionCutoverReceipt) -> Self {
        Self {
            selected_count: receipt.selected_count,
            manifest_generation: receipt.manifest_generation,
            event_watermark: receipt.event_watermark,
            reconciled_source: receipt.reconciled_source,
        }
    }
}

impl From<ProjectionDurableReceipt> for ProjectionRuntimeReceipt {
    fn from(receipt: ProjectionDurableReceipt) -> Self {
        Self {
            selected_count: receipt.selected_count,
            manifest_generation: receipt.manifest_generation,
            event_watermark: receipt.event_watermark,
            reconciled_source: receipt.reconciled_source,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ProjectionRuntimeError {
    #[error("projection operation requires all eligible revisions")]
    AllEligibleRequired,
    #[error("projection revision was not found")]
    NotFound,
    #[error("projection revision is not eligible")]
    NotEligible,
    #[error("no eligible projection revision exists")]
    NothingToRebuild,
    #[error("projection is unavailable: {0}")]
    Unavailable(&'static str),
}

enum ProjectionRuntimeState {
    Ready(Arc<ProjectionCoordinator>),
    Faulted {
        _controller: Arc<ProjectionCoordinator>,
        reason: ProjectionUnavailableReason,
    },
    RecoveredGenesis(crate::memory::projection::ProjectionRecoveredGenesis),
    Unavailable(ProjectionUnavailableReason),
}

/// The only process-lifetime owner of a projection controller.
pub(crate) struct ProjectionRuntime {
    vault: Arc<PersonalVaultStorage>,
    canonical: Arc<CASCanonicalLedger>,
    embedding: HotSwapEmbeddingProvider,
    owner_gate: Mutex<()>,
    owner_pending: AtomicBool,
    state: RwLock<ProjectionRuntimeState>,
    identity: RwLock<Result<String, &'static str>>,
    wake_sender: std::sync::mpsc::SyncSender<()>,
    wake_receiver: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    worker_started: AtomicBool,
    state_epoch: AtomicU64,
    stopping: AtomicBool,
    #[cfg(test)]
    worker_after_claim_barrier: Mutex<Option<(std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>)>>,
    #[cfg(test)]
    fail_recovered_resume_pre_pointer_once: AtomicBool,
}

pub(crate) struct ProjectionWorkerHandle {
    stop: Arc<AtomicBool>,
    wake_sender: std::sync::mpsc::SyncSender<()>,
    thread: Option<JoinHandle<()>>,
}

struct ProjectionWorkerRunning(Arc<ProjectionRuntime>);

impl Drop for ProjectionWorkerRunning {
    fn drop(&mut self) {
        self.0.worker_started.store(false, Ordering::Release);
    }
}

impl Drop for ProjectionWorkerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake_sender.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
impl ProjectionWorkerHandle {
    fn stop_token_for_test(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }
}

impl ProjectionRuntime {
    pub(crate) fn initialize(
        vault: Arc<PersonalVaultStorage>,
        canonical: Arc<CASCanonicalLedger>,
        embedding: HotSwapEmbeddingProvider,
    ) -> Arc<Self> {
        let (initial, identity) = startup_state(&vault, &canonical, &embedding);
        let (wake_sender, wake_receiver) = std::sync::mpsc::sync_channel(1);
        Arc::new(Self {
            vault,
            canonical,
            embedding,
            owner_gate: Mutex::new(()),
            owner_pending: AtomicBool::new(false),
            state: RwLock::new(initial),
            identity: RwLock::new(identity),
            wake_sender,
            wake_receiver: Mutex::new(Some(wake_receiver)),
            worker_started: AtomicBool::new(false),
            state_epoch: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            #[cfg(test)]
            worker_after_claim_barrier: Mutex::new(None),
            #[cfg(test)]
            fail_recovered_resume_pre_pointer_once: AtomicBool::new(false),
        })
    }

    pub(crate) fn worker_started(&self) -> bool {
        self.worker_started.load(Ordering::Acquire)
    }

    pub(crate) fn readiness_snapshot(&self) -> ProjectionRuntimeReadinessSnapshot {
        let state = match self.state.read() {
            Ok(state) => state,
            Err(_) => {
                return ProjectionRuntimeReadinessSnapshot {
                    control_plane_ready: false,
                    worker_ready: false,
                    control_plane_reason: Some(ProjectionUnavailableReason::RuntimeStateUnavailable),
                    worker_reason: Some(ProjectionUnavailableReason::RuntimeStateUnavailable),
                    identity: Err("projection_runtime_state_unavailable"),
                    shutting_down: self.stopping.load(Ordering::Acquire),
                };
            }
        };
        let identity = self
            .identity
            .read()
            .map(|identity| identity.clone())
            .unwrap_or(Err("projection_runtime_state_unavailable"));
        let shutting_down = self.stopping.load(Ordering::Acquire);
        match &*state {
            ProjectionRuntimeState::Ready(_) if shutting_down => ProjectionRuntimeReadinessSnapshot {
                control_plane_ready: true,
                worker_ready: false,
                control_plane_reason: None,
                worker_reason: Some(ProjectionUnavailableReason::RuntimeShuttingDown),
                identity,
                shutting_down,
            },
            ProjectionRuntimeState::Ready(_) => match &identity {
                Ok(_) if self.worker_started() => ProjectionRuntimeReadinessSnapshot {
                    control_plane_ready: true,
                    worker_ready: true,
                    control_plane_reason: None,
                    worker_reason: None,
                    identity,
                    shutting_down,
                },
                Ok(_) => ProjectionRuntimeReadinessSnapshot {
                    control_plane_ready: true,
                    worker_ready: false,
                    control_plane_reason: None,
                    worker_reason: Some(ProjectionUnavailableReason::RuntimeStateUnavailable),
                    identity,
                    shutting_down,
                },
                Err(category) => ProjectionRuntimeReadinessSnapshot {
                    control_plane_ready: true,
                    worker_ready: false,
                    control_plane_reason: None,
                    worker_reason: Some(ProjectionUnavailableReason::IdentityUnavailable(category)),
                    identity,
                    shutting_down,
                },
            },
            ProjectionRuntimeState::RecoveredGenesis(_) => ProjectionRuntimeReadinessSnapshot {
                control_plane_ready: false,
                worker_ready: false,
                control_plane_reason: Some(ProjectionUnavailableReason::OwnerResumeRequired),
                worker_reason: Some(ProjectionUnavailableReason::OwnerResumeRequired),
                identity,
                shutting_down,
            },
            ProjectionRuntimeState::Faulted { reason, .. } => ProjectionRuntimeReadinessSnapshot {
                control_plane_ready: false,
                worker_ready: false,
                control_plane_reason: Some(*reason),
                worker_reason: Some(*reason),
                identity,
                shutting_down,
            },
            ProjectionRuntimeState::Unavailable(reason) => ProjectionRuntimeReadinessSnapshot {
                control_plane_ready: false,
                worker_ready: false,
                control_plane_reason: Some(*reason),
                worker_reason: Some(*reason),
                identity,
                shutting_down,
            },
        }
    }

    pub(crate) fn status_authorized(
        &self,
        trusted_role: &str,
        revision_id: &MemoryRevisionId,
    ) -> Result<Option<ProjectionRuntimeStatus>, ProjectionRuntimeError> {
        let state = self
            .state
            .read()
            .map_err(|_| ProjectionRuntimeError::Unavailable("projection_runtime_state_unavailable"))?;
        if let ProjectionRuntimeState::Ready(controller) = &*state {
            return controller
                .status_authorized(trusted_role, revision_id)
                .map(|observation| observation.map(ProjectionRuntimeStatus::Projection))
                .map_err(map_controller_error);
        }
        let reason = match &*state {
            ProjectionRuntimeState::RecoveredGenesis(_) => ProjectionUnavailableReason::OwnerResumeRequired,
            ProjectionRuntimeState::Unavailable(reason) => *reason,
            ProjectionRuntimeState::Faulted { reason, .. } => *reason,
            ProjectionRuntimeState::Ready(_) => {
                return Err(ProjectionRuntimeError::Unavailable(
                    "projection_runtime_state_unavailable",
                ));
            }
        };
        self.canonical
            .with_authorized_current_revision(trusted_role, revision_id, |proof| {
                ProjectionRuntimeStatus::Unavailable {
                    revision_id: proof.source().revision_id.clone(),
                    content_hash: proof.source().content_hash.clone(),
                    reason,
                }
            })
            .map_err(|_| ProjectionRuntimeError::Unavailable("canonical_unavailable"))
    }

    /// Send a lossy content-free hint after the canonical commit is durable.
    pub(crate) fn notify_current(&self, entry: &MemoryEntry, request_id: Option<Uuid>) {
        if self.stopping.load(Ordering::Acquire) {
            projection_runtime_wake_unavailable(request_id, "projection_runtime_shutting_down");
            return;
        }
        let Ok(state) = self.state.read() else {
            projection_runtime_wake_unavailable(request_id, "projection_runtime_state_unavailable");
            return;
        };
        let controller = match &*state {
            ProjectionRuntimeState::Ready(controller) => controller,
            ProjectionRuntimeState::RecoveredGenesis(_) => {
                projection_runtime_wake_unavailable(request_id, "projection_owner_resume_required");
                return;
            }
            ProjectionRuntimeState::Faulted { reason, .. } | ProjectionRuntimeState::Unavailable(reason) => {
                projection_runtime_wake_unavailable(request_id, reason.category());
                return;
            }
        };
        let identity_reason = match self.identity.read().as_deref() {
            Ok(Ok(_)) => None,
            Ok(Err(reason)) => Some(*reason),
            Err(_) => Some("projection_runtime_state_unavailable"),
        };
        if let Some(reason) = identity_reason {
            projection_runtime_wake_unavailable(request_id, reason);
            return;
        }
        if !self.worker_started() {
            projection_runtime_wake_unavailable(request_id, "projection_worker_unavailable");
            return;
        }
        match controller.wake_for_current(
            entry.memory_id.clone(),
            entry.id.as_str().into(),
            entry.canonical_content_hash.clone(),
            request_id,
        ) {
            Ok(wake) => {
                if self.stopping.load(Ordering::Acquire) {
                    projection_runtime_wake_unavailable(request_id, "projection_runtime_shutting_down");
                    return;
                }
                let _ = controller.notify(wake);
                let _ = self.wake_sender.try_send(());
            }
            Err(error) => {
                projection_runtime_wake_unavailable(request_id, controller_unavailable_reason(&error).category());
            }
        }
    }

    pub(crate) fn start_worker(self: &Arc<Self>) -> Option<ProjectionWorkerHandle> {
        if self.stopping.load(Ordering::Acquire) {
            return None;
        }
        if self.worker_started.swap(true, Ordering::AcqRel) {
            return None;
        }
        let Some(wake_receiver) = self.wake_receiver.lock().ok().and_then(|mut slot| slot.take()) else {
            self.worker_started.store(false, Ordering::Release);
            return None;
        };
        let runtime = Arc::clone(self);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("plico-projection".into())
            .spawn(move || {
                let runtime = ProjectionWorkerRunning(runtime);
                while !worker_stop.load(Ordering::Acquire) {
                    match wake_receiver.recv_timeout(RECONCILE_INTERVAL) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    runtime.0.run_worker_turn(&worker_stop);
                }
            })
            .ok();
        let Some(thread) = thread else {
            self.worker_started.store(false, Ordering::Release);
            return None;
        };
        let _ = self.wake_sender.try_send(());
        Some(ProjectionWorkerHandle {
            stop,
            wake_sender: self.wake_sender.clone(),
            thread: Some(thread),
        })
    }

    pub(crate) fn owner_rebuild(
        &self,
        trusted_role: &str,
        selector: ProjectionRebuildSelector,
    ) -> Result<ProjectionRuntimeReceipt, ProjectionRuntimeError> {
        if self.stopping.load(Ordering::Acquire) {
            return Err(ProjectionRuntimeError::Unavailable("projection_runtime_shutting_down"));
        }
        if trusted_role != crate::PERSONAL_OWNER_ROLE_ID {
            return Err(ProjectionRuntimeError::Unavailable("projection_owner_required"));
        }
        let _owner_operation = self
            .owner_gate
            .lock()
            .map_err(|_| ProjectionRuntimeError::Unavailable("projection_runtime_state_unavailable"))?;
        if self.stopping.load(Ordering::Acquire) {
            return Err(ProjectionRuntimeError::Unavailable("projection_runtime_shutting_down"));
        }
        self.owner_pending.store(true, Ordering::Release);
        let _owner_pending = OwnerPending(&self.owner_pending);

        // A healthy controller already owns the namespace. Wait for the
        // current worker read-side attempt, then rebuild through that same
        // controller without probing or opening a second store.
        {
            let mut state = self
                .state
                .write()
                .map_err(|_| ProjectionRuntimeError::Unavailable("projection_runtime_state_unavailable"))?;
            if let ProjectionRuntimeState::Ready(controller) = &*state {
                self.revalidate_ready_controller(controller)?;
                let receipt = controller.owner_rebuild(selector).map_err(map_controller_error)?;
                self.state_epoch.fetch_add(1, Ordering::AcqRel);
                let _ = self.wake_sender.try_send(());
                return Ok(receipt.into());
            }
            if let ProjectionRuntimeState::Faulted { reason, .. } = &*state {
                return Err(ProjectionRuntimeError::Unavailable(reason.category()));
            }
            if matches!(selector, ProjectionRebuildSelector::CurrentRevision(_)) {
                return Err(ProjectionRuntimeError::AllEligibleRequired);
            }
            // Do not retain a lifecycle write guard across provider I/O.
            let _ = &mut state;
        }

        let provider = self.embedding.verified_document_snapshot().map_err(|error| {
            if let Ok(mut identity) = self.identity.write() {
                *identity = Err(error.category());
            }
            ProjectionRuntimeError::Unavailable(error.category())
        })?;

        // Re-enter the lifecycle only after provider identity is sealed. The
        // write guard waits for any in-flight worker attempt and covers the
        // exact re-inspection, cutover, and controller installation.
        let mut state = self
            .state
            .write()
            .map_err(|_| ProjectionRuntimeError::Unavailable("projection_runtime_state_unavailable"))?;
        if let ProjectionRuntimeState::Ready(controller) = &*state {
            self.revalidate_ready_controller(controller)?;
            let receipt = controller.owner_rebuild(selector).map_err(map_controller_error)?;
            self.state_epoch.fetch_add(1, Ordering::AcqRel);
            let _ = self.wake_sender.try_send(());
            return Ok(receipt.into());
        }
        if let ProjectionRuntimeState::Faulted { reason, .. } = &*state {
            return Err(ProjectionRuntimeError::Unavailable(reason.category()));
        }
        provider
            .revalidate()
            .map_err(|error| ProjectionRuntimeError::Unavailable(error.category()))?;
        if let Ok(mut identity) = self.identity.write() {
            *identity = Ok(provider.identity().model_id().to_string());
        }

        if matches!(&*state, ProjectionRuntimeState::RecoveredGenesis(_)) {
            let previous = std::mem::replace(
                &mut *state,
                ProjectionRuntimeState::Unavailable(ProjectionUnavailableReason::OwnerResumeRequired),
            );
            let ProjectionRuntimeState::RecoveredGenesis(recovered) = previous else {
                *state = previous;
                return Err(ProjectionRuntimeError::Unavailable(
                    "projection_runtime_state_unavailable",
                ));
            };
            return match ProjectionCoordinator::resume_recovered_for_owner(
                recovered,
                Arc::clone(&self.canonical),
                provider,
            ) {
                Ok((controller, receipt)) => {
                    *state = ProjectionRuntimeState::Ready(Arc::new(controller));
                    self.state_epoch.fetch_add(1, Ordering::AcqRel);
                    let _ = self.wake_sender.try_send(());
                    Ok(receipt.into())
                }
                Err(failure) => {
                    *state = ProjectionRuntimeState::RecoveredGenesis(failure.recovered);
                    Err(map_controller_error(failure.error))
                }
            };
        }

        let inspection = ProjectionCoordinator::inspect_verified(&self.vault, &self.canonical, &provider)
            .map_err(map_controller_error)?;
        let (controller, receipt) = match inspection {
            ProjectionCoreInspection::Exact { .. } => {
                let controller = ProjectionCoordinator::open_existing(
                    Arc::clone(&self.vault),
                    Arc::clone(&self.canonical),
                    provider,
                )
                .map_err(map_controller_error)?;
                let receipt = controller
                    .owner_rebuild(ProjectionRebuildSelector::AllEligible)
                    .map_err(map_controller_error)?;
                *state = ProjectionRuntimeState::Ready(Arc::new(controller));
                self.state_epoch.fetch_add(1, Ordering::AcqRel);
                let _ = self.wake_sender.try_send(());
                return Ok(receipt.into());
            }
            ProjectionCoreInspection::Absent => ProjectionCoordinator::bootstrap_for_owner(
                Arc::clone(&self.vault),
                Arc::clone(&self.canonical),
                provider,
            ),
            ProjectionCoreInspection::GenesisOnly => ProjectionCoordinator::resume_genesis_for_owner(
                Arc::clone(&self.vault),
                Arc::clone(&self.canonical),
                provider,
            ),
            ProjectionCoreInspection::BuilderMismatch => ProjectionCoordinator::change_builder_for_owner(
                Arc::clone(&self.vault),
                Arc::clone(&self.canonical),
                provider,
            ),
            ProjectionCoreInspection::ResetRequired(_) => {
                ProjectionCoordinator::reset_for_owner(Arc::clone(&self.vault), Arc::clone(&self.canonical), provider)
            }
            ProjectionCoreInspection::ResetPending | ProjectionCoreInspection::MaintenanceRequired => {
                let recovered = ProjectionCoordinator::recover_reset_for_owner(
                    Arc::clone(&self.vault),
                    Arc::clone(&self.canonical),
                )
                .map_err(map_controller_error)?;
                #[cfg(test)]
                if self
                    .fail_recovered_resume_pre_pointer_once
                    .swap(false, Ordering::AcqRel)
                {
                    recovered.inject_pre_pointer_failure_once();
                }
                return match ProjectionCoordinator::resume_recovered_for_owner(
                    recovered,
                    Arc::clone(&self.canonical),
                    provider,
                ) {
                    Ok((controller, receipt)) => {
                        *state = ProjectionRuntimeState::Ready(Arc::new(controller));
                        self.state_epoch.fetch_add(1, Ordering::AcqRel);
                        let _ = self.wake_sender.try_send(());
                        Ok(receipt.into())
                    }
                    Err(failure) => {
                        *state = ProjectionRuntimeState::RecoveredGenesis(failure.recovered);
                        Err(map_controller_error(failure.error))
                    }
                };
            }
            other => return Err(ProjectionRuntimeError::Unavailable(inspection_reason(other).category())),
        }
        .map_err(map_controller_error)?;
        *state = ProjectionRuntimeState::Ready(Arc::new(controller));
        self.state_epoch.fetch_add(1, Ordering::AcqRel);
        let _ = self.wake_sender.try_send(());
        Ok(receipt.into())
    }

    fn run_worker_turn(&self, stop: &AtomicBool) {
        if stop.load(Ordering::Acquire) || self.stopping.load(Ordering::Acquire) {
            return;
        }
        let mut completed = 0;
        for _ in 0..MAX_JOBS_PER_TURN {
            if stop.load(Ordering::Acquire)
                || self.stopping.load(Ordering::Acquire)
                || self.owner_pending.load(Ordering::Acquire)
            {
                break;
            }
            let turn_epoch = self.state_epoch.load(Ordering::Acquire);
            let (controller, fault) = {
                let Ok(state) = self.state.read() else {
                    return;
                };
                let ProjectionRuntimeState::Ready(controller) = &*state else {
                    return;
                };
                if !matches!(self.identity.read().as_deref(), Ok(Ok(_))) {
                    return;
                }
                let controller = Arc::clone(controller);
                let job = match controller.reconcile_and_claim_one() {
                    Ok(Some(job)) => job,
                    Ok(None) => return,
                    Err(_error) => {
                        drop(state);
                        self.publish_worker_fault(&controller, turn_epoch);
                        return;
                    }
                };
                #[cfg(test)]
                if let Some((entered, release)) = self
                    .worker_after_claim_barrier
                    .lock()
                    .ok()
                    .and_then(|mut barrier| barrier.take())
                {
                    let _ = entered.send(());
                    let _ = release.recv();
                }
                let outcome = controller.complete_one_interruptible(job, || stop.load(Ordering::Acquire));
                let mut fault = false;
                match outcome {
                    Ok(super::projection_controller::ProjectionBuildOutcome::Failed(
                        crate::memory::projection::FailureCategory::ProviderIdentityChanged,
                    )) => {
                        if let Ok(mut identity) = self.identity.write() {
                            *identity = Err("provider_changed_restart_required");
                        }
                    }
                    Ok(_) => {}
                    Err(_error) => fault = true,
                }
                (controller, fault)
            };
            if fault {
                self.publish_worker_fault(&controller, turn_epoch);
                break;
            }
            completed += 1;
        }
        if completed == MAX_JOBS_PER_TURN {
            let _ = self.wake_sender.try_send(());
        }
    }

    fn publish_worker_fault(&self, controller: &Arc<ProjectionCoordinator>, turn_epoch: u64) {
        if let Ok(mut state) = self.state.write() {
            if self.state_epoch.load(Ordering::Acquire) == turn_epoch
                && matches!(&*state, ProjectionRuntimeState::Ready(current) if Arc::ptr_eq(current, controller))
            {
                *state = ProjectionRuntimeState::Faulted {
                    _controller: Arc::clone(controller),
                    reason: ProjectionUnavailableReason::WorkerRestartRequired,
                };
            }
        }
    }

    pub(crate) fn begin_shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.wake_sender.try_send(());
    }

    pub(crate) fn finish_shutdown_barrier(&self) {
        let Ok(_owner) = self.owner_gate.lock() else {
            return;
        };
        let Ok(_lifecycle) = self.state.write() else {
            return;
        };
    }

    fn revalidate_ready_controller(&self, controller: &ProjectionCoordinator) -> Result<(), ProjectionRuntimeError> {
        if let Err(error) = controller.revalidate_provider() {
            if let Ok(mut identity) = self.identity.write() {
                *identity = Err("provider_changed_restart_required");
            }
            return Err(map_controller_error(error));
        }
        Ok(())
    }

    #[cfg(test)]
    fn inject_worker_after_claim_barrier(
        &self,
        entered: std::sync::mpsc::SyncSender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        *self.worker_after_claim_barrier.lock().unwrap() = Some((entered, release));
    }

    #[cfg(test)]
    fn inject_recovered_resume_pre_pointer_failure_once(&self) {
        self.fail_recovered_resume_pre_pointer_once
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn inject_manifest_pre_pointer_failure_once(&self) {
        if let Ok(state) = self.state.read() {
            if let ProjectionRuntimeState::Ready(controller) = &*state {
                controller.inject_manifest_pre_pointer_failure_once();
            }
        }
    }

    #[cfg(test)]
    fn inject_manifest_post_exchange_sync_failure_once(&self) {
        if let Ok(state) = self.state.read() {
            if let ProjectionRuntimeState::Ready(controller) = &*state {
                controller.inject_manifest_post_exchange_sync_failure_once();
            }
        }
    }

    #[cfg(test)]
    fn owner_pending_for_test(&self) -> bool {
        self.owner_pending.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(crate) fn vault_weak_for_test(&self) -> std::sync::Weak<PersonalVaultStorage> {
        Arc::downgrade(&self.vault)
    }
}

struct OwnerPending<'a>(&'a AtomicBool);

impl Drop for OwnerPending<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn projection_runtime_wake_unavailable(request_id: Option<Uuid>, reason: &'static str) {
    let run_id = Uuid::new_v4();
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
        result_category = "worker_unavailable",
        reason,
        run_id = %run_id,
        request_id = %request_id,
    );
}

fn startup_state(
    vault: &Arc<PersonalVaultStorage>,
    canonical: &Arc<CASCanonicalLedger>,
    embedding: &HotSwapEmbeddingProvider,
) -> (ProjectionRuntimeState, Result<String, &'static str>) {
    match embedding.verified_document_snapshot() {
        Ok(provider) => {
            let active_provider = provider.identity().model_id().to_string();
            let inspection = match ProjectionCoordinator::inspect_verified(vault, canonical, &provider) {
                Ok(inspection) => inspection,
                Err(error) => {
                    return (
                        ProjectionRuntimeState::Unavailable(controller_unavailable_reason(&error)),
                        Ok(active_provider),
                    );
                }
            };
            let state = match inspection {
                ProjectionCoreInspection::Exact { .. } => {
                    match ProjectionCoordinator::open_existing(Arc::clone(vault), Arc::clone(canonical), provider) {
                        Ok(controller) => {
                            let controller = Arc::new(controller);
                            match controller.reconcile_startup() {
                                Ok(()) => ProjectionRuntimeState::Ready(controller),
                                Err(_error) => ProjectionRuntimeState::Faulted {
                                    _controller: controller,
                                    reason: ProjectionUnavailableReason::WorkerRestartRequired,
                                },
                            }
                        }
                        Err(error) => ProjectionRuntimeState::Unavailable(controller_unavailable_reason(&error)),
                    }
                }
                ProjectionCoreInspection::Absent
                    if vault.created_this_open() && canonical_is_exact_genesis(canonical) =>
                {
                    ProjectionCoordinator::bootstrap_for_owner(Arc::clone(vault), Arc::clone(canonical), provider)
                        .map(|(controller, _)| ProjectionRuntimeState::Ready(Arc::new(controller)))
                        .unwrap_or_else(|error| {
                            ProjectionRuntimeState::Unavailable(controller_unavailable_reason(&error))
                        })
                }
                other => ProjectionRuntimeState::Unavailable(inspection_reason(other)),
            };
            (state, Ok(active_provider))
        }
        Err(identity_error) => {
            let category = identity_error.category();
            let state = match ProjectionCoordinator::inspect_identity_unavailable(vault, canonical, identity_error) {
                Ok(inspection) => {
                    let reason = match inspection.inspection {
                        ProjectionCoreInspection::Existing { .. } | ProjectionCoreInspection::Exact { .. } => {
                            ProjectionUnavailableReason::IdentityUnavailable(inspection.identity_category)
                        }
                        other => inspection_reason(other),
                    };
                    ProjectionRuntimeState::Unavailable(reason)
                }
                Err(error) => ProjectionRuntimeState::Unavailable(controller_unavailable_reason(&error)),
            };
            (state, Err(category))
        }
    }
}

fn canonical_is_exact_genesis(canonical: &CASCanonicalLedger) -> bool {
    canonical
        .projection_snapshot()
        .is_ok_and(|snapshot| exact_genesis_snapshot(&snapshot))
}

fn exact_genesis_snapshot(snapshot: &CanonicalProjectionSnapshot) -> bool {
    snapshot.root_hash == snapshot.genesis_root_hash
        && snapshot.root_chain.len() == 1
        && snapshot.root.generation == 0
        && snapshot.root.previous_root_hash.is_none()
        && snapshot.root.revision_head.is_none()
        && snapshot.root.revision_watermark == 0
        && snapshot.root.policy_head.is_none()
        && snapshot.root.policy_watermark == 0
        && snapshot.root.relation_head.is_none()
        && snapshot.root.relation_watermark == 0
        && snapshot.root.migration_manifest_hash.is_none()
        && snapshot.revisions.is_empty()
}

fn inspection_reason(inspection: ProjectionCoreInspection) -> ProjectionUnavailableReason {
    match inspection {
        ProjectionCoreInspection::Absent => ProjectionUnavailableReason::ProjectionNotInitialized,
        ProjectionCoreInspection::GenesisOnly => ProjectionUnavailableReason::OwnerResumeRequired,
        ProjectionCoreInspection::Existing { .. } => ProjectionUnavailableReason::BuilderChangeRequired,
        ProjectionCoreInspection::Exact { .. } => ProjectionUnavailableReason::RuntimeStateUnavailable,
        ProjectionCoreInspection::BuilderMismatch => ProjectionUnavailableReason::BuilderChangeRequired,
        ProjectionCoreInspection::ResetRequired(reason) => ProjectionUnavailableReason::ResetRequired(reason),
        ProjectionCoreInspection::UnsupportedFormat => ProjectionUnavailableReason::UnsupportedFormat,
        ProjectionCoreInspection::Unavailable(category) => match category {
            ProjectionCoreUnavailable::VaultLocked => ProjectionUnavailableReason::VaultLocked,
            ProjectionCoreUnavailable::PermissionDenied => ProjectionUnavailableReason::PermissionDenied,
            ProjectionCoreUnavailable::StorageIo => ProjectionUnavailableReason::StorageIo,
            ProjectionCoreUnavailable::ResourceExhausted => ProjectionUnavailableReason::ResourceExhausted,
        },
        ProjectionCoreInspection::ResetPending => ProjectionUnavailableReason::ResetPending,
        ProjectionCoreInspection::MaintenanceRequired => ProjectionUnavailableReason::MaintenanceRequired,
        ProjectionCoreInspection::ManualIntervention => ProjectionUnavailableReason::ManualIntervention,
    }
}

fn controller_unavailable_reason(error: &ProjectionControllerError) -> ProjectionUnavailableReason {
    match error {
        ProjectionControllerError::Canonical {
            category: "canonical_vault_locked",
        } => ProjectionUnavailableReason::VaultLocked,
        ProjectionControllerError::Canonical { .. } => ProjectionUnavailableReason::StorageIo,
        ProjectionControllerError::NotInitialized => ProjectionUnavailableReason::ProjectionNotInitialized,
        ProjectionControllerError::BuilderChangeRequiresOwner => ProjectionUnavailableReason::BuilderChangeRequired,
        ProjectionControllerError::ProviderIdentityChanged => {
            ProjectionUnavailableReason::IdentityUnavailable("provider_changed_restart_required")
        }
        ProjectionControllerError::Projection(error) => projection_unavailable_reason(error),
        _ => ProjectionUnavailableReason::StorageIo,
    }
}

fn projection_unavailable_reason(error: &crate::memory::projection::ProjectionError) -> ProjectionUnavailableReason {
    match error {
        crate::memory::projection::ProjectionError::UnsupportedFormat { .. } => {
            ProjectionUnavailableReason::UnsupportedFormat
        }
        crate::memory::projection::ProjectionError::ManualInterventionRequired => {
            ProjectionUnavailableReason::ManualIntervention
        }
        crate::memory::projection::ProjectionError::ProjectionMaintenanceRequired
        | crate::memory::projection::ProjectionError::ArtifactMaintenanceRequired => {
            ProjectionUnavailableReason::MaintenanceRequired
        }
        crate::memory::projection::ProjectionError::ResetPending => ProjectionUnavailableReason::ResetPending,
        crate::memory::projection::ProjectionError::Io(error)
            if error.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            ProjectionUnavailableReason::PermissionDenied
        }
        _ => ProjectionUnavailableReason::StorageIo,
    }
}

fn map_controller_error(error: ProjectionControllerError) -> ProjectionRuntimeError {
    match error {
        ProjectionControllerError::RebuildNotFound => ProjectionRuntimeError::NotFound,
        ProjectionControllerError::RebuildNotEligible => ProjectionRuntimeError::NotEligible,
        ProjectionControllerError::NothingToRebuild => ProjectionRuntimeError::NothingToRebuild,
        ProjectionControllerError::NotInitialized | ProjectionControllerError::BuilderChangeRequiresOwner => {
            ProjectionRuntimeError::AllEligibleRequired
        }
        ProjectionControllerError::Canonical { category } => ProjectionRuntimeError::Unavailable(category),
        ProjectionControllerError::ProjectionUnavailable => {
            ProjectionRuntimeError::Unavailable("projection_unavailable")
        }
        ProjectionControllerError::Projection(error) => {
            ProjectionRuntimeError::Unavailable(projection_unavailable_reason(&error).category())
        }
        ProjectionControllerError::ProviderIdentityChanged => {
            ProjectionRuntimeError::Unavailable("provider_changed_restart_required")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc;

    use crate::fs::{EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider};
    use crate::memory::{CanonicalLedger, CanonicalRevision, ExpectedHead, MemoryTier};

    use super::*;

    struct DeterministicProvider {
        completed: Option<mpsc::SyncSender<()>>,
        entered: Option<mpsc::SyncSender<()>>,
        release: Option<Mutex<mpsc::Receiver<()>>>,
    }

    struct DriftingProvider {
        drifted: Arc<AtomicBool>,
    }

    struct NamedProvider(&'static str);

    struct IdentityUnavailableProvider {
        calls: Arc<AtomicUsize>,
    }

    struct FirstBlockingOrderedProvider {
        calls: Arc<AtomicUsize>,
        first_entered: mpsc::SyncSender<()>,
        first_release: Mutex<mpsc::Receiver<()>>,
        order: Arc<AtomicUsize>,
        second_order: mpsc::SyncSender<usize>,
    }

    impl DeterministicProvider {
        fn immediate(completed: Option<mpsc::SyncSender<()>>) -> Self {
            Self {
                completed,
                entered: None,
                release: None,
            }
        }
    }

    impl EmbeddingProvider for DeterministicProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            if let Some(entered) = &self.entered {
                entered.send(()).unwrap();
            }
            if let Some(release) = &self.release {
                release.lock().unwrap().recv().unwrap();
            }
            if let Some(completed) = &self.completed {
                completed.send(()).unwrap();
            }
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "runtime-test",
                2,
                "runtime-test-v1",
            ))
        }

        fn model_name(&self) -> String {
            "runtime-test".into()
        }
    }

    impl EmbeddingProvider for DriftingProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            Ok(texts.iter().map(|_| EmbedResult::new(vec![1.0, 0.0], 1)).collect())
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "runtime-drift-test",
                2,
                if self.drifted.load(Ordering::Acquire) {
                    "runtime-drift-v2"
                } else {
                    "runtime-drift-v1"
                },
            ))
        }

        fn model_name(&self) -> String {
            "runtime-drift-test".into()
        }
    }

    impl EmbeddingProvider for NamedProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            Ok(texts.iter().map(|_| EmbedResult::new(vec![1.0, 0.0], 1)).collect())
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(self.0, 2, self.0))
        }

        fn model_name(&self) -> String {
            self.0.into()
        }
    }

    impl EmbeddingProvider for IdentityUnavailableProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(texts.iter().map(|_| EmbedResult::new(vec![1.0, 0.0], 1)).collect())
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Err(EmbeddingIdentityError::ProviderProbeFailed)
        }

        fn model_name(&self) -> String {
            "identity-unavailable".into()
        }
    }

    impl EmbeddingProvider for FirstBlockingOrderedProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            let call = self.calls.fetch_add(1, Ordering::AcqRel);
            if call == 0 {
                self.first_entered.send(()).unwrap();
                self.first_release.lock().unwrap().recv().unwrap();
            } else if call == 1 {
                let order = self.order.fetch_add(1, Ordering::AcqRel) + 1;
                self.second_order.send(order).unwrap();
            }
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "owner-pending",
                2,
                "owner-pending-v1",
            ))
        }

        fn model_name(&self) -> String {
            "owner-pending".into()
        }
    }

    fn open_runtime(
        vault_path: &std::path::Path,
        provider: Arc<dyn EmbeddingProvider>,
    ) -> (
        Arc<PersonalVaultStorage>,
        Arc<CASCanonicalLedger>,
        Arc<ProjectionRuntime>,
    ) {
        let vault = Arc::new(PersonalVaultStorage::open(vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        let runtime = ProjectionRuntime::initialize(
            Arc::clone(&vault),
            Arc::clone(&canonical),
            HotSwapEmbeddingProvider::new(provider),
        );
        (vault, canonical, runtime)
    }

    fn commit_root(canonical: &CASCanonicalLedger, content: &str) -> MemoryEntry {
        let entry = MemoryEntry {
            tier: MemoryTier::Working,
            ..MemoryEntry::ephemeral(crate::PERSONAL_OWNER_ROLE_ID, content)
        };
        canonical
            .commit_expected(
                crate::PERSONAL_OWNER_ROLE_ID,
                MemoryTier::Working,
                ExpectedHead::Absent,
                CanonicalRevision::from_entry(&entry).unwrap(),
            )
            .unwrap();
        canonical
            .projection_snapshot()
            .unwrap()
            .revisions
            .last()
            .unwrap()
            .clone()
            .into_runtime_entry(crate::PERSONAL_OWNER_ROLE_ID)
    }

    fn wait_for_stop(token: &AtomicBool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !token.load(Ordering::Acquire) {
            assert!(
                std::time::Instant::now() < deadline,
                "worker stop signal was not published"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn startup_requires_fresh_evidence_for_absent_projection() {
        let parent = tempfile::tempdir().unwrap();
        let fresh_path = parent.path().join("fresh");
        let (vault, canonical, runtime) = open_runtime(&fresh_path, Arc::new(DeterministicProvider::immediate(None)));
        assert!(runtime.readiness_snapshot().control_plane_ready);
        drop(runtime);
        drop(canonical);
        drop(vault);

        let (vault, canonical, reopened) = open_runtime(&fresh_path, Arc::new(DeterministicProvider::immediate(None)));
        assert!(reopened.readiness_snapshot().control_plane_ready);
        drop(reopened);
        drop(canonical);
        drop(vault);

        let existing_path = parent.path().join("existing-empty");
        std::fs::create_dir(&existing_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&existing_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (_, _, existing) = open_runtime(&existing_path, Arc::new(DeterministicProvider::immediate(None)));
        assert!(matches!(
            existing.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::ProjectionNotInitialized)
        ));
    }

    #[test]
    fn startup_genesis_mismatch_and_identity_unavailable_are_zero_write() {
        let parent = tempfile::tempdir().unwrap();

        let genesis_path = parent.path().join("genesis-only");
        let genesis_vault = Arc::new(PersonalVaultStorage::open(&genesis_path, None).unwrap());
        let genesis_canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&genesis_vault)).unwrap());
        genesis_canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                crate::memory::projection::ProjectionCoordinatorCore::bootstrap_genesis_only_for_test(
                    Arc::clone(&genesis_vault),
                    &proof,
                )
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let genesis_before = genesis_vault.projection_tree_fingerprint_for_test().unwrap();
        drop(genesis_canonical);
        drop(genesis_vault);
        let (genesis_vault, genesis_canonical, genesis_runtime) =
            open_runtime(&genesis_path, Arc::new(NamedProvider("genesis-provider")));
        assert!(matches!(
            genesis_runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::OwnerResumeRequired)
        ));
        assert_eq!(
            genesis_vault.projection_tree_fingerprint_for_test().unwrap(),
            genesis_before
        );
        drop(genesis_runtime);
        drop(genesis_canonical);
        drop(genesis_vault);

        let mismatch_path = parent.path().join("builder-mismatch");
        let (mismatch_vault, mismatch_canonical, mismatch_runtime) =
            open_runtime(&mismatch_path, Arc::new(NamedProvider("builder-v1")));
        assert!(mismatch_runtime.readiness_snapshot().control_plane_ready);
        drop(mismatch_runtime);
        drop(mismatch_canonical);
        drop(mismatch_vault);
        let mismatch_vault = Arc::new(PersonalVaultStorage::open(&mismatch_path, None).unwrap());
        let mismatch_canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&mismatch_vault)).unwrap());
        let mismatch_before = mismatch_vault.projection_tree_fingerprint_for_test().unwrap();
        let mismatch_runtime = ProjectionRuntime::initialize(
            Arc::clone(&mismatch_vault),
            Arc::clone(&mismatch_canonical),
            HotSwapEmbeddingProvider::new(Arc::new(NamedProvider("builder-v2"))),
        );
        assert!(matches!(
            mismatch_runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::BuilderChangeRequired)
        ));
        assert_eq!(
            mismatch_vault.projection_tree_fingerprint_for_test().unwrap(),
            mismatch_before
        );
        drop(mismatch_runtime);
        drop(mismatch_canonical);
        drop(mismatch_vault);

        let unavailable_path = parent.path().join("identity-unavailable");
        std::fs::create_dir(&unavailable_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&unavailable_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let (unavailable_vault, _canonical, unavailable_runtime) = open_runtime(
            &unavailable_path,
            Arc::new(IdentityUnavailableProvider {
                calls: Arc::clone(&calls),
            }),
        );
        let unavailable_before = unavailable_vault.projection_tree_fingerprint_for_test().unwrap();
        let worker = unavailable_runtime.start_worker().unwrap();
        assert!(matches!(
            unavailable_runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::ProjectionNotInitialized)
        ));
        assert_eq!(calls.load(Ordering::Acquire), 0);
        assert_eq!(
            unavailable_vault.projection_tree_fingerprint_for_test().unwrap(),
            unavailable_before
        );
        drop(worker);
    }

    #[test]
    fn healthy_owner_current_and_all_eligible_reuse_the_claimed_controller() {
        let parent = tempfile::tempdir().unwrap();
        let (_, canonical, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(DeterministicProvider::immediate(None)),
        );
        let entry = commit_root(&canonical, "owner rebuild source");
        let all = runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .unwrap();
        assert_eq!(all.selected_count, 1);
        let current = runtime
            .owner_rebuild(
                crate::PERSONAL_OWNER_ROLE_ID,
                ProjectionRebuildSelector::CurrentRevision(entry.id.as_str().into()),
            )
            .unwrap();
        assert_eq!(current.selected_count, 1);
        assert!(runtime.readiness_snapshot().control_plane_ready);
    }

    #[test]
    fn idle_worker_activates_after_owner_bootstrap_without_restart() {
        let parent = tempfile::tempdir().unwrap();
        let vault_path = parent.path().join("existing-empty");
        std::fs::create_dir(&vault_path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&vault_path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (completed_tx, completed_rx) = mpsc::sync_channel(1);
        let (vault, canonical, runtime) = open_runtime(
            &vault_path,
            Arc::new(DeterministicProvider::immediate(Some(completed_tx))),
        );
        assert!(matches!(
            runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::ProjectionNotInitialized)
        ));
        let before = vault.projection_tree_fingerprint_for_test().unwrap();
        let worker = runtime
            .start_worker()
            .expect("unavailable runtime still owns one idle worker");
        assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), before);

        let entry = commit_root(&canonical, "late owner activation");
        let receipt = runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .unwrap();
        assert_eq!(receipt.selected_count, 1);
        completed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let status = runtime
                .status_authorized(crate::PERSONAL_OWNER_ROLE_ID, &entry.id.as_str().into())
                .unwrap();
            if matches!(
                status,
                Some(ProjectionRuntimeStatus::Projection(
                    ProjectionStatusObservation::Observed {
                        state: crate::memory::projection::ProjectionStatusState::Ready,
                        ..
                    }
                ))
            ) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owner wake did not publish the queued revision"
            );
            std::thread::yield_now();
        }
        drop(worker);

        let readiness = runtime.readiness_snapshot();
        assert!(readiness.control_plane_ready);
        assert!(!readiness.worker_ready, "joined worker must not be reported ready");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recovered_genesis_resume_failure_retains_capability_for_same_process_retry() {
        let parent = tempfile::tempdir().unwrap();
        let vault_path = parent.path().join("vault");
        let provider_identity = NamedProvider("recovered-provider").builder_identity().unwrap();
        let builder = super::super::projection_controller::builder_spec(&provider_identity);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        commit_root(&canonical, "reset recovery source");
        let (core, _) = canonical
            .with_owner_projection_maintenance(crate::PERSONAL_OWNER_ROLE_ID, |proof| {
                crate::memory::projection::ProjectionCoordinatorCore::bootstrap_authorized(
                    Arc::clone(&vault),
                    builder,
                    ProjectionRebuildSelector::AllEligible,
                    &proof,
                )
            })
            .unwrap()
            .unwrap()
            .unwrap();
        let current_view_hash = core
            .root_chain_for_test()
            .unwrap()
            .last()
            .unwrap()
            .current_view_hash
            .clone();
        drop(core);
        drop(canonical);
        drop(vault);

        let vault = Arc::new(PersonalVaultStorage::open(&vault_path, None).unwrap());
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).unwrap());
        vault
            .remove_projection_manifest_object_for_test(&current_view_hash)
            .unwrap();
        let runtime = ProjectionRuntime::initialize(
            Arc::clone(&vault),
            Arc::clone(&canonical),
            HotSwapEmbeddingProvider::new(Arc::new(NamedProvider("recovered-provider"))),
        );
        vault.inject_projection_reset_after_marker_exchange_once();
        assert!(runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .is_err());
        drop(runtime);
        drop(canonical);
        drop(vault);

        let (vault, _canonical, runtime) = open_runtime(&vault_path, Arc::new(NamedProvider("recovered-provider")));
        assert!(matches!(
            runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::MaintenanceRequired | ProjectionUnavailableReason::ResetPending)
        ));
        runtime.inject_recovered_resume_pre_pointer_failure_once();
        assert!(runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .is_err());
        assert!(matches!(
            runtime.readiness_snapshot().control_plane_reason,
            Some(ProjectionUnavailableReason::OwnerResumeRequired)
        ));
        let receipt = runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .unwrap();
        assert_eq!(receipt.selected_count, 1);
        assert!(runtime.readiness_snapshot().control_plane_ready);
        assert_eq!(vault.projection_quarantine_count_for_test().unwrap(), 0);
    }

    #[test]
    fn bounded_worker_self_wakes_after_sixteen_jobs_and_shutdown_releases_vault() {
        let parent = tempfile::tempdir().unwrap();
        let vault_path = parent.path().join("vault");
        let (completed_tx, completed_rx) = mpsc::sync_channel(32);
        let (vault, canonical, runtime) = open_runtime(
            &vault_path,
            Arc::new(DeterministicProvider::immediate(Some(completed_tx))),
        );
        for index in 0..17 {
            let entry = commit_root(&canonical, &format!("bounded worker {index}"));
            runtime.notify_current(&entry, None);
        }
        let worker = runtime.start_worker().unwrap();
        assert!(runtime.start_worker().is_none());
        for _ in 0..17 {
            completed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        drop(worker);
        assert!(!runtime.worker_started());
        drop(runtime);
        drop(canonical);
        drop(vault);
        let reopened = PersonalVaultStorage::open(&vault_path, None).unwrap();
        assert!(!reopened.created_this_open());
    }

    #[test]
    fn owner_pending_yields_between_jobs_before_second_provider_call() {
        let parent = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(AtomicUsize::new(0));
        let (first_entered_tx, first_entered_rx) = mpsc::sync_channel(1);
        let (first_release_tx, first_release_rx) = mpsc::sync_channel(1);
        let (second_order_tx, second_order_rx) = mpsc::sync_channel(1);
        let (_, canonical, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(FirstBlockingOrderedProvider {
                calls: Arc::clone(&calls),
                first_entered: first_entered_tx,
                first_release: Mutex::new(first_release_rx),
                order: Arc::clone(&order),
                second_order: second_order_tx,
            }),
        );
        commit_root(&canonical, "owner pending first");
        commit_root(&canonical, "owner pending second");
        runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .unwrap();
        let worker = runtime.start_worker().unwrap();
        first_entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        let (owner_order_tx, owner_order_rx) = mpsc::sync_channel(1);
        let owner_runtime = Arc::clone(&runtime);
        let owner_sequence = Arc::clone(&order);
        let owner = std::thread::spawn(move || {
            owner_runtime
                .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
                .unwrap();
            let completed_order = owner_sequence.fetch_add(1, Ordering::AcqRel) + 1;
            owner_order_tx.send(completed_order).unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !runtime.owner_pending_for_test() {
            assert!(
                std::time::Instant::now() < deadline,
                "owner did not publish pending state"
            );
            std::thread::yield_now();
        }
        first_release_tx.send(()).unwrap();
        let owner_completed = owner_order_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second_started = second_order_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(owner_completed < second_started);
        owner.join().unwrap();
        drop(worker);
        assert_eq!(calls.load(Ordering::Acquire), 2);
    }

    #[test]
    fn shutdown_during_provider_call_never_publishes_ready_or_failed() {
        let parent = tempfile::tempdir().unwrap();
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        let (_, canonical, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(DeterministicProvider {
                completed: None,
                entered: Some(entered_tx),
                release: Some(Mutex::new(release_rx)),
            }),
        );
        let entry = commit_root(&canonical, "shutdown barrier");
        runtime.notify_current(&entry, None);
        let worker = runtime.start_worker().unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.begin_shutdown();
        let stop = worker.stop_token_for_test();
        let shutdown = std::thread::spawn(move || drop(worker));
        wait_for_stop(&stop);
        release_tx.send(()).unwrap();
        shutdown.join().unwrap();
        runtime.finish_shutdown_barrier();
        let Some(ProjectionRuntimeStatus::Projection(ProjectionStatusObservation::Observed { state, .. })) = runtime
            .status_authorized(crate::PERSONAL_OWNER_ROLE_ID, &entry.id.as_str().into())
            .unwrap()
        else {
            panic!("projection status must remain observable after shutdown")
        };
        assert!(matches!(
            state,
            crate::memory::projection::ProjectionStatusState::Building
        ));
        let projection_before = runtime.vault.projection_tree_fingerprint_for_test().unwrap();
        for selector in [
            ProjectionRebuildSelector::AllEligible,
            ProjectionRebuildSelector::CurrentRevision(entry.id.as_str().into()),
        ] {
            assert!(matches!(
                runtime.owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, selector),
                Err(ProjectionRuntimeError::Unavailable("projection_runtime_shutting_down"))
            ));
        }
        assert_eq!(
            runtime.vault.projection_tree_fingerprint_for_test().unwrap(),
            projection_before
        );
        let after_shutdown = commit_root(&canonical, "canonical remains writable during projection shutdown");
        runtime.notify_current(&after_shutdown, Some(Uuid::new_v4()));
        runtime.run_worker_turn(&AtomicBool::new(false));
        assert_eq!(
            runtime.vault.projection_tree_fingerprint_for_test().unwrap(),
            projection_before
        );
    }

    #[test]
    fn ready_owner_detects_identity_drift_without_projection_write() {
        let parent = tempfile::tempdir().unwrap();
        let drifted = Arc::new(AtomicBool::new(false));
        let (vault, _, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(DriftingProvider {
                drifted: Arc::clone(&drifted),
            }),
        );
        let before = vault.projection_tree_fingerprint_for_test().unwrap();
        drifted.store(true, Ordering::Release);
        assert!(matches!(
            runtime.owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible),
            Err(ProjectionRuntimeError::Unavailable("provider_changed_restart_required"))
        ));
        let readiness = runtime.readiness_snapshot();
        assert_eq!(readiness.identity, Err("provider_changed_restart_required"));
        assert!(readiness.control_plane_ready);
        assert!(!readiness.worker_ready);
        assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), before);
    }

    #[test]
    fn provider_drift_stops_later_jobs_across_wake_and_turn() {
        let parent = tempfile::tempdir().unwrap();
        let drifted = Arc::new(AtomicBool::new(false));
        let (vault, canonical, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(DriftingProvider {
                drifted: Arc::clone(&drifted),
            }),
        );
        let first = commit_root(&canonical, "drift first");
        let second = commit_root(&canonical, "drift second");
        runtime
            .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
            .unwrap();
        drifted.store(true, Ordering::Release);
        runtime.run_worker_turn(&AtomicBool::new(false));

        let first_status = runtime
            .status_authorized(crate::PERSONAL_OWNER_ROLE_ID, &first.id.as_str().into())
            .unwrap();
        let second_status = runtime
            .status_authorized(crate::PERSONAL_OWNER_ROLE_ID, &second.id.as_str().into())
            .unwrap();
        let failed = |status: &Option<ProjectionRuntimeStatus>| {
            matches!(
                status,
                Some(ProjectionRuntimeStatus::Projection(
                    ProjectionStatusObservation::Observed {
                        state: crate::memory::projection::ProjectionStatusState::Failed {
                            failure_category: crate::memory::projection::FailureCategory::ProviderIdentityChanged,
                            ..
                        },
                        ..
                    }
                ))
            )
        };
        let queued = |status: &Option<ProjectionRuntimeStatus>| {
            matches!(
                status,
                Some(ProjectionRuntimeStatus::Projection(
                    ProjectionStatusObservation::Observed {
                        state: crate::memory::projection::ProjectionStatusState::Queued { .. },
                        ..
                    }
                ))
            )
        };
        assert!((failed(&first_status) && queued(&second_status)) || (queued(&first_status) && failed(&second_status)));
        let queued_entry = if queued(&first_status) { &first } else { &second };
        let after_drift = vault.projection_tree_fingerprint_for_test().unwrap();
        runtime.notify_current(queued_entry, Some(Uuid::new_v4()));
        runtime.run_worker_turn(&AtomicBool::new(false));
        assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), after_drift);
    }

    #[test]
    fn worker_manifest_failures_retain_controller_and_require_restart_for_both_selectors() {
        #[derive(Clone, Copy)]
        enum Cutpoint {
            Reconcile,
            Claim,
            Complete,
        }

        for cutpoint in [Cutpoint::Reconcile, Cutpoint::Claim, Cutpoint::Complete] {
            let parent = tempfile::tempdir().unwrap();
            let vault_path = parent.path().join("vault");
            let (vault, canonical, runtime) =
                open_runtime(&vault_path, Arc::new(DeterministicProvider::immediate(None)));
            let entry = commit_root(&canonical, "faulted worker");
            if !matches!(cutpoint, Cutpoint::Reconcile) {
                runtime
                    .owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, ProjectionRebuildSelector::AllEligible)
                    .unwrap();
            }

            match cutpoint {
                Cutpoint::Reconcile | Cutpoint::Claim => {
                    runtime.inject_manifest_pre_pointer_failure_once();
                    runtime.run_worker_turn(&AtomicBool::new(false));
                }
                Cutpoint::Complete => {
                    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
                    let (release_tx, release_rx) = mpsc::sync_channel(1);
                    runtime.inject_worker_after_claim_barrier(entered_tx, release_rx);
                    let running = Arc::clone(&runtime);
                    let turn = std::thread::spawn(move || running.run_worker_turn(&AtomicBool::new(false)));
                    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    runtime.inject_manifest_post_exchange_sync_failure_once();
                    release_tx.send(()).unwrap();
                    turn.join().unwrap();
                }
            }

            assert!(matches!(
                runtime.readiness_snapshot().control_plane_reason,
                Some(ProjectionUnavailableReason::WorkerRestartRequired)
            ));
            let after_fault = vault.projection_tree_fingerprint_for_test().unwrap();
            for selector in [
                ProjectionRebuildSelector::AllEligible,
                ProjectionRebuildSelector::CurrentRevision(entry.id.as_str().into()),
            ] {
                assert!(matches!(
                    runtime.owner_rebuild(crate::PERSONAL_OWNER_ROLE_ID, selector),
                    Err(ProjectionRuntimeError::Unavailable(
                        "projection_worker_restart_required"
                    ))
                ));
            }
            assert_eq!(vault.projection_tree_fingerprint_for_test().unwrap(), after_fault);
            if matches!(cutpoint, Cutpoint::Complete) {
                drop(runtime);
                drop(canonical);
                drop(vault);
                let (_vault, _canonical, restarted) =
                    open_runtime(&vault_path, Arc::new(DeterministicProvider::immediate(None)));
                assert!(restarted.readiness_snapshot().control_plane_ready);
                assert!(matches!(
                    restarted.status_authorized(crate::PERSONAL_OWNER_ROLE_ID, &entry.id.as_str().into()),
                    Ok(Some(ProjectionRuntimeStatus::Projection(
                        ProjectionStatusObservation::Observed {
                            state: crate::memory::projection::ProjectionStatusState::Ready,
                            ..
                        }
                    )))
                ));
            }
        }
    }

    #[test]
    fn shutdown_after_claim_never_starts_provider_call() {
        let parent = tempfile::tempdir().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        struct CountingProvider(Arc<AtomicUsize>);
        impl EmbeddingProvider for CountingProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                self.0.fetch_add(1, Ordering::AcqRel);
                Ok(EmbedResult::new(vec![1.0, 0.0], 1))
            }
            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                texts.iter().map(|text| self.embed(text)).collect()
            }
            fn dimension(&self) -> usize {
                2
            }
            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                Ok(EmbeddingBuilderIdentity::test_deterministic(
                    "runtime-count",
                    2,
                    "runtime-count-v1",
                ))
            }
            fn model_name(&self) -> String {
                "runtime-count".into()
            }
        }
        let (_, canonical, runtime) = open_runtime(
            &parent.path().join("vault"),
            Arc::new(CountingProvider(Arc::clone(&calls))),
        );
        let entry = commit_root(&canonical, "pre-provider shutdown");
        runtime.notify_current(&entry, None);
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let (release_tx, release_rx) = mpsc::sync_channel(1);
        runtime.inject_worker_after_claim_barrier(entered_tx, release_rx);
        let worker = runtime.start_worker().unwrap();
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        runtime.begin_shutdown();
        let stop = worker.stop_token_for_test();
        let shutdown = std::thread::spawn(move || drop(worker));
        wait_for_stop(&stop);
        release_tx.send(()).unwrap();
        shutdown.join().unwrap();
        runtime.finish_shutdown_barrier();
        assert_eq!(calls.load(Ordering::Acquire), 0);
    }
}
