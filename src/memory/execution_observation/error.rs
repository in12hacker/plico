//! Frozen error contract for the execution-observation fixture ledger (ADR-0007 §11).
//!
//! Display and trace output may only carry the stable category wire name; request
//! bodies, full digests, role refs, host paths, and underlying storage messages
//! must never be formatted into errors.

use std::fmt;

use super::ids::FailureCategoryV1;

/// Typed store error. The I/O fault variants (`StorageUnavailable`,
/// `NamespaceAlreadyClaimed`, `CommitIndeterminate`, `Poisoned`) are frozen here as
/// part of the v1 contract; the pure WP1 core only ever produces the four
/// category-carrying variants.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum ObservationStoreError {
    #[error("invalid_request/{category}")]
    InvalidRequest { category: InvalidRequestCategory },
    #[error("transition_conflict/{category}")]
    TransitionConflict { category: TransitionConflictCategory },
    #[error("limit_exceeded/{category}")]
    LimitExceeded { category: LimitCategory },
    #[error("corrupt_store/{category}")]
    CorruptStore { category: CorruptionCategory },
    #[error("storage_unavailable")]
    StorageUnavailable,
    #[error("namespace_already_claimed")]
    NamespaceAlreadyClaimed,
    #[error("commit_indeterminate")]
    CommitIndeterminate,
    #[error("poisoned")]
    Poisoned,
}

impl ObservationStoreError {
    pub(crate) fn invalid(category: InvalidRequestCategory) -> Self {
        Self::InvalidRequest { category }
    }

    pub(crate) fn conflict(category: TransitionConflictCategory) -> Self {
        Self::TransitionConflict { category }
    }

    pub(crate) fn limit(category: LimitCategory) -> Self {
        Self::LimitExceeded { category }
    }

    pub(crate) fn corrupt(category: CorruptionCategory) -> Self {
        Self::CorruptStore { category }
    }
}

/// Closed set of caller-input rejection categories (ADR-0007 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvalidRequestCategory {
    UnsupportedSchema,
    InvalidAttestation,
    NilUuid,
    ZeroAttempt,
    InvalidDigest,
    InvalidCid,
    DuplicateCid,
    InvalidFailureCategory,
    UnsafeInteger,
    SizeLimitExceeded,
    JcsCanonicalizationFailed,
}

impl InvalidRequestCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSchema => "unsupported_schema",
            Self::InvalidAttestation => "invalid_attestation",
            Self::NilUuid => "nil_uuid",
            Self::ZeroAttempt => "zero_attempt",
            Self::InvalidDigest => "invalid_digest",
            Self::InvalidCid => "invalid_cid",
            Self::DuplicateCid => "duplicate_cid",
            Self::InvalidFailureCategory => "invalid_failure_category",
            Self::UnsafeInteger => "unsafe_integer",
            Self::SizeLimitExceeded => "size_limit_exceeded",
            Self::JcsCanonicalizationFailed => "jcs_canonicalization_failed",
        }
    }
}

impl fmt::Display for InvalidRequestCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Closed set of attempt-state transition conflict categories (ADR-0007 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransitionConflictCategory {
    StartedAlreadyBound,
    TerminalWithoutStarted,
    TerminalAlreadyBound,
    TerminalPolicyRebind,
    TerminalRuntimeRebind,
}

impl TransitionConflictCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::StartedAlreadyBound => "started_already_bound",
            Self::TerminalWithoutStarted => "terminal_without_started",
            Self::TerminalAlreadyBound => "terminal_already_bound",
            Self::TerminalPolicyRebind => "terminal_policy_rebind",
            Self::TerminalRuntimeRebind => "terminal_runtime_rebind",
        }
    }
}

impl fmt::Display for TransitionConflictCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Closed set of ledger capacity limit categories (ADR-0007 §11). Variant
/// identifiers drop the shared `Limit` postfix; `as_str` carries the frozen
/// wire names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LimitCategory {
    Attempt,
    Event,
    EvidenceList,
    EvidenceTotal,
    RequestBytes,
    ObjectBytes,
}

impl LimitCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Attempt => "attempt_limit",
            Self::Event => "event_limit",
            Self::EvidenceList => "evidence_list_limit",
            Self::EvidenceTotal => "evidence_total_limit",
            Self::RequestBytes => "request_bytes_limit",
            Self::ObjectBytes => "object_bytes_limit",
        }
    }
}

