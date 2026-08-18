//! Deterministic read facade (ADR-0009; milestone v53 WP3A).
//!
//! Sealed, crate-private, strictly read-only: borrows the architecture-frozen
//! existing-only CAS read capability, independently replays the authoritative
//! active chain through the single pure reducer, verifies the stored current
//! view against that replay, and serves `read_attempt` lookups from the
//! verified result. No append, receipt, clock, repair, CAS paths, or
//! production wiring.
//!
//! Read seam (WP3A.2-A `ExistingExecutionObservationReadOnly`): an absent
//! namespace is an empty ledger, a present-but-damaged topology fails closed
//! without repair, and opening never creates, completes, chmods, or claims
//! anything — a writer may hold or later take the namespace claim on the
//! same vault Arc.

#![allow(dead_code)] // WP3A has no production caller by contract; wiring belongs to a later authorized package

mod reducer;
mod replay;

#[cfg(test)]
mod readonly_tests;
#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::cas::PersonalVaultStorage;

use super::error::ObservationStoreError;
use super::model::{FixtureAttemptObservationV1, ObservationReceiptV1, ATTESTATION_STATE};

#[derive(Debug)]
pub(crate) struct FixtureObservationReaderV1 {
    attempts: Vec<reducer::ReducibleAttemptV1>,
}

impl FixtureObservationReaderV1 {
    /// Opens the existing fixture ledger through the existing-only readonly
    /// capability and replays it once; an absent namespace replays as an
    /// empty ledger. The vault handle is only borrowed for the closure, and
    /// the reader keeps no storage handle and no write capability of any
    /// kind. Open-phase I/O and topology failures are storage-level for the
    /// reader (the WP2 corpus pins the same open-phase classification).
    pub(crate) fn open_fixture(vault: Arc<PersonalVaultStorage>) -> Result<Self, ObservationStoreError> {
        let attempts = vault
            .with_existing_execution_observation_readonly(|view| match view {
                None => Ok(Vec::new()),
                Some(view) => replay::replay(&view),
            })
            .map_err(|_| ObservationStoreError::StorageUnavailable)??;
        Ok(Self { attempts })
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
