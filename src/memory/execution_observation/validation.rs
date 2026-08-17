//! Strict caller-request, boundary, and transition validation (ADR-0007 §3–§6).
//!
//! Pure functions only: no I/O, no clock, no store state; every failure returns
//! a typed `ObservationStoreError` and nothing panics. Check order is fixed
//! (schema → attestation → identifiers → digests → evidence lists → limits) so
//! error categories are deterministic. Stored-object self-verification lives on
//! the model types in `model.rs`.

use super::error::{
    CorruptionCategory, InvalidRequestCategory, LimitCategory, ObservationStoreError, TransitionConflictCategory,
};
use super::hash;
use super::ids::{CanonicalUuid, ExecutionAttemptKeyV1};
use super::model::*;

pub(crate) const EVIDENCE_ITEMS_PER_LIST_MAX: usize = 256;
pub(crate) const EVIDENCE_ITEMS_TOTAL_MAX: usize = 512;
pub(crate) const CANONICAL_REQUEST_MAX_BYTES: usize = 131_072;
/// All JSON integers must stay within `0..=2^53-1` (ADR-0007 §5).
pub(crate) const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

fn invalid(category: InvalidRequestCategory) -> ObservationStoreError {
    ObservationStoreError::invalid(category)
}

fn conflict(category: TransitionConflictCategory) -> ObservationStoreError {
    ObservationStoreError::conflict(category)
}

fn corrupt(category: CorruptionCategory) -> ObservationStoreError {
    ObservationStoreError::corrupt(category)
}

pub(crate) fn is_lowercase_hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn check_digest(value: &str) -> Result<(), ObservationStoreError> {
    if is_lowercase_hex64(value) {
        Ok(())
    } else {
        Err(invalid(InvalidRequestCategory::InvalidDigest))
    }
}

pub(crate) fn check_json_safe(value: u64) -> Result<(), ObservationStoreError> {
    if value > JSON_SAFE_INTEGER_MAX {
        Err(invalid(InvalidRequestCategory::UnsafeInteger))
    } else {
        Ok(())
    }
}

/// Writer stamps: sequence starts at 1, both stamps stay JSON-safe, and the
/// event ordinal stays within the 20,000-event ledger cap (§5/§6).
pub(crate) fn check_writer_stamps(sequence: u64, recorded_at_ms: u64) -> Result<(), ObservationStoreError> {
    check_json_safe(sequence)?;
    check_json_safe(recorded_at_ms)?;
    super::validate_event_count(sequence)?;
    if sequence == 0 {
        Err(corrupt(CorruptionCategory::SequenceGap))
    } else {
        Ok(())
    }
}

pub(crate) fn check_non_nil(uuid: &CanonicalUuid) -> Result<(), ObservationStoreError> {
    if uuid.is_nil() {
        Err(invalid(InvalidRequestCategory::NilUuid))
    } else {
        Ok(())
    }
}

pub(crate) fn check_key(key: &ExecutionAttemptKeyV1) -> Result<(), ObservationStoreError> {
    check_non_nil(&key.execution_id)
}

fn check_cid(value: &str) -> Result<(), ObservationStoreError> {
    if is_lowercase_hex64(value) {
        Ok(())
    } else {
        Err(invalid(InvalidRequestCategory::InvalidCid))
    }
}

fn check_cid_list(cids: &[String]) -> Result<(), ObservationStoreError> {
    if cids.len() > EVIDENCE_ITEMS_PER_LIST_MAX {
        return Err(ObservationStoreError::limit(LimitCategory::EvidenceList));
    }
    for cid in cids {
        check_cid(cid)?;
    }
    for (index, cid) in cids.iter().enumerate() {
        if cids.iter().skip(index + 1).any(|other| other == cid) {
            return Err(invalid(InvalidRequestCategory::DuplicateCid));
        }
    }
    Ok(())
}

fn check_request_size<T: serde::Serialize>(request: &T) -> Result<(), ObservationStoreError> {
    if super::canonical::to_canonical_vec(request)?.len() > CANONICAL_REQUEST_MAX_BYTES {
        Err(ObservationStoreError::limit(LimitCategory::RequestBytes))
    } else {
        Ok(())
    }
}

pub(crate) fn validate_started_request(request: &AppendStartedRequestV1) -> Result<(), ObservationStoreError> {
    if request.schema != STARTED_REQUEST_SCHEMA {
        return Err(invalid(InvalidRequestCategory::UnsupportedSchema));
    }
    if request.attestation_state != ATTESTATION_STATE {
        return Err(invalid(InvalidRequestCategory::InvalidAttestation));
    }
    check_key(&request.key)?;
    check_non_nil(request.fixture_origin.id())?;
    if let Some(role_ref) = &request.fixture_role_ref {
        check_non_nil(role_ref)?;
    }
    if let Some(session_ref) = &request.fixture_session_ref {
        check_non_nil(session_ref)?;
    }
    check_digest(&request.operation_contract_sha256)?;
    check_digest(&request.policy_sha256)?;
    check_digest(&request.runtime_sha256)?;
    check_cid_list(&request.input_evidence_cids)?;
    check_cid_list(&request.context_evidence_cids)?;
    let total = request.input_evidence_cids.len() + request.context_evidence_cids.len();
    if total > EVIDENCE_ITEMS_TOTAL_MAX {
        return Err(ObservationStoreError::limit(LimitCategory::EvidenceTotal));
    }
    check_request_size(request)
}

