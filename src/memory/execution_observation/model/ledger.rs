use serde::{Deserialize, Serialize};

use super::super::canonical::check_object_bytes;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::ids::ExecutionAttemptKeyV1;
use super::super::validation::{check_digest, check_json_safe, check_key};
use super::super::{validate_attempt_count, validate_event_count, CURRENT_VIEW_MAX_BYTES, ROOT_MAX_BYTES};
use super::{bad_attestation, unsupported_schema, ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, ROOT_SCHEMA, TRUST_CLASS};

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
    /// Ascending attempts (§7); caps: 8 MiB, ≤10,000 attempts, ≤20,000 events.
    pub(crate) fn validate(&self) -> Result<(), ObservationStoreError> {
        if self.schema != CURRENT_VIEW_SCHEMA {
            return Err(unsupported_schema());
        }
        check_object_bytes(self, CURRENT_VIEW_MAX_BYTES)?;
        validate_attempt_count(self.attempts.len())?;
        if self.attestation_state != ATTESTATION_STATE {
            return Err(bad_attestation());
        }
        validate_event_count(self.generation)?;
        validate_event_count(self.event_watermark)?;
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
    /// Root binds the current view digest (§7); 64 KiB + watermark ≤20,000 (§5).
    pub(crate) fn validate(&self, expected_current_view_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != ROOT_SCHEMA || self.trust_class != TRUST_CLASS {
            return Err(unsupported_schema());
        }
        check_object_bytes(self, ROOT_MAX_BYTES)?;
        validate_event_count(self.generation)?;
        validate_event_count(self.event_watermark)?;
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
