//! Fixed wire schemas for the execution-observation fixture ledger (ADR-0007 §4/§6/§7/§10).
//!
//! Every type is `deny_unknown_fields`; nullable fields encode `None` as
//! explicit JSON `null` while a *missing* field is a deserialization error.
//! Schema and attestation strings are checked against the frozen literals.

use serde::{Deserialize, Serialize};

use super::error::{CorruptionCategory, InvalidRequestCategory, ObservationStoreError};
use super::hash;
use super::ids::{CanonicalUuid, EventKind, ExecutionAttemptKeyV1, FixtureOriginV1, TerminalOutcomeV1};
use super::validation::{check_digest, check_json_safe, check_key, check_writer_stamps};

fn unsupported_schema() -> ObservationStoreError {
    ObservationStoreError::corrupt(CorruptionCategory::UnsupportedStoredSchema)
}

fn bad_attestation() -> ObservationStoreError {
    ObservationStoreError::invalid(InvalidRequestCategory::InvalidAttestation)
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendStartedRequestV1 {
    pub(crate) schema: String,
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) fixture_origin: FixtureOriginV1,
    pub(crate) attestation_state: String,
    pub(crate) fixture_role_ref: Option<CanonicalUuid>,
    pub(crate) fixture_session_ref: Option<CanonicalUuid>,
    pub(crate) operation_contract_sha256: String,
    pub(crate) input_evidence_cids: Vec<String>,
    pub(crate) context_evidence_cids: Vec<String>,
    pub(crate) policy_sha256: String,
    pub(crate) runtime_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppendTerminalRequestV1 {
    pub(crate) schema: String,
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) attestation_state: String,
    pub(crate) outcome: TerminalOutcomeV1,
    pub(crate) output_evidence_cids: Vec<String>,
    pub(crate) execution_elapsed_ms: Option<u64>,
    pub(crate) policy_sha256: String,
    pub(crate) runtime_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredStartedEventV1 {
    pub(crate) schema: String,
    pub(crate) request: AppendStartedRequestV1,
    pub(crate) request_sha256: String,
    pub(crate) sequence: u64,
    pub(crate) root_generation: u64,
    pub(crate) recorded_at_ms: u64,
}

impl StoredStartedEventV1 {
    /// Schema literal, request validity, digest binding, writer stamps (§6).
    pub(crate) fn validate(&self) -> Result<(), ObservationStoreError> {
        if self.schema != STARTED_EVENT_SCHEMA {
            return Err(unsupported_schema());
        }
        super::validation::validate_started_request(&self.request)?;
        check_digest(&self.request_sha256)?;
        let computed = hash::started_request_sha256(&self.request)?;
        if computed != self.request_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        check_json_safe(self.root_generation)?;
        check_writer_stamps(self.sequence, self.recorded_at_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredTerminalEventV1 {
    pub(crate) schema: String,
    pub(crate) request: AppendTerminalRequestV1,
    pub(crate) request_sha256: String,
    pub(crate) sequence: u64,
    pub(crate) root_generation: u64,
    pub(crate) recorded_at_ms: u64,
}

impl StoredTerminalEventV1 {
    /// Same contract as [`StoredStartedEventV1::validate`] for terminal events.
    pub(crate) fn validate(&self) -> Result<(), ObservationStoreError> {
        if self.schema != TERMINAL_EVENT_SCHEMA {
            return Err(unsupported_schema());
        }
        super::validation::validate_terminal_request(&self.request)?;
        check_digest(&self.request_sha256)?;
        let computed = hash::terminal_request_sha256(&self.request)?;
        if computed != self.request_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        check_json_safe(self.root_generation)?;
        check_writer_stamps(self.sequence, self.recorded_at_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureEventSegmentV1 {
    pub(crate) schema: String,
    pub(crate) first_sequence: u64,
    pub(crate) last_sequence: u64,
    pub(crate) previous_segment_sha256: Option<String>,
    pub(crate) event_kind: EventKind,
    pub(crate) event_sha256: String,
}

impl FixtureEventSegmentV1 {
    /// One event per segment: `last == first`, binds the event digest (§7).
    pub(crate) fn validate(&self, expected_event_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != SEGMENT_SCHEMA {
            return Err(unsupported_schema());
        }
        if self.last_sequence != self.first_sequence {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition));
        }
        check_writer_stamps(self.first_sequence, 0)?;
        if let Some(previous) = &self.previous_segment_sha256 {
            check_digest(previous)?;
        }
        check_digest(&self.event_sha256)?;
        if self.event_sha256 != expected_event_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureAttemptViewV1 {
    pub(crate) key: ExecutionAttemptKeyV1,
    pub(crate) attestation_state: String,
    pub(crate) started_request_sha256: String,
    pub(crate) started_event_sha256: String,
    pub(crate) terminal_request_sha256: Option<String>,
    pub(crate) terminal_event_sha256: Option<String>,
}

impl FixtureAttemptViewV1 {
    /// Open = both terminal fields `null`; Terminal = both present (ADR-0007 §7).
    pub(crate) fn validate(&self) -> Result<(), ObservationStoreError> {
        if self.attestation_state != ATTESTATION_STATE {
            return Err(bad_attestation());
        }
        check_key(&self.key)?;
        check_digest(&self.started_request_sha256)?;
        check_digest(&self.started_event_sha256)?;
        match (&self.terminal_request_sha256, &self.terminal_event_sha256) {
            (None, None) => Ok(()),
            (Some(request_hash), Some(event_hash)) => {
                check_digest(request_hash)?;
                check_digest(event_hash)
            }
            _ => Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureCurrentViewV1 {
    pub(crate) schema: String,
    pub(crate) attestation_state: String,
    pub(crate) generation: u64,
    pub(crate) event_watermark: u64,
    pub(crate) attempts: Vec<FixtureAttemptViewV1>,
}

impl FixtureCurrentViewV1 {
    /// Attempts ascend by execution UUID bytes then attempt (ADR-0007 §7).
    pub(crate) fn validate(&self) -> Result<(), ObservationStoreError> {
        if self.schema != CURRENT_VIEW_SCHEMA {
            return Err(unsupported_schema());
        }
        if self.attestation_state != ATTESTATION_STATE {
            return Err(bad_attestation());
        }
        check_json_safe(self.generation)?;
        check_json_safe(self.event_watermark)?;
        for attempt in &self.attempts {
            attempt.validate()?;
        }
        for pair in self.attempts.windows(2) {
            let (left, right) = (&pair[0].key, &pair[1].key);
            if (left.execution_id.as_bytes(), left.attempt) >= (right.execution_id.as_bytes(), right.attempt) {
                return Err(ObservationStoreError::corrupt(CorruptionCategory::CurrentViewMismatch));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureLedgerRootV1 {
    pub(crate) schema: String,
    pub(crate) trust_class: String,
    pub(crate) generation: u64,
    pub(crate) previous_root_sha256: Option<String>,
    pub(crate) event_segment_head_sha256: Option<String>,
    pub(crate) event_watermark: u64,
    pub(crate) current_view_sha256: String,
    pub(crate) committed_at_ms: u64,
}

impl FixtureLedgerRootV1 {
    /// Root binds the current view digest it commits (ADR-0007 §7).
    pub(crate) fn validate(&self, expected_current_view_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != ROOT_SCHEMA || self.trust_class != TRUST_CLASS {
            return Err(unsupported_schema());
        }
        check_json_safe(self.generation)?;
        check_json_safe(self.event_watermark)?;
        check_json_safe(self.committed_at_ms)?;
        if let Some(previous) = &self.previous_root_sha256 {
            check_digest(previous)?;
        }
        if let Some(segment_head) = &self.event_segment_head_sha256 {
            check_digest(segment_head)?;
        }
        check_digest(&self.current_view_sha256)?;
        if self.current_view_sha256 != expected_current_view_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FixtureActivePointerV1 {
    pub(crate) schema: String,
    pub(crate) root_sha256: String,
}

impl FixtureActivePointerV1 {
    /// Pointer carries only the schema literal and the active root digest.
    pub(crate) fn validate(&self, expected_root_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != POINTER_SCHEMA {
            return Err(unsupported_schema());
        }
        check_digest(&self.root_sha256)?;
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
