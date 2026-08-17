use super::error::InvalidRequestCategory::{
    DuplicateCid, InvalidAttestation, InvalidCid, InvalidDigest, UnsafeInteger, UnsupportedSchema,
};
use super::error::ObservationStoreError;
use super::hash::tests::{golden_started_request, golden_terminal_request, hex64};
use super::model::{STARTED_EVENT_SCHEMA, STARTED_REQUEST_SCHEMA};
use super::tests::{err, golden_chain};
use super::validation::{
    validate_monotonic_record, validate_started_request, validate_terminal_request, JSON_SAFE_INTEGER_MAX,
};

#[test]
fn execution_observation_f13_field_level_typed_rejects() {
    let mut request = golden_started_request();
    validate_started_request(&request).expect("golden request is valid");
    request.input_evidence_cids = vec![hex64('0'), hex64('0')];
    assert_eq!(validate_started_request(&request), Err(err(DuplicateCid)));
    request.input_evidence_cids = vec!["A".repeat(64)];
    assert_eq!(validate_started_request(&request), Err(err(InvalidCid)));

    request = golden_started_request();
    request.schema = format!("{STARTED_REQUEST_SCHEMA}-v2");
    assert_eq!(validate_started_request(&request), Err(err(UnsupportedSchema)));
    request = golden_started_request();
    request.operation_contract_sha256 = format!("{}\u{1}", "a".repeat(63));
    assert_eq!(validate_started_request(&request), Err(err(InvalidDigest)));
    request.attestation_state = "trusted".to_string();
    assert_eq!(validate_started_request(&request), Err(err(InvalidAttestation)));
    let mut terminal = golden_terminal_request();
    terminal.execution_elapsed_ms = Some(JSON_SAFE_INTEGER_MAX);
    validate_terminal_request(&terminal).expect("2^53-1 is json-safe");
    terminal.execution_elapsed_ms = Some(JSON_SAFE_INTEGER_MAX + 1);
    assert_eq!(validate_terminal_request(&terminal), Err(err(UnsafeInteger)));

    let mut event = golden_chain().started_event;
    event.schema = format!("{STARTED_EVENT_SCHEMA}-v2");
    assert_eq!(
        event.validate(),
        Err(ObservationStoreError::corrupt(
            super::error::CorruptionCategory::UnsupportedStoredSchema
        ))
    );
    assert_eq!(validate_monotonic_record(100, 99), Err(err(UnsafeInteger)));
    validate_monotonic_record(99, 100).unwrap();
    validate_monotonic_record(100, 100).unwrap();
}
