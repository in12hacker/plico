use super::super::error::CorruptionCategory;
use super::super::hash::tests::{flow, golden_started_request, golden_terminal_request};
use super::*;

fn unique_cids(from: u64, count: usize) -> Vec<String> {
    (from..from + count as u64)
        .map(|value| format!("{value:064x}"))
        .collect()
}

#[test]
fn execution_observation_counterexample_evidence_total_overflow_256_256_1() {
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
    assert_eq!(event.validate(), Err(corrupt(CorruptionCategory::SequenceGap)));
    flow("logic.boundaries sequence=0 -> corrupt_store/sequence_gap");
    event.sequence = JSON_SAFE_INTEGER_MAX + 1;
    assert_eq!(event.validate(), Err(invalid(InvalidRequestCategory::UnsafeInteger)));
    flow("logic.boundaries sequence=2^53 -> invalid_request/unsafe_integer");
}