pub(crate) fn validate_terminal_request(request: &AppendTerminalRequestV1) -> Result<(), ObservationStoreError> {
    if request.schema != TERMINAL_REQUEST_SCHEMA {
        return Err(invalid(InvalidRequestCategory::UnsupportedSchema));
    }
    if request.attestation_state != ATTESTATION_STATE {
        return Err(invalid(InvalidRequestCategory::InvalidAttestation));
    }
    check_key(&request.key)?;
    if let Some(elapsed_ms) = request.execution_elapsed_ms {
        check_json_safe(elapsed_ms)?;
    }
    check_digest(&request.policy_sha256)?;
    check_digest(&request.runtime_sha256)?;
    check_cid_list(&request.output_evidence_cids)?;
    check_request_size(request)
}

/// The three CID lists of one attempt (input + context + output) share a single
/// 512-item budget (ADR-0007 §5); duplicates within one list are already
/// rejected per request.
pub(crate) fn validate_attempt_evidence_total(
    started: &AppendStartedRequestV1,
    terminal: &AppendTerminalRequestV1,
) -> Result<(), ObservationStoreError> {
    let total =
        started.input_evidence_cids.len() + started.context_evidence_cids.len() + terminal.output_evidence_cids.len();
    if total > EVIDENCE_ITEMS_TOTAL_MAX {
        Err(ObservationStoreError::limit(LimitCategory::EvidenceTotal))
    } else {
        Ok(())
    }
}

/// Takes the request BODY: validates and re-canonicalizes internally, never
/// trusts a caller-provided digest. Absent → Ok; same canonical digest → Ok
/// (idempotent retry); any other Started for this key → `started_already_bound`;
/// a view for a different key is an invalid transition (ADR-0007 §3).
pub(crate) fn validate_started_transition(
    request: &AppendStartedRequestV1,
    existing: Option<&FixtureAttemptViewV1>,
) -> Result<(), ObservationStoreError> {
    validate_started_request(request)?;
    let Some(view) = existing else {
        return Ok(());
    };
    view.validate()?;
    if view.key != request.key {
        return Err(corrupt(CorruptionCategory::InvalidTransition));
    }
    if view.started_request_sha256 == hash::started_request_sha256(request)? {
        Ok(())
    } else {
        Err(conflict(TransitionConflictCategory::StartedAlreadyBound))
    }
}

/// Takes the request BODY (validated and re-canonicalized internally). Enforces
/// `request.key == existing.key == bound_started.key` and that the bound
/// Started is exactly the one this view was built from. Both the first-Terminal
/// and the idempotent-Terminal path re-check key, policy, runtime, the request
/// digest, and the three-list evidence total (ADR-0007 §3/§4/§5).
pub(crate) fn validate_terminal_transition(
    request: &AppendTerminalRequestV1,
    existing: Option<&FixtureAttemptViewV1>,
    bound_started: Option<&AppendStartedRequestV1>,
) -> Result<(), ObservationStoreError> {
    validate_terminal_request(request)?;
    let Some(view) = existing else {
        return Err(conflict(TransitionConflictCategory::TerminalWithoutStarted));
    };
    let Some(started) = bound_started else {
        return Err(corrupt(CorruptionCategory::InvalidTransition));
    };
    view.validate()?;
    validate_started_request(started)?;
    if view.key != request.key || started.key != request.key {
        return Err(corrupt(CorruptionCategory::InvalidTransition));
    }
    if view.started_request_sha256 != hash::started_request_sha256(started)? {
        return Err(corrupt(CorruptionCategory::ObjectHashMismatch));
    }
    if request.policy_sha256 != started.policy_sha256 {
        return Err(conflict(TransitionConflictCategory::TerminalPolicyRebind));
    }
    if request.runtime_sha256 != started.runtime_sha256 {
        return Err(conflict(TransitionConflictCategory::TerminalRuntimeRebind));
    }
    validate_attempt_evidence_total(started, request)?;
    let request_sha256 = hash::terminal_request_sha256(request)?;
    match view.terminal_request_sha256.as_deref() {
        None => Ok(()),
        Some(bound) if bound == request_sha256 => Ok(()),
        Some(_) => Err(conflict(TransitionConflictCategory::TerminalAlreadyBound)),
    }
}

/// Writer time is non-decreasing (§6); a reversal is a typed reject.
pub(crate) fn validate_monotonic_record(previous_ms: u64, next_ms: u64) -> Result<(), ObservationStoreError> {
    check_json_safe(previous_ms)?;
    check_json_safe(next_ms)?;
    if next_ms < previous_ms {
        Err(invalid(InvalidRequestCategory::UnsafeInteger))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
