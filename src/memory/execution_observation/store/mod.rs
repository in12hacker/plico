//! Durable transaction substrate (ADR-0008; milestone v53 WP2-R2).
//!
//! Sealed, crate-private. One mutex owns the whole linearization interval
//! (poison check → snapshot → validation → writes → publish → state update),
//! so sibling commits on one handle serialize and at most one returns `Ok`.

mod loader;
mod publisher;
mod slots;

#[cfg(test)]
mod tests;

use std::fmt::Display;
use std::sync::{Arc, Mutex};

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use std::io::{Error, ErrorKind};

use super::error::{CorruptionCategory, InvalidRequestCategory, ObservationStoreError};
use super::model::{
    FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1, StoredStartedEventV1, StoredTerminalEventV1,
};
use super::validation::CANONICAL_REQUEST_MAX_BYTES;

/// Envelope headroom over the largest canonical request (ADR-0008 §4).
const STORED_EVENT_MAX_BYTES: usize = CANONICAL_REQUEST_MAX_BYTES + 4_096;

/// The typed event one structural commit appends (ADR-0008 §6).
pub(super) enum FixtureStoredEventV1 {
    Started(StoredStartedEventV1),
    Terminal(StoredTerminalEventV1),
}

/// Full immutable object set of one commit; digests are recomputed, never
/// taken from the caller.
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

struct Transaction {
    state: Option<FixtureStructuralStateV1>,
    poisoned: bool,
}

/// Sealed durable store handle over the architecture-frozen CAS capability.
pub(super) struct FixtureObservationStoreV1 {
    storage: ExecutionObservationFixtureStorage,
    transaction: Mutex<Transaction>,
}

impl FixtureObservationStoreV1 {
    /// Opens (fresh: deterministically creates) the fixture ledger. The
    /// handle argument is consumed exactly once by the sealed CAS opener.
    pub(super) fn open_fixture(vault: Arc<PersonalVaultStorage>) -> Result<Self, ObservationStoreError> {
        let storage = ExecutionObservationFixtureStorage::open(vault).map_err(|error| {
            if namespace_already_claimed(&error) {
                ObservationStoreError::NamespaceAlreadyClaimed
            } else {
                ObservationStoreError::StorageUnavailable
            }
        })?;
        let state = slots::startup(&storage)?;
        Ok(Self {
            storage,
            transaction: Mutex::new(Transaction {
                state: Some(state),
                poisoned: false,
            }),
        })
    }

    /// Latest verified active-root identity; a poisoned handle fails closed.
    pub(super) fn structural_state(&self) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
        let transaction = self.lock()?;
        if transaction.poisoned {
            return Err(ObservationStoreError::Poisoned);
        }
        transaction
            .state
            .clone()
            .ok_or(ObservationStoreError::StorageUnavailable)
    }

    /// Appends one immutable commit under the full transaction lock: bundle
    /// validation, unique-direct-child proof, bounded writes, atomic pointer
    /// publish, then the in-memory update. `CommitIndeterminate` poisons the
    /// handle; pre-exchange failures leave the old active bytes intact.
    pub(super) fn commit_structural(
        &self,
        commit: FixtureStructuralCommitV1,
    ) -> Result<FixtureStructuralStateV1, ObservationStoreError> {
        let mut transaction = self.lock()?;
        if transaction.poisoned {
            return Err(ObservationStoreError::Poisoned);
        }
        let current = transaction
            .state
            .clone()
            .ok_or(ObservationStoreError::StorageUnavailable)?;
        match publisher::commit(&self.storage, &current, commit) {
            Ok(next) => {
                transaction.state = Some(next.clone());
                Ok(next)
            }
            Err(ObservationStoreError::CommitIndeterminate) => {
                transaction.poisoned = true;
                Err(ObservationStoreError::CommitIndeterminate)
            }
            Err(other) => Err(other),
        }
    }

    #[cfg(test)]
    pub(super) fn inject_pre_exchange_failure_once(&self) {
        self.storage.inject_pre_exchange_failure_once();
    }

    #[cfg(test)]
    pub(super) fn inject_post_exchange_sync_failure_once(&self) {
        self.storage.inject_post_exchange_sync_failure_once();
    }

    /// Typed fail-closed lock acquisition: a poisoned mutex (a writer panicked
    /// mid-transaction) reports `Poisoned` instead of panicking here.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Transaction>, ObservationStoreError> {
        self.transaction.lock().map_err(|_| ObservationStoreError::Poisoned)
    }
}

/// The opener's claim error is classified through its frozen message prefix;
/// the error type itself is outside the WP2 import allowlist.
fn namespace_already_claimed(error: &impl Display) -> bool {
    error
        .to_string()
        .starts_with("immutable ledger namespace is already claimed")
}

/// io→typed: absent=chain corruption, over-cap=stored limit, rest=availability.
fn map_read_io(error: Error, missing: CorruptionCategory) -> ObservationStoreError {
    match error.kind() {
        ErrorKind::NotFound => corrupt(missing),
        ErrorKind::InvalidData => corrupt(CorruptionCategory::StoredResourceLimit),
        _ => ObservationStoreError::StorageUnavailable,
    }
}

fn map_stored_parse(error: ObservationStoreError) -> ObservationStoreError {
    match error {
        ObservationStoreError::InvalidRequest { category } => corrupt(match category {
            InvalidRequestCategory::UnsupportedSchema | InvalidRequestCategory::InvalidAttestation => {
                CorruptionCategory::UnsupportedStoredSchema
            }
            _ => CorruptionCategory::ObjectHashMismatch,
        }),
        ObservationStoreError::LimitExceeded { .. } => corrupt(CorruptionCategory::StoredResourceLimit),
        ObservationStoreError::TransitionConflict { .. } => corrupt(CorruptionCategory::InvalidTransition),
        stored => stored,
    }
}

fn corrupt(category: CorruptionCategory) -> ObservationStoreError {
    ObservationStoreError::corrupt(category)
}
