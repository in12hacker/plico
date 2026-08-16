//! Strict caller-request, boundary, and transition validation (ADR-0007 §3–§6).
//!
//! Pure functions only: no I/O, no clock, no store state; every failure returns
//! a typed `ObservationStoreError` and nothing panics. Check order is fixed
//! (schema → attestation → identifiers → digests → evidence lists → limits) so
//! error categories are deterministic. Stored-object self-verification lives on
//! the model types in `model.rs`.

use super::error::{InvalidRequestCategory, LimitCategory, ObservationStoreError, TransitionConflictCategory};
use super::ids::{CanonicalUuid, ExecutionAttemptKeyV1, FailureCategoryV1};
use super::model::*;

pub(crate) const EVIDENCE_ITEMS_PER_LIST_MAX: usize = 256;
pub(crate) const EVIDENCE_ITEMS_TOTAL_MAX: usize = 512;
pub(crate) const CANONICAL_REQUEST_MAX_BYTES: usize = 131_072;
/// All JSON integers must stay within `0..=2^53-1` (ADR-0007 §5).
pub(crate) const JSON_SAFE_INTEGER_MAX: u64 = 9_007_199_254_740_991;

fn invalid(category: InvalidRequestCategory) -> ObservationStoreError {
    ObservationStoreError::invalid(category)
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

/// Writer stamps: sequence starts at 1 and both sequence and record time stay
/// JSON-safe (ADR-0007 §5/§6).
pub(crate) fn check_writer_stamps(sequence: u64, recorded_at_ms: u64) -> Result<(), ObservationStoreError> {
    check_json_safe(sequence)?;
    check_json_safe(recorded_at_ms)?;
    if sequence == 0 {
        Err(ObservationStoreError::corrupt(
            super::error::CorruptionCategory::SequenceGap,
        ))
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

pub(crate) fn validate_failure_category(value: &str) -> Result<FailureCategoryV1, ObservationStoreError> {
    match value {
        "invalid_input" => Ok(FailureCategoryV1::InvalidInput),
        "policy_denied" => Ok(FailureCategoryV1::PolicyDenied),
        "dependency_unavailable" => Ok(FailureCategoryV1::DependencyUnavailable),
        "executor_rejected" => Ok(FailureCategoryV1::ExecutorRejected),
        "executor_failed" => Ok(FailureCategoryV1::ExecutorFailed),
        "executor_panicked" => Ok(FailureCategoryV1::ExecutorPanicked),
        "tool_failed" => Ok(FailureCategoryV1::ToolFailed),
        "internal" => Ok(FailureCategoryV1::Internal),
        _ => Err(invalid(InvalidRequestCategory::InvalidFailureCategory)),
    }
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

/// Absent → Ok; same canonical request digest → Ok (idempotent retry); any other
/// Started for the same key → `started_already_bound` (ADR-0007 §3).
pub(crate) fn validate_started_transition(
    request_sha256: &str,
    existing: Option<&FixtureAttemptViewV1>,
) -> Result<(), ObservationStoreError> {
    match existing {
        None => Ok(()),
        Some(view) if view.started_request_sha256 == request_sha256 => Ok(()),
        Some(_) => Err(ObservationStoreError::conflict(
            TransitionConflictCategory::StartedAlreadyBound,
        )),
    }
}

/// Terminal-without-Started, double Terminal, and policy/runtime rebind are typed
/// conflicts. `bound_started` must be present whenever `existing` is.
pub(crate) fn validate_terminal_transition(
    request: &AppendTerminalRequestV1,
    request_sha256: &str,
    existing: Option<&FixtureAttemptViewV1>,
    bound_started: Option<&AppendStartedRequestV1>,
) -> Result<(), ObservationStoreError> {
    let Some(view) = existing else {
        return Err(ObservationStoreError::conflict(
            TransitionConflictCategory::TerminalWithoutStarted,
        ));
    };
    if let Some(bound_terminal) = &view.terminal_request_sha256 {
        return if bound_terminal == request_sha256 {
            Ok(())
        } else {
            Err(ObservationStoreError::conflict(
                TransitionConflictCategory::TerminalAlreadyBound,
            ))
        };
    }
    let Some(started) = bound_started else {
        return Err(ObservationStoreError::corrupt(
            super::error::CorruptionCategory::InvalidTransition,
        ));
    };
    if request.policy_sha256 != started.policy_sha256 {
        return Err(ObservationStoreError::conflict(
            TransitionConflictCategory::TerminalPolicyRebind,
        ));
    }
    if request.runtime_sha256 != started.runtime_sha256 {
        return Err(ObservationStoreError::conflict(
            TransitionConflictCategory::TerminalRuntimeRebind,
        ));
    }
    Ok(())
}

/// Writer time is non-decreasing: `max(system_now_ms, previous_recorded_at_ms)`
/// (ADR-0007 §6). A reversal is a typed reject, never an auto-rewrite.
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
mod tests {
    use super::super::error::CorruptionCategory;
    use super::super::hash::tests::{flow, golden_started_request, golden_terminal_request};
    use super::*;

    fn unique_cids(from: u64, count: usize) -> Vec<String> {
        (from..from + count as u64)
            .map(|value| format!("{value:064x}"))
            .collect()
    }

    #[test]
    fn execution_observation_evidence_list_and_total_limits() {
        let mut request = golden_started_request();
        request.input_evidence_cids = unique_cids(1, EVIDENCE_ITEMS_PER_LIST_MAX);
        request.context_evidence_cids = unique_cids(257, EVIDENCE_ITEMS_PER_LIST_MAX);
        validate_started_request(&request).expect("512 combined items at the boundary");
        request.input_evidence_cids = unique_cids(1, EVIDENCE_ITEMS_PER_LIST_MAX + 1);
        assert_eq!(
            validate_started_request(&request),
            Err(ObservationStoreError::limit(LimitCategory::EvidenceList))
        );
        request.input_evidence_cids = unique_cids(1, EVIDENCE_ITEMS_PER_LIST_MAX);
        let started = request;
        assert_eq!(
            validate_attempt_evidence_total(&started, &golden_terminal_request()),
            Err(ObservationStoreError::limit(LimitCategory::EvidenceTotal))
        );
        flow("logic.limits per-list=257 -> evidence_list_limit; attempt-total=513 -> evidence_total_limit");
        let mut empty_terminal = golden_terminal_request();
        empty_terminal.output_evidence_cids = Vec::new();
        validate_attempt_evidence_total(&started, &empty_terminal).expect("512 total at the boundary");
        flow("logic.limits per-list=256 total=512 attempt-total=512 -> ok");
    }

    #[test]
    fn execution_observation_validation_integer_and_time_boundaries() {
        let mut terminal = golden_terminal_request();
        terminal.execution_elapsed_ms = Some(0);
        validate_terminal_request(&terminal).expect("zero elapsed is allowed");
        let mut event = super::super::tests::golden_chain().started_event;
        event.recorded_at_ms = JSON_SAFE_INTEGER_MAX;
        event.validate().expect("recorded time at 2^53-1");
        flow("logic.boundaries elapsed_ms=0 recorded_at_ms=2^53-1 -> ok");
        event.sequence = 0;
        assert_eq!(
            event.validate(),
            Err(ObservationStoreError::corrupt(CorruptionCategory::SequenceGap))
        );
        flow("logic.boundaries sequence=0 -> corrupt_store/sequence_gap");
        event.sequence = JSON_SAFE_INTEGER_MAX + 1;
        assert_eq!(event.validate(), Err(invalid(InvalidRequestCategory::UnsafeInteger)));
        flow("logic.boundaries sequence=2^53 -> invalid_request/unsafe_integer");
    }
}
