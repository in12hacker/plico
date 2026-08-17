use serde::{Deserialize, Serialize};

use super::super::canonical::check_object_bytes;
use super::super::error::{CorruptionCategory, ObservationStoreError};
use super::super::hash;
use super::super::ids::EventKind;
use super::super::validation::{check_digest, check_writer_stamps};
use super::super::{validate_event_count, SEGMENT_MAX_BYTES};
use super::{
    unsupported_schema, AppendStartedRequestV1, AppendTerminalRequestV1, SEGMENT_SCHEMA, STARTED_EVENT_SCHEMA,
    TERMINAL_EVENT_SCHEMA,
};

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
        super::super::validation::validate_started_request(&self.request)?;
        check_digest(&self.request_sha256)?;
        let computed = hash::started_request_sha256(&self.request)?;
        if computed != self.request_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        validate_event_count(self.root_generation)?;
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
        super::super::validation::validate_terminal_request(&self.request)?;
        check_digest(&self.request_sha256)?;
        let computed = hash::terminal_request_sha256(&self.request)?;
        if computed != self.request_sha256 {
            return Err(ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch));
        }
        validate_event_count(self.root_generation)?;
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
    /// One event per segment: `last == first`, digest-bound, ≤64 KiB (§5/§7).
    pub(crate) fn validate(&self, expected_event_sha256: &str) -> Result<(), ObservationStoreError> {
        if self.schema != SEGMENT_SCHEMA {
            return Err(unsupported_schema());
        }
        check_object_bytes(self, SEGMENT_MAX_BYTES)?;
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
