//! Domain-separated identity hashes for observation objects (ADR-0007 §8).
//!
//! Every digest is `sha256(domain || RFC8785_JCS(value))` with lowercase hex
//! output. The seven domains are frozen; the active pointer has no identity
//! hash because its canonical bytes carry only the root hash. This module is an
//! independent implementation and never imports `memory/ledger/hash.rs`.

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::canonical::to_canonical_vec;
use super::error::ObservationStoreError;
use super::model::{
    AppendStartedRequestV1, AppendTerminalRequestV1, FixtureCurrentViewV1, FixtureEventSegmentV1, FixtureLedgerRootV1,
    StoredStartedEventV1, StoredTerminalEventV1,
};

const STARTED_REQUEST_DOMAIN: &[u8] = b"plico.execution-observation.fixture.started-request.v1\0";
const TERMINAL_REQUEST_DOMAIN: &[u8] = b"plico.execution-observation.fixture.terminal-request.v1\0";
const STARTED_EVENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.started-event.v1\0";
const TERMINAL_EVENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.terminal-event.v1\0";
const SEGMENT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.segment.v1\0";
const CURRENT_VIEW_DOMAIN: &[u8] = b"plico.execution-observation.fixture.current-view.v1\0";
const ROOT_DOMAIN: &[u8] = b"plico.execution-observation.fixture.root.v1\0";

pub(crate) fn started_request_sha256(value: &AppendStartedRequestV1) -> Result<String, ObservationStoreError> {
    domain_hash(STARTED_REQUEST_DOMAIN, value)
}

pub(crate) fn terminal_request_sha256(value: &AppendTerminalRequestV1) -> Result<String, ObservationStoreError> {
    domain_hash(TERMINAL_REQUEST_DOMAIN, value)
}

pub(crate) fn started_event_sha256(value: &StoredStartedEventV1) -> Result<String, ObservationStoreError> {
    domain_hash(STARTED_EVENT_DOMAIN, value)
}

pub(crate) fn terminal_event_sha256(value: &StoredTerminalEventV1) -> Result<String, ObservationStoreError> {
    domain_hash(TERMINAL_EVENT_DOMAIN, value)
}

pub(crate) fn segment_sha256(value: &FixtureEventSegmentV1) -> Result<String, ObservationStoreError> {
    domain_hash(SEGMENT_DOMAIN, value)
}

pub(crate) fn current_view_sha256(value: &FixtureCurrentViewV1) -> Result<String, ObservationStoreError> {
    domain_hash(CURRENT_VIEW_DOMAIN, value)
}

pub(crate) fn root_sha256(value: &FixtureLedgerRootV1) -> Result<String, ObservationStoreError> {
    domain_hash(ROOT_DOMAIN, value)
}

