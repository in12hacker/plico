use super::*;

#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    b: u32,
    a: String,
}

#[test]
fn execution_observation_canonical_roundtrip() {
    let sample = Sample { b: 1, a: "x".into() };
    let bytes = to_canonical_vec(&sample).expect("canonicalize");
    assert_eq!(bytes, br#"{"a":"x","b":1}"#);
    let parsed: Sample = parse_canonical(&bytes).expect("parse");
    assert_eq!(parsed, sample);
    super::super::hash::tests::flow(format!("data.canonical roundtrip {}B byte-equal ok", bytes.len()));
}

#[test]
fn execution_observation_canonical_rejects_noncanonical_and_unknown() {
    let cases: [&[u8]; 4] = [
        br#"{"b":1,"a":"x"}"#,                // key order not JCS-sorted
        b"{\n  \"a\": \"x\",\n  \"b\": 1\n}", // whitespace
        br#"{"a":"x","b":1,"c":0}"#,          // unknown field
        br#"{"a":"x"}"#,                      // missing field
    ];
    for (index, case) in cases.iter().enumerate() {
        assert_eq!(parse_canonical::<Sample>(case).unwrap_err(), jcs_error());
        super::super::hash::tests::flow(format!(
            "data.canonical case={index} -> invalid_request/jcs_canonicalization_failed"
        ));
    }
}

#[test]
fn execution_observation_canonical_missing_versus_null_nullable_fields() {
    use super::super::hash::tests::{flow, golden_started_request, golden_terminal_request, hex64};
    use super::super::model::{AppendStartedRequestV1, AppendTerminalRequestV1};

    let started_text = String::from_utf8(to_canonical_vec(&golden_started_request()).unwrap()).unwrap();
    assert!(started_text.contains("\"fixture_role_ref\":null,\"fixture_session_ref\":null"));
    let parsed: AppendStartedRequestV1 = parse_canonical(started_text.as_bytes()).expect("explicit nulls parse");
    assert!(parsed.fixture_role_ref.is_none() && parsed.fixture_session_ref.is_none());
    flow(format!(
        "data.canonical started bytes={} explicit-nulls ok",
        started_text.len()
    ));
    let missing_role = started_text.replacen("\"fixture_role_ref\":null,", "", 1);
    assert!(parse_canonical::<AppendStartedRequestV1>(missing_role.as_bytes()).is_err());
    flow("data.canonical nullable missing -> reject");
    let null_policy = started_text.replacen(
        &format!("\"policy_sha256\":\"{}\"", hex64('b')),
        "\"policy_sha256\":null",
        1,
    );
    assert!(parse_canonical::<AppendStartedRequestV1>(null_policy.as_bytes()).is_err());
    flow("data.canonical non-nullable policy_sha256 = null -> reject");
    let terminal_text = String::from_utf8(to_canonical_vec(&golden_terminal_request()).unwrap()).unwrap();
    assert!(terminal_text.contains("\"execution_elapsed_ms\":null"));
    let missing_elapsed = terminal_text.replacen("\"execution_elapsed_ms\":null,", "", 1);
    assert!(parse_canonical::<AppendTerminalRequestV1>(missing_elapsed.as_bytes()).is_err());
    flow("data.canonical nullable execution_elapsed_ms missing -> reject");
}

#[test]
fn execution_observation_counterexample_wire_zero_attempt() {
    use super::super::error::InvalidRequestCategory;
    use super::super::hash::tests::{flow, golden_started_request, golden_terminal_request};
    use super::super::model::{AppendStartedRequestV1, AppendTerminalRequestV1};
    let text = String::from_utf8(to_canonical_vec(&golden_started_request()).unwrap()).unwrap();
    let zero_attempt = text.replacen("\"attempt\":1", "\"attempt\":0", 1);
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(zero_attempt.as_bytes()),
        Err(ObservationStoreError::invalid(InvalidRequestCategory::ZeroAttempt))
    );
    let terminal = String::from_utf8(to_canonical_vec(&golden_terminal_request()).unwrap()).unwrap();
    let terminal_zero = terminal.replacen("\"attempt\":1", "\"attempt\":0", 1);
    assert_eq!(
        parse_canonical::<AppendTerminalRequestV1>(terminal_zero.as_bytes()),
        Err(ObservationStoreError::invalid(InvalidRequestCategory::ZeroAttempt))
    );
    flow("counterexample wire attempt=0 (started+terminal) -> invalid_request/zero_attempt");
}

#[test]
fn execution_observation_counterexample_wire_unknown_failure_category() {
    use super::super::error::InvalidRequestCategory;
    use super::super::hash::tests::{flow, golden_terminal_request};
    use super::super::model::AppendTerminalRequestV1;
    let text = String::from_utf8(to_canonical_vec(&golden_terminal_request()).unwrap()).unwrap();
    let unknown_category = text.replacen("\"category\":\"tool_failed\"", "\"category\":\"unknown_cat\"", 1);
    assert_eq!(
        parse_canonical::<AppendTerminalRequestV1>(unknown_category.as_bytes()),
        Err(ObservationStoreError::invalid(
            InvalidRequestCategory::InvalidFailureCategory
        ))
    );
    flow("counterexample wire unknown failure category -> invalid_request/invalid_failure_category");
}

#[test]
fn execution_observation_counterexample_modified_started_with_stale_hash() {
    use super::super::error::TransitionConflictCategory;
    use super::super::hash;
    use super::super::hash::tests::{flow, golden_started_request, hex64};
    use super::super::tests::attempt_view;
    use super::super::validation::validate_started_request;
    use super::super::validation::validate_started_transition;

    // The view binds the original Started; a caller that modified the body
    // must not pass as an idempotent retry, whatever digest it claims.
    let view = attempt_view(false);
    let mut modified = golden_started_request();
    modified.input_evidence_cids = vec![hex64('7')];
    validate_started_request(&modified).expect("modified request itself is valid");
    let modified_hash = hash::started_request_sha256(&modified).unwrap();
    assert_ne!(modified_hash, view.started_request_sha256);
    assert_eq!(
        validate_started_transition(&modified, Some(&view)),
        Err(ObservationStoreError::conflict(
            TransitionConflictCategory::StartedAlreadyBound
        ))
    );
    flow("counterexample modified-started + stale view hash -> started_already_bound (hash recomputed from body)");
}

#[test]
fn execution_observation_counterexample_modified_terminal_with_stale_hash() {
    use super::super::error::TransitionConflictCategory;
    use super::super::hash::tests::{flow, golden_started_request, golden_terminal_request};
    use super::super::ids::TerminalOutcomeV1;
    use super::super::tests::attempt_view;
    use super::super::validation::{validate_terminal_request, validate_terminal_transition};

    let view = attempt_view(true);
    let started = golden_started_request();
    let mut modified = golden_terminal_request();
    modified.outcome = TerminalOutcomeV1::Success;
    validate_terminal_request(&modified).expect("modified request itself is valid");
    assert_eq!(
        validate_terminal_transition(&modified, Some(&view), Some(&started)),
        Err(ObservationStoreError::conflict(
            TransitionConflictCategory::TerminalAlreadyBound
        ))
    );
    // rebind fields are checked before idempotency even on a bound view
    modified.policy_sha256 = "d".repeat(64);
    assert_eq!(
        validate_terminal_transition(&modified, Some(&view), Some(&started)),
        Err(ObservationStoreError::conflict(
            TransitionConflictCategory::TerminalPolicyRebind
        ))
    );
    flow("counterexample modified-terminal + stale view hash -> already_bound / policy_rebind before idempotency");
}
