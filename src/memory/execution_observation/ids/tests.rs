use super::*;

const CANONICAL: &str = "123e4567-e89b-42d3-a456-426614174000";

#[test]
fn execution_observation_ids_canonical_uuid_accepted() {
    let value = CanonicalUuid::from_canonical_str(CANONICAL).expect("canonical form");
    assert!(!value.is_nil());
    let wire = serde_json::to_string(&value).expect("serialize");
    assert_eq!(wire, format!("\"{CANONICAL}\""));
    let parsed: CanonicalUuid = serde_json::from_str(&wire).expect("deserialize");
    assert_eq!(parsed, value);
    super::super::hash::tests::flow("data.ids uuid wire=36-byte lowercase hyphenated -> serialize/parse roundtrip ok");
}

#[test]
fn execution_observation_ids_noncanonical_uuid_wire_rejected() {
    for malformed in [
        "123E4567-E89B-42D3-A456-426614174000",
        "123e4567e89b42d3a456426614174000",
        "{123e4567-e89b-42d3-a456-426614174000}",
        "urn:uuid:123e4567-e89b-42d3-a456-426614174000",
        "123e4567-e89b-42d3-a456-4266141740",
        "123e4567-e89b-42d3-a456-42661417400g",
        "",
    ] {
        assert!(
            serde_json::from_str::<CanonicalUuid>(&format!("\"{malformed}\"")).is_err(),
            "expected rejection: {malformed}"
        );
        assert!(CanonicalUuid::from_canonical_str(malformed).is_none());
        super::super::hash::tests::flow("logic.ids non-canonical uuid -> wire reject at deserialize");
    }
}

#[test]
fn execution_observation_ids_nil_uuid_parses_for_typed_validation() {
    let nil =
        CanonicalUuid::from_canonical_str("00000000-0000-0000-0000-000000000000").expect("nil has canonical form");
    assert!(nil.is_nil());
}

#[test]
fn execution_observation_ids_zero_attempt_is_typed_error() {
    let execution_id = CanonicalUuid::from_canonical_str(CANONICAL).expect("canonical form");
    let error = ExecutionAttemptKeyV1::from_parts(execution_id, 0).unwrap_err();
    assert_eq!(
        error,
        ObservationStoreError::InvalidRequest {
            category: InvalidRequestCategory::ZeroAttempt
        }
    );
}

#[test]
fn execution_observation_ids_key_rejects_unknown_fields_and_nulls() {
    let json = format!("{{\"execution_id\":\"{CANONICAL}\",\"attempt\":1,\"extra\":2}}");
    assert!(serde_json::from_str::<ExecutionAttemptKeyV1>(&json).is_err());
    let json = format!("{{\"execution_id\":\"{CANONICAL}\"}}");
    assert!(serde_json::from_str::<ExecutionAttemptKeyV1>(&json).is_err());
    let json = "{\"execution_id\":null,\"attempt\":1}";
    assert!(serde_json::from_str::<ExecutionAttemptKeyV1>(json).is_err());
}

#[test]
fn execution_observation_ids_outcome_wire_forms() {
    let cases = [
        (TerminalOutcomeV1::Success, r#"{"type":"success"}"#),
        (
            TerminalOutcomeV1::Failure {
                category: FailureCategoryV1::ToolFailed,
            },
            // serde emits the tag first; JCS sorting (category < type) is asserted in tests.rs
            r#"{"type":"failure","category":"tool_failed"}"#,
        ),
        (TerminalOutcomeV1::Timeout, r#"{"type":"timeout"}"#),
        (TerminalOutcomeV1::Cancelled, r#"{"type":"cancelled"}"#),
        (TerminalOutcomeV1::Indeterminate, r#"{"type":"indeterminate"}"#),
    ];
    for (outcome, wire) in cases {
        assert_eq!(serde_json::to_string(&outcome).unwrap(), wire);
        assert_eq!(serde_json::from_str::<TerminalOutcomeV1>(wire).unwrap(), outcome);
    }
    for (wire, category) in [
        ("invalid_input", FailureCategoryV1::InvalidInput),
        ("dependency_unavailable", FailureCategoryV1::DependencyUnavailable),
        ("executor_panicked", FailureCategoryV1::ExecutorPanicked),
        ("internal", FailureCategoryV1::Internal),
    ] {
        assert_eq!(serde_json::to_string(&category).unwrap(), format!("\"{wire}\""));
        assert_eq!(
            serde_json::from_str::<FailureCategoryV1>(&format!("\"{wire}\"")).unwrap(),
            category
        );
    }
    assert!(serde_json::from_str::<TerminalOutcomeV1>(r#"{"type":"success","extra":1}"#).is_err());
    assert!(serde_json::from_str::<TerminalOutcomeV1>(r#"{"type":"unknown"}"#).is_err());
}

#[test]
fn execution_observation_ids_origin_wire_forms() {
    let origin = FixtureOriginV1::IntentDispatch {
        intent_id: CanonicalUuid::from_canonical_str(CANONICAL).unwrap(),
    };
    assert_eq!(
        serde_json::to_string(&origin).unwrap(),
        format!("{{\"type\":\"intent_dispatch\",\"intent_id\":\"{CANONICAL}\"}}")
    );
    assert!(serde_json::from_str::<FixtureOriginV1>(&format!(
        "{{\"type\":\"intent_dispatch\",\"intent_id\":\"{CANONICAL}\",\"x\":1}}"
    ))
    .is_err());
    assert!(serde_json::from_str::<FixtureOriginV1>(&format!(
        "{{\"type\":\"internal_task\",\"task_id\":\"{CANONICAL}\",\"request_id\":\"{CANONICAL}\"}}"
    ))
    .is_err());
}

#[test]
fn execution_observation_ids_request_nil_uuid_typed_reject() {
    use super::super::error::{InvalidRequestCategory, ObservationStoreError};
    use super::super::hash::tests::{golden_started_request, uuid};
    use super::super::validation::validate_started_request;

    let nil = uuid("00000000-0000-0000-0000-000000000000");
    let expected = Err(ObservationStoreError::invalid(InvalidRequestCategory::NilUuid));

    let mut request = golden_started_request();
    request.key.execution_id = nil;
    assert_eq!(validate_started_request(&request), expected);

    let mut request = golden_started_request();
    request.fixture_origin = FixtureOriginV1::PublicRequest { request_id: nil };
    assert_eq!(validate_started_request(&request), expected);

    let mut request = golden_started_request();
    request.fixture_role_ref = Some(nil);
    assert_eq!(validate_started_request(&request), expected);
}