fn domain_hash<T: Serialize>(domain: &[u8], value: &T) -> Result<String, ObservationStoreError> {
    let canonical = to_canonical_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(&canonical);
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::num::NonZeroU32;

    use sha2::{Digest, Sha256};

    use super::super::canonical::{parse_canonical, to_canonical_vec};
    use super::super::ids::{CanonicalUuid, ExecutionAttemptKeyV1, FixtureOriginV1, TerminalOutcomeV1};
    use super::super::model::{
        FixtureActivePointerV1, ATTESTATION_STATE, CURRENT_VIEW_SCHEMA, POINTER_SCHEMA, ROOT_SCHEMA,
        STARTED_REQUEST_SCHEMA, TERMINAL_REQUEST_SCHEMA, TRUST_CLASS,
    };
    use super::super::tests::golden_chain;
    use super::*;

    pub(crate) const EXECUTION_ID: &str = "123e4567-e89b-42d3-a456-426614174000";
    pub(crate) const ORIGIN_REQUEST_ID: &str = "123e4567-e89b-42d3-a456-426614174001";
    pub(crate) const STARTED_RECORDED_AT_MS: u64 = 1_700_000_000_000;
    pub(crate) const TERMINAL_RECORDED_AT_MS: u64 = 1_700_000_000_042;

    pub(crate) const GENESIS_VIEW_SHA: &str = "f0b5d12cde6534fdccf88a1e8ff915feaa0f6c4302c3f99453fd713de1c3e92d";
    pub(crate) const GENESIS_ROOT_SHA: &str = "1f1106793cdd964ef5c6b41644638ddc0c12b296b80c57fca13c98fc657a398f";
    pub(crate) const STARTED_REQUEST_SHA: &str = "160804b6003538aba7cf858993b2f3efdf830493875a9c03e5277db0225975ac";
    pub(crate) const STARTED_EVENT_SHA: &str = "96438232ef0aab25ad5f54b3082bc0ed0fb0dcabdfa78a1c3567d51b2026cfc0";
    pub(crate) const STARTED_SEGMENT_SHA: &str = "aeab7ab3e137f5b9a2a20fd945e970976c68df6639c4031869569c545d03674d";
    pub(crate) const STARTED_VIEW_SHA: &str = "4204a72e2366a15efeb9e8135979fcb883cc7323773d61385b43c30445e5aba0";
    pub(crate) const STARTED_ROOT_SHA: &str = "6c3e5154ae5e26f8a3e230d54391f3639ad7adce8c6848fd9d077a121d8a4936";
    pub(crate) const TERMINAL_REQUEST_SHA: &str = "f8dd59a4bdaeabe52b27b79f0f4c749e344f7483ec66588ef6f9efe55f9d5bf2";
    pub(crate) const TERMINAL_EVENT_SHA: &str = "c178e5f3fc6c3570b655eccff18e337ccc579e09a3ad07b6586b1f4a5a27a858";
    pub(crate) const TERMINAL_SEGMENT_SHA: &str = "d0a1ed026079be1dc59258d9d10f1fbc9e3f6ef1dd390682814751d8a9bd584f";
    pub(crate) const TERMINAL_VIEW_SHA: &str = "d2bbabf5a9b3ce6121b48bc3c599b83be7e8c7d4f5330374a8837e2c51799722";
    pub(crate) const TERMINAL_ROOT_SHA: &str = "1a0a1c708d872579d387651509cf3383617f764faa44fc475f3a5798c1a85e8a";

    pub(crate) const GENESIS_ROOT_JCS: &str = "{\"committed_at_ms\":0,\"current_view_sha256\":\"f0b5d12cde6534fdccf88a1e8ff915feaa0f6c4302c3f99453fd713de1c3e92d\",\"event_segment_head_sha256\":null,\"event_watermark\":0,\"generation\":0,\"previous_root_sha256\":null,\"schema\":\"plico.execution-observation.fixture-root/v1\",\"trust_class\":\"unverified_fixture_only\"}";
    pub(crate) const STARTED_REQUEST_JCS: &str = "{\"attestation_state\":\"unverified_fixture\",\"context_evidence_cids\":[\"2222222222222222222222222222222222222222222222222222222222222222\"],\"fixture_origin\":{\"request_id\":\"123e4567-e89b-42d3-a456-426614174001\",\"type\":\"public_request\"},\"fixture_role_ref\":null,\"fixture_session_ref\":null,\"input_evidence_cids\":[\"0000000000000000000000000000000000000000000000000000000000000000\",\"1111111111111111111111111111111111111111111111111111111111111111\"],\"key\":{\"attempt\":1,\"execution_id\":\"123e4567-e89b-42d3-a456-426614174000\"},\"operation_contract_sha256\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"policy_sha256\":\"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\",\"runtime_sha256\":\"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\",\"schema\":\"plico.execution-observation.fixture-start-request/v1\"}";
    const POINTER_GENESIS_JCS: &str = "{\"root_sha256\":\"1f1106793cdd964ef5c6b41644638ddc0c12b296b80c57fca13c98fc657a398f\",\"schema\":\"plico.execution-observation.fixture-root-pointer/v1\"}";
    const POINTER_STARTED_JCS: &str = "{\"root_sha256\":\"6c3e5154ae5e26f8a3e230d54391f3639ad7adce8c6848fd9d077a121d8a4936\",\"schema\":\"plico.execution-observation.fixture-root-pointer/v1\"}";
    const POINTER_TERMINAL_JCS: &str = "{\"root_sha256\":\"1a0a1c708d872579d387651509cf3383617f764faa44fc475f3a5798c1a85e8a\",\"schema\":\"plico.execution-observation.fixture-root-pointer/v1\"}";

    pub(crate) fn uuid(value: &str) -> CanonicalUuid {
        CanonicalUuid::from_canonical_str(value).expect("golden uuid is canonical")
    }

    pub(crate) fn hex64(fill: char) -> String {
        fill.to_string().repeat(64)
    }

    pub(crate) fn golden_key() -> ExecutionAttemptKeyV1 {
        ExecutionAttemptKeyV1 {
            execution_id: uuid(EXECUTION_ID),
            attempt: NonZeroU32::MIN,
        }
    }

    pub(crate) fn golden_started_request() -> AppendStartedRequestV1 {
        AppendStartedRequestV1 {
            schema: STARTED_REQUEST_SCHEMA.into(),
            key: golden_key(),
            fixture_origin: FixtureOriginV1::PublicRequest {
                request_id: uuid(ORIGIN_REQUEST_ID),
            },
            attestation_state: ATTESTATION_STATE.into(),
            fixture_role_ref: None,
            fixture_session_ref: None,
            operation_contract_sha256: hex64('a'),
            input_evidence_cids: vec![hex64('0'), hex64('1')],
            context_evidence_cids: vec![hex64('2')],
            policy_sha256: hex64('b'),
            runtime_sha256: hex64('c'),
        }
    }

    pub(crate) fn golden_terminal_request() -> AppendTerminalRequestV1 {
        AppendTerminalRequestV1 {
            schema: TERMINAL_REQUEST_SCHEMA.into(),
            key: golden_key(),
            attestation_state: ATTESTATION_STATE.into(),
            outcome: TerminalOutcomeV1::Failure {
                category: super::super::ids::FailureCategoryV1::ToolFailed,
            },
            output_evidence_cids: vec![hex64('3')],
            execution_elapsed_ms: None,
            policy_sha256: hex64('b'),
            runtime_sha256: hex64('c'),
        }
    }

    fn genesis() -> (FixtureCurrentViewV1, FixtureLedgerRootV1) {
        let view = FixtureCurrentViewV1 {
            schema: CURRENT_VIEW_SCHEMA.into(),
            attestation_state: ATTESTATION_STATE.into(),
            generation: 0,
            event_watermark: 0,
            attempts: Vec::new(),
        };
        let root = FixtureLedgerRootV1 {
            schema: ROOT_SCHEMA.into(),
            trust_class: TRUST_CLASS.into(),
            generation: 0,
            previous_root_sha256: None,
            event_segment_head_sha256: None,
            event_watermark: 0,
            current_view_sha256: GENESIS_VIEW_SHA.into(),
            committed_at_ms: 0,
        };
        (view, root)
    }

    #[test]
    fn execution_observation_golden_genesis_and_request_vectors() {
        let (view, root) = genesis();
        assert_eq!(current_view_sha256(&view).expect("hash"), GENESIS_VIEW_SHA);
        assert_eq!(root_sha256(&root).expect("hash"), GENESIS_ROOT_SHA);
        assert_eq!(to_canonical_vec(&root).expect("canonical"), GENESIS_ROOT_JCS.as_bytes());
        view.validate().expect("genesis view");
        root.validate(GENESIS_VIEW_SHA).expect("genesis root");

        let started = golden_started_request();
        assert_eq!(started_request_sha256(&started).expect("hash"), STARTED_REQUEST_SHA);
        assert_eq!(
            to_canonical_vec(&started).expect("canonical"),
            STARTED_REQUEST_JCS.as_bytes()
        );
        let terminal = golden_terminal_request();
        assert_eq!(terminal_request_sha256(&terminal).expect("hash"), TERMINAL_REQUEST_SHA);
    }

    #[test]
    fn execution_observation_golden_full_chain_and_pointers() {
        let chain = golden_chain();
        assert_eq!(started_event_sha256(&chain.started_event).unwrap(), STARTED_EVENT_SHA);
        assert_eq!(segment_sha256(&chain.started_segment).unwrap(), STARTED_SEGMENT_SHA);
        assert_eq!(current_view_sha256(&chain.open_view).unwrap(), STARTED_VIEW_SHA);
        assert_eq!(root_sha256(&chain.started_root).unwrap(), STARTED_ROOT_SHA);
        assert_eq!(
            terminal_event_sha256(&chain.terminal_event).unwrap(),
            TERMINAL_EVENT_SHA
        );
        assert_eq!(segment_sha256(&chain.terminal_segment).unwrap(), TERMINAL_SEGMENT_SHA);
        assert_eq!(current_view_sha256(&chain.terminal_view).unwrap(), TERMINAL_VIEW_SHA);
        assert_eq!(root_sha256(&chain.terminal_root).unwrap(), TERMINAL_ROOT_SHA);

        chain.started_event.validate().unwrap();
        chain.started_segment.validate(STARTED_EVENT_SHA).unwrap();
        chain.open_view.validate().unwrap();
        chain.started_root.validate(STARTED_VIEW_SHA).unwrap();
        chain.terminal_event.validate().unwrap();
        chain.terminal_segment.validate(TERMINAL_EVENT_SHA).unwrap();
        chain.terminal_view.validate().unwrap();
        chain.terminal_root.validate(TERMINAL_VIEW_SHA).unwrap();
        assert_eq!(
            chain.terminal_segment.previous_segment_sha256.as_deref(),
            Some(STARTED_SEGMENT_SHA)
        );
        assert_eq!(
            chain.terminal_root.previous_root_sha256.as_deref(),
            Some(STARTED_ROOT_SHA)
        );

        for (root_sha, golden) in [
            (GENESIS_ROOT_SHA, POINTER_GENESIS_JCS),
            (STARTED_ROOT_SHA, POINTER_STARTED_JCS),
            (TERMINAL_ROOT_SHA, POINTER_TERMINAL_JCS),
        ] {
            let pointer = FixtureActivePointerV1 {
                schema: POINTER_SCHEMA.into(),
                root_sha256: root_sha.into(),
            };
            pointer.validate(root_sha).expect("pointer");
            assert_eq!(to_canonical_vec(&pointer).unwrap(), golden.as_bytes());
            assert_eq!(
                parse_canonical::<FixtureActivePointerV1>(golden.as_bytes()).unwrap(),
                pointer
            );
        }
    }

    #[test]
    fn execution_observation_hash_domain_isolation() {
        let started = golden_started_request();
        let canonical = to_canonical_vec(&started).unwrap();
        let started_digest = started_request_sha256(&started).unwrap();
        assert_eq!(started_digest, STARTED_REQUEST_SHA);

        let mut hasher = Sha256::new();
        hasher.update(b"plico.execution-observation.fixture.terminal-request.v1\0");
        hasher.update(&canonical);
        let cross_domain = format!("{:x}", hasher.finalize());
        assert_ne!(cross_domain, started_digest);
        assert_ne!(cross_domain, TERMINAL_REQUEST_SHA);

        let (view, root) = genesis();
        assert_ne!(current_view_sha256(&view).unwrap(), root_sha256(&root).unwrap());
    }
}
