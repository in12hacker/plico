//! Durable transaction substrate for the execution-observation fixture
//! ledger (ADR-0008, milestone v53 WP2).
//!
//! Crate-private and sealed: bounded loader, dual-slot classifier, and a
//! structural publisher over the architecture-frozen CAS capability. No
//! facade, no attempt lookup, no replay, no production wiring (WP3+).
//!
//! Debug narration is leveled (`store_error!`/`store_warn!`/`store_info!`
//! /`store_debug!`) and compiles to nothing in release builds — the whole
//! statement expands to an empty block when neither `test` nor
//! `debug_assertions` holds, so no formatting code is linked and nothing but
//! stable categories/phases/counts is ever printed.

mod loader;
mod publisher;
mod slots;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::ledger_store::{LedgerStorageOpenError, PersonalVaultStorage};

use super::error::ObservationStoreError;
use super::model::{
    FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1, StoredStartedEventV1, StoredTerminalEventV1,
};
use super::validation::CANONICAL_REQUEST_MAX_BYTES;

/// Envelope headroom over the largest canonical request (ADR-0008 §4).
pub(super) const STORED_EVENT_MAX_BYTES: usize = CANONICAL_REQUEST_MAX_BYTES + 4_096;

/// The typed event a structural commit appends (ADR-0008 §6).
pub(super) enum FixtureStoredEventV1 {
    Started(StoredStartedEventV1),
    Terminal(StoredTerminalEventV1),
}

/// The full immutable object set of one commit; the publisher recomputes
/// every binding and never trusts caller-supplied digests.
pub(super) struct FixtureStructuralCommitV1 {
    pub(super) event: FixtureStoredEventV1,
    pub(super) segment: FixtureEventSegmentV1,
    pub(super) current_view: FixtureCurrentViewV1,
    pub(super) root: FixtureLedgerRootV1,
}

/// Verified identity of the active root after open or a successful commit.
#[derive(Clone)]
pub(super) struct FixtureStructuralStateV1 {
    pub(super) root_sha256: String,
    pub(super) generation: u64,
    pub(super) event_watermark: u64,
}

/// Sealed durable store handle. A single instance owns the CAS capability
/// for this vault lifecycle; `CommitIndeterminate` poisons the handle.
pub(super) struct FixtureObservationStoreV1 {
    storage: ExecutionObservationFixtureStorage,
    state: Mutex<Option<FixtureStructuralStateV1>>,
    poisoned: AtomicBool,
}

impl FixtureObservationStoreV1 {
    /// Opens (and if fresh, deterministically creates) the fixture ledger.
    /// The vault handle is consumed exactly once by the sealed CAS opener.
    pub(super) fn open_fixture(vault: Arc<PersonalVaultStorage>) -> Result<Self, ObservationStoreError> {
        let storage = ExecutionObservationFixtureStorage::open(vault).map_err(|error| match error {
            LedgerStorageOpenError::NamespaceAlreadyClaimed => ObservationStoreError::NamespaceAlreadyClaimed,
            other => {
                let _ = &other;
                store_warn!("phase=open category=storage_unavailable kind={}", kind_of(&other));
                ObservationStoreError::StorageUnavailable
            }
        })?;
        let state = slots::startup(&storage)?;
        store_info!("phase=open category=ok generation={}", state.generation);
        Ok(Self {
            storage,
            state: Mutex::new(Some(state)),
            poisoned: AtomicBool::new(false),
        })
    }

    /// Latest verified active-root identity. Poisoned handles answer `Poisoned`.
    pub(super) fn structural_state(&self) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(ObservationStoreError::Poisoned);
        }
        self.state
            .lock()
            .expect("structural state lock")
            .clone()
            .ok_or(ObservationStoreError::StorageUnavailable)
    }

    /// Appends one immutable commit: validates the bundle, recomputes every
    /// domain hash and the pointer, proves the new root is the unique direct
    /// child of the current active root, writes the four objects, then
    /// publishes the active pointer atomically. Pre-exchange failures keep
    /// the old active bytes; post-exchange uncertainty returns
    /// `CommitIndeterminate` and poisons this handle.
    pub(super) fn commit_structural(
        &self,
        commit: FixtureStructuralCommitV1,
    ) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(ObservationStoreError::Poisoned);
        }
        let current = self.structural_state()?;
        let next = match publisher::commit(&self.storage, &current, commit) {
            Ok(next) => next,
            Err(ObservationStoreError::CommitIndeterminate) => {
                self.poisoned.store(true, Ordering::SeqCst);
                store_error!("phase=commit category=poisoned detail=indeterminate");
                return Err(ObservationStoreError::CommitIndeterminate);
            }
            Err(other) => return Err(other),
        };
        if let Some(state) = self.state.lock().expect("structural state lock").as_mut() {
            *state = next.clone();
        }
        store_info!(
            "phase=commit category=ok generation={} watermark={}",
            next.generation,
            next.event_watermark
        );
        Ok(next)
    }

    #[cfg(test)]
    pub(super) fn inject_pre_exchange_failure_once(&self) {
        self.storage.inject_pre_exchange_failure_once();
    }

    #[cfg(test)]
    pub(super) fn inject_post_exchange_sync_failure_once(&self) {
        self.storage.inject_post_exchange_sync_failure_once();
    }
}

fn kind_of(error: &LedgerStorageOpenError) -> &'static str {
    match error {
        LedgerStorageOpenError::Io(_) => "io",
        LedgerStorageOpenError::RejectedMarker => "rejected_marker",
        LedgerStorageOpenError::NamespaceAlreadyClaimed => "claimed",
        LedgerStorageOpenError::ProjectionResetPending => "projection_reset_pending",
        LedgerStorageOpenError::ProjectionResetMaintenanceRequired => "projection_reset_maintenance",
        LedgerStorageOpenError::ProjectionResetIndeterminate => "projection_reset_indeterminate",
        LedgerStorageOpenError::ProjectionResetManualIntervention => "projection_reset_manual",
        LedgerStorageOpenError::UnsupportedProjectionFormat => "projection_unsupported",
    }
}

macro_rules! store_log {
    ($level:expr, $($arg:tt)*) => {{
        #[cfg(any(test, debug_assertions))]
        println!("[obs-store] {} {}", $level, format!($($arg)*));
    }};
}
pub(super) use store_log;

macro_rules! store_error {
    ($($arg:tt)*) => {
        store_log!("error", $($arg)*)
    };
}
macro_rules! store_warn {
    ($($arg:tt)*) => {
        store_log!("warn", $($arg)*)
    };
}
macro_rules! store_info {
    ($($arg:tt)*) => {
        store_log!("info", $($arg)*)
    };
}
macro_rules! store_debug {
    ($($arg:tt)*) => {
        store_log!("debug", $($arg)*)
    };
}
pub(super) use store_debug;
pub(super) use store_error;
pub(super) use store_info;
pub(super) use store_warn;
