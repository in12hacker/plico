//! WP1 self-tests: F10/F13 rejects, transitions, key counterexamples (field
//! rejects live in mod.rs; stale-hash in canonical.rs; golden in hash.rs).

use super::canonical::{parse_canonical, to_canonical_vec};
use super::error::InvalidRequestCategory::{DuplicateCid, InvalidCid, JcsCanonicalizationFailed};
use super::error::{ObservationStoreError, TransitionConflictCategory};
use super::hash::tests::{flow, golden_started_request, golden_terminal_request, hex64, uuid, ORIGIN_REQUEST_ID};
use super::ids::{ExecutionAttemptKeyV1, FixtureOriginV1, TerminalOutcomeV1};
use super::model::*;
use super::validation::*;

mod fixtures;

pub(crate) use fixtures::{attempt_view, conflict, err, golden_chain};

pub(crate) struct GoldenChain {
    pub(crate) started_event: StoredStartedEventV1,
    pub(crate) started_segment: FixtureEventSegmentV1,
    pub(crate) open_view: FixtureCurrentViewV1,
    pub(crate) started_root: FixtureLedgerRootV1,
    pub(crate) terminal_event: StoredTerminalEventV1,
    pub(crate) terminal_segment: FixtureEventSegmentV1,
    pub(crate) terminal_view: FixtureCurrentViewV1,
    pub(crate) terminal_root: FixtureLedgerRootV1,
}

#[test]
fn execution_observation_f10_malformed_and_inline_cid_rejected() {
    let mut request = golden_started_request();
    validate_started_request(&request).expect("golden request is valid");
    let malformed = [
        String::new(),
        "abc".to_string(),
        "0".repeat(63),
        "A".repeat(64),
        "g".repeat(64),
        "inline bytes are not cid references".to_string(),
        format!("{}\n", "0".repeat(63)),
    ];
    for (index, cid) in malformed.into_iter().enumerate() {
        request.input_evidence_cids = vec![cid];
        assert_eq!(validate_started_request(&request), Err(err(InvalidCid)));
        flow(format!("logic.f10 cid-case={index} -> invalid_request/invalid_cid"));
    }
    request = golden_started_request();
    request.input_evidence_cids = vec![hex64('0'), hex64('0')];
    assert_eq!(validate_started_request(&request), Err(err(DuplicateCid)));
    flow("logic.f10 duplicate-within-list -> invalid_request/duplicate_cid");
    request.input_evidence_cids = vec![hex64('0')];
    request.context_evidence_cids = vec![hex64('0')];
    validate_started_request(&request).expect("same cid across lists is allowed");
    flow("logic.f10 same-cid-across-lists -> ok");
}
#[test]
fn execution_observation_f13_wire_level_strict_rejects() {
    let canonical = to_canonical_vec(&golden_started_request()).unwrap();
    let declaration_order = serde_json::to_vec(&golden_started_request()).unwrap();
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(&declaration_order),
        Err(err(JcsCanonicalizationFailed))
    );
    let text = std::str::from_utf8(&canonical).expect("ascii");

    let unknown_field = format!("{{\"zz\":0,{}", &text[1..]);
    assert!(parse_canonical::<AppendStartedRequestV1>(unknown_field.as_bytes()).is_err());
    // serde echoes unknown field names in its error Display; a marker-shaped
    // field name must stay jcs, never forge a typed category (red-team P1).
    let hijacked = format!("{{\"invalid_request/zero_attempt\":0,{}", &text[1..]);
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(hijacked.as_bytes()),
        Err(err(JcsCanonicalizationFailed))
    );
    let whitespace = format!(" {text}");
    assert!(parse_canonical::<AppendStartedRequestV1>(whitespace.as_bytes()).is_err());
    // combined attacks (parser-order P1): non-canonical bytes report jcs first
    // even when the payload also carries a semantic violation.
    let zero_ws = format!(" {}", text.replacen("\"attempt\":1", "\"attempt\":0", 1));
    assert_eq!(
        parse_canonical::<AppendStartedRequestV1>(zero_ws.as_bytes()),
        Err(err(JcsCanonicalizationFailed))
    );
    let terminal_text = String::from_utf8(to_canonical_vec(&golden_terminal_request()).unwrap()).unwrap();
    let unknown = terminal_text.replacen("\"category\":\"tool_failed\"", "\"category\":\"unknown_cat\"", 1);
    // true swap of the first two keys (a duplicated-key shape would fail the
    // typed parse on BOTH old and new parsers and discriminate nothing)
    let head = "{\"attestation_state\":\"unverified_fixture\",\"execution_elapsed_ms\":null,";
    assert!(unknown.starts_with(head));
    let tail = &unknown[head.len()..];
    let reordered = format!("{{\"execution_elapsed_ms\":null,\"attestation_state\":\"unverified_fixture\",{tail}");
    assert_eq!(
        parse_canonical::<AppendTerminalRequestV1>(reordered.as_bytes()),
        Err(err(JcsCanonicalizationFailed))
    );
    let escaped_unicode = text.replacen("\"policy_sha256\":\"bbbb", "\"policy_sha256\":\"\\u0062bbb", 1);
    assert!(parse_canonical::<AppendStartedRequestV1>(escaped_unicode.as_bytes()).is_err());
    flow("logic.f13 wire rejects order|unknown-field|marker-hijack|whitespace|ws+zero|reorder+unknown|escape -> jcs");
}
#[test]
fn execution_observation_transition_state_machine() {
    let chain = golden_chain();
    let open_view = &chain.open_view.attempts[0];
    let terminal_view = &chain.terminal_view.attempts[0];
    let started = golden_started_request();
    let terminal = golden_terminal_request();

    validate_started_transition(&started, None).expect("absent accepts started");
    validate_started_transition(&started, Some(open_view)).expect("same started is idempotent");
    validate_started_transition(&started, Some(terminal_view)).expect("same started is idempotent");
    flow("logic.transition absent+started -> ok; open+same-started -> ok-idempotent; terminal+same -> ok-idempotent");

    let mut rebound = started.clone();
    rebound.input_evidence_cids = vec![hex64('7')];
    assert_eq!(
        validate_started_transition(&rebound, Some(open_view)),
        Err(conflict(TransitionConflictCategory::StartedAlreadyBound))
    );
    let mut origin_rebound = started.clone();
    let origin_id = uuid(ORIGIN_REQUEST_ID);
    origin_rebound.fixture_origin = FixtureOriginV1::IntentDispatch { intent_id: origin_id };
    assert_eq!(
        validate_started_transition(&origin_rebound, Some(terminal_view)),
        Err(conflict(TransitionConflictCategory::StartedAlreadyBound))
    );
    flow("logic.transition open/terminal + different-started (evidence|origin rebind) -> started_already_bound");
    assert_eq!(
        validate_terminal_transition(&terminal, None, None),
        Err(conflict(TransitionConflictCategory::TerminalWithoutStarted))
    );
    validate_terminal_transition(&terminal, Some(open_view), Some(&started))
        .expect("open accepts first terminal with matching policy/runtime");
    validate_terminal_transition(&terminal, Some(terminal_view), Some(&started)).expect("same terminal is idempotent");
    flow("logic.transition absent+terminal -> terminal_without_started; open+first-terminal -> ok; terminal+same -> ok-idempotent");
    let mut policy_rebind = terminal.clone();
    policy_rebind.policy_sha256 = hex64('d');
    assert_eq!(
        validate_terminal_transition(&policy_rebind, Some(open_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalPolicyRebind))
    );
    let mut runtime_rebind = terminal.clone();
    runtime_rebind.runtime_sha256 = hex64('e');
    assert_eq!(
        validate_terminal_transition(&runtime_rebind, Some(open_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalRuntimeRebind))
    );
    let mut second_terminal = terminal.clone();
    second_terminal.outcome = TerminalOutcomeV1::Success;
    assert_eq!(
        validate_terminal_transition(&second_terminal, Some(terminal_view), Some(&started)),
        Err(conflict(TransitionConflictCategory::TerminalAlreadyBound))
    );
    flow("logic.transition open+policy/runtime-mismatch -> rebind conflicts; terminal+different-terminal -> terminal_already_bound");
}

