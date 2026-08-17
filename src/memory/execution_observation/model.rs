//! Fixed wire schemas for the execution-observation fixture ledger (ADR-0007 §4/§6/§7/§10).
//!
//! Every type is `deny_unknown_fields`; `None` is explicit JSON `null`, missing is an error.

mod event;
mod ledger;
mod request;

pub(crate) use event::{FixtureEventSegmentV1, StoredStartedEventV1, StoredTerminalEventV1};
pub(crate) use ledger::{FixtureAttemptViewV1, FixtureCurrentViewV1, FixtureLedgerRootV1};
pub(crate) use request::{AppendStartedRequestV1, AppendTerminalRequestV1};

use serde::{Deserialize, Serialize};

use super::error::{CorruptionCategory, InvalidRequestCategory, ObservationStoreError};
use super::ids::ExecutionAttemptKeyV1;

pub(crate) const STARTED_REQUEST_SCHEMA: &str = "plico.execution-observation.fixture-start-request/v1";
pub(crate) const TERMINAL_REQUEST_SCHEMA: &str = "plico.execution-observation.fixture-terminal-request/v1";
pub(crate) const STARTED_EVENT_SCHEMA: &str = "plico.execution-observation.fixture-started/v1";
pub(crate) const TERMINAL_EVENT_SCHEMA: &str = "plico.execution-observation.fixture-terminal/v1";
pub(crate) const SEGMENT_SCHEMA: &str = "plico.execution-observation.fixture-segment/v1";
pub(crate) const CURRENT_VIEW_SCHEMA: &str = "plico.execution-observation.fixture-current-view/v1";
pub(crate) const ROOT_SCHEMA: &str = "plico.execution-observation.fixture-root/v1";
pub(crate) const POINTER_SCHEMA: &str = "plico.execution-observation.fixture-root-pointer/v1";
pub(crate) const ATTESTATION_STATE: &str = "unverified_fixture";
pub(crate) const TRUST_CLASS: &str = "unverified_fixture_only";

fn unsupported_schema() -> ObservationStoreError {
    ObservationStoreError::corrupt(CorruptionCategory::UnsupportedStoredSchema)
}

fn bad_attestation() -> ObservationStoreError {
    ObservationStoreError::invalid(InvalidRequestCategory::InvalidAttestation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureActivePointerV1 {
    pub(crate) schema: String,
    pub(crate) root_sha256: String,
}

impl FixtureActivePointerV1 {
    /// Pointer carries only the schema literal and the root digest; ≤4 KiB.
    pub(crate) fn validate(&self, expected_root_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != POINTER_SCHEMA {
            return Err(unsupported_schema());
        }
        super::canonical::check_object_bytes(self, super::POINTER_MAX_BYTES)?;
        super::validation::check_digest(&self.root_sha256)?;
        if self.root_sha256 != expected_root_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationReceiptV1 {
    pub(crate) request_sha256: String,
    pub(crate) event_sha256: String,
    pub(crate) sequence: u64,
    pub(crate) root_generation: u64,
    pub(crate) root_sha256: String,
    pub(crate) recorded_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureAttemptObservationV1 {
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) attestation_state: String,
    pub(crate) started_receipt: ObservationReceiptV1,
    pub(crate) terminal_receipt: Option<ObservationReceiptV1>,
}
