//! Deterministic read facade (ADR-0009; milestone v53 WP3A).
//!
//! Sealed, crate-private, strictly read-only: opens the frozen sealed CAS
//! capability, independently replays the authoritative active chain through
//! the single pure reducer, verifies the stored current view against that
//! replay, and serves `read_attempt` lookups from the verified result.
//! No append, receipt, clock, repair, CAS paths, or production wiring.
//!
//! Inherited seam behavior (accurate as of this baseline): the only available
//! sealed opener is the WRITER opener — it creates the observation namespace
//! directories on a fresh vault (no open-existing-only capability exists yet)
//! and registers a writer namespace claim that persists for the whole vault
//! lifecycle even after this reader is dropped. Consequences: opening a
//! reader on a fresh vault mutates the vault, and a caller that keeps the
//! same `Arc<PersonalVaultStorage>` cannot open a writer afterwards
//! (`NamespaceAlreadyClaimed`). An existing-only read capability is the
//! architecture group's WP3A.1 precondition (Architecture Deviation filed).

#![allow(dead_code)] // WP3A has no production caller by contract; wiring belongs to a later authorized package

mod reducer;
mod replay;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::cas::execution_observation_store::ExecutionObservationFixtureStorage;
use crate::cas::PersonalVaultStorage;

use super::error::ObservationStoreError;
use super::model::{FixtureAttemptObservationV1, ObservationReceiptV1, ATTESTATION_STATE};

#[derive(Debug)]
pub(crate) struct FixtureObservationReaderV1 {
    attempts: Vec<reducer::ReducibleAttemptV1>,
}

impl FixtureObservationReaderV1 {
    /// Opens the fixture ledger and replays it once. The vault handle is
    /// consumed by the sealed CAS opener and dropped with it afterwards; the
    /// reader keeps no storage handle and no write capability of any kind.
    pub(crate) fn open_fixture(vault: Arc<PersonalVaultStorage>) -> Result<Self, ObservationStoreError> {
        let storage = ExecutionObservationFixtureStorage::open(vault).map_err(|error| {
            if error
                .to_string()
                .starts_with("immutable ledger namespace is already claimed")
            {
                ObservationStoreError::NamespaceAlreadyClaimed
            } else {
                ObservationStoreError::StorageUnavailable
            }
        })?;
        let ledger = replay::replay(&storage)?;
        Ok(Self { attempts: ledger })
    }

    /// Reads one attempt's rebuilt observation. `None` when the attempt key is
    /// absent from the authoritative chain; the result always carries
    /// `attestation_state = unverified_fixture` (ADR-0009).
    pub(crate) fn read_attempt(
        &self,
        key: &super::ids::ExecutionAttemptKeyV1,
    ) -> Result<Option<FixtureAttemptObservationV1>, ObservationStoreError> {
        let needle = (key.execution_id.as_bytes(), key.attempt.get());
        let index = self
            .attempts
            .binary_search_by(|attempt| reducer::attempt_ordering(attempt, needle));
        Ok(index.ok().map(|index| observation_from(&self.attempts[index])))
    }
}

fn receipt_from(parts: &reducer::ReducibleReceiptV1) -> ObservationReceiptV1 {
    ObservationReceiptV1 {
        request_sha256: parts.request_sha256.clone(),
        event_sha256: parts.event_sha256.clone(),
        sequence: parts.sequence,
        root_generation: parts.root_generation,
        root_sha256: parts.root_sha256.clone(),
        recorded_at_ms: parts.recorded_at_ms,
    }
}

fn observation_from(attempt: &reducer::ReducibleAttemptV1) -> FixtureAttemptObservationV1 {
    FixtureAttemptObservationV1 {
        key: attempt.key,
        attestation_state: ATTESTATION_STATE.to_string(),
        started_receipt: receipt_from(&attempt.started),
        terminal_receipt: attempt.terminal.as_ref().map(receipt_from),
    }
}
