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
        ObservationStoreError::corrupt(CorruptionCategory::StoredResourceLimit).to_string(),
        "corrupt_store/stored_resource_limit"
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