impl fmt::Display for LimitCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Closed set of stored-ledger corruption categories (ADR-0007 §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CorruptionCategory {
    MissingActivePointer,
    NoncanonicalPointer,
    UnsupportedStoredSchema,
    ObjectHashMismatch,
    BrokenRootChain,
    BrokenSegmentChain,
    SequenceGap,
    GenerationMismatch,
    DuplicateStarted,
    DuplicateTerminal,
    InvalidTransition,
    CurrentViewMismatch,
    InvalidCandidateState,
}

impl CorruptionCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::MissingActivePointer => "missing_active_pointer",
            Self::NoncanonicalPointer => "noncanonical_pointer",
            Self::UnsupportedStoredSchema => "unsupported_stored_schema",
            Self::ObjectHashMismatch => "object_hash_mismatch",
            Self::BrokenRootChain => "broken_root_chain",
            Self::BrokenSegmentChain => "broken_segment_chain",
            Self::SequenceGap => "sequence_gap",
            Self::GenerationMismatch => "generation_mismatch",
            Self::DuplicateStarted => "duplicate_started",
            Self::DuplicateTerminal => "duplicate_terminal",
            Self::InvalidTransition => "invalid_transition",
            Self::CurrentViewMismatch => "current_view_mismatch",
            Self::InvalidCandidateState => "invalid_candidate_state",
        }
    }
}

impl fmt::Display for CorruptionCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Closed failure-category wire set (ADR-0007 §4): unknown strings are typed
/// rejects; there is no `unknown` or free-text category.
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
        _ => Err(ObservationStoreError::invalid(
            InvalidRequestCategory::InvalidFailureCategory,
        )),
    }
}

/// Hand-rolled strict wire form so an unknown category string surfaces the
/// frozen `invalid_failure_category` instead of a generic parse error. The
/// error's stable Display doubles as the classification marker that
/// `canonical::parse_canonical` recognizes on the wire boundary.
impl<'de> serde::Deserialize<'de> for FailureCategoryV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = <&str>::deserialize(deserializer)?;
        validate_failure_category(wire).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_observation_error_display_is_stable_category_only() {
        assert_eq!(
            ObservationStoreError::invalid(InvalidRequestCategory::NilUuid).to_string(),
            "invalid_request/nil_uuid"
        );
        assert_eq!(
            ObservationStoreError::conflict(TransitionConflictCategory::TerminalPolicyRebind).to_string(),
            "transition_conflict/terminal_policy_rebind"
        );
        assert_eq!(
            ObservationStoreError::limit(LimitCategory::EvidenceList).to_string(),
            "limit_exceeded/evidence_list_limit"
        );
        assert_eq!(
            ObservationStoreError::corrupt(CorruptionCategory::ObjectHashMismatch).to_string(),
            "corrupt_store/object_hash_mismatch"
        );
        assert_eq!(
            ObservationStoreError::StorageUnavailable.to_string(),
            "storage_unavailable"
        );
        assert_eq!(
            ObservationStoreError::NamespaceAlreadyClaimed.to_string(),
            "namespace_already_claimed"
        );
        assert_eq!(
            ObservationStoreError::CommitIndeterminate.to_string(),
            "commit_indeterminate"
        );
        assert_eq!(ObservationStoreError::Poisoned.to_string(), "poisoned");
        super::super::hash::tests::flow("logic.error display: 8 variants -> stable category strings only");
    }

    #[test]
    fn execution_observation_error_failure_category_closed_set() {
        use super::super::hash::tests::flow;

        for wire in [
            "invalid_input",
            "policy_denied",
            "dependency_unavailable",
            "executor_rejected",
            "executor_failed",
            "executor_panicked",
            "tool_failed",
            "internal",
        ] {
            validate_failure_category(wire).expect("closed-set member");
        }
        assert_eq!(
            validate_failure_category("unknown"),
            Err(ObservationStoreError::invalid(
                InvalidRequestCategory::InvalidFailureCategory
            ))
        );
        flow("logic.failure-category closed-set=8 accepted; unknown -> invalid_request/invalid_failure_category");
    }
}