#[test]
fn execution_observation_counterexample_terminal_cross_attempt_key() {
    use std::num::NonZeroU32;

    use super::error::CorruptionCategory;

    let view = attempt_view(false);
    let started = golden_started_request();
    let mut terminal = golden_terminal_request();
    terminal.key = ExecutionAttemptKeyV1 {
        execution_id: uuid("123e4567-e89b-42d3-a456-426614174099"),
        attempt: NonZeroU32::new(2).expect("nonzero"),
    };
    validate_terminal_request(&terminal).expect("request itself is valid");
    assert_eq!(
        validate_terminal_transition(&terminal, Some(&view), Some(&started)),
        Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
    );
    flow("counterexample terminal cross-attempt key -> corrupt_store/invalid_transition");
}

#[test]
fn execution_observation_counterexample_started_retry_unrelated_view() {
    use std::num::NonZeroU32;

    use super::error::CorruptionCategory;

    let view = attempt_view(false);
    let mut started = golden_started_request();
    started.key = ExecutionAttemptKeyV1 {
        execution_id: uuid("123e4567-e89b-42d3-a456-426614174099"),
        attempt: NonZeroU32::new(7).expect("nonzero"),
    };
    validate_started_request(&started).expect("request itself is valid");
    assert_eq!(
        validate_started_transition(&started, Some(&view)),
        Err(ObservationStoreError::corrupt(CorruptionCategory::InvalidTransition))
    );
    flow("counterexample started retry vs unrelated view -> corrupt_store/invalid_transition");
}
