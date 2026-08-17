//! RFC 8785/JCS canonicalization boundary (ADR-0007 §8).
//!
//! All observation bytes are produced by `serde_json_canonicalizer` and parsed
//! strictly: a value deserializes only if its raw bytes are already the exact
//! JCS encoding of the value. Non-canonical key order, whitespace, unknown
//! fields, missing fields, and misplaced nulls are rejected with the typed
//! `jcs_canonicalization_failed` category instead of panicking.

use serde::de::DeserializeOwned;
use serde::Serialize;

use super::error::{InvalidRequestCategory, ObservationStoreError};

pub(crate) fn to_canonical_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, ObservationStoreError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| ObservationStoreError::InvalidRequest {
        category: InvalidRequestCategory::JcsCanonicalizationFailed,
    })
}

pub(crate) fn parse_canonical<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, ObservationStoreError> {
    let value: T = serde_json::from_slice(bytes).map_err(|_| jcs_error())?;
    let canonical = to_canonical_vec(&value)?;
    if canonical.as_slice() == bytes {
        Ok(value)
    } else {
        Err(jcs_error())
    }
}

fn jcs_error() -> ObservationStoreError {
    ObservationStoreError::InvalidRequest {
        category: InvalidRequestCategory::JcsCanonicalizationFailed,
    }
}

/// Canonical size cap: oversize objects are rejected by measuring the JCS
/// encoding itself (ADR-0007 §5 — must fail before any immutable write).
pub(crate) fn check_object_bytes<T: Serialize>(value: &T, maximum_bytes: usize) -> Result<(), ObservationStoreError> {
    if to_canonical_vec(value)?.len() > maximum_bytes {
        Err(ObservationStoreError::limit(super::error::LimitCategory::ObjectBytes))
    } else {
        Ok(())
    }
}

// serde's internally-tagged enums do not enforce `deny_unknown_fields`, so the
// two tagged enums get hand-rolled strict deserializers here: a flat
// `deny_unknown_fields` wire struct plus explicit field-combination checks.
// `Option<Option<T>>` distinguishes absent (None) from present-null
// (Some(None)); both are invalid wherever the variant does not declare the
// field, and present-null is invalid even where it is declared.

impl<'de> serde::Deserialize<'de> for super::ids::TerminalOutcomeV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(rename = "type")]
            tag: String,
            #[serde(default)]
            category: Option<Option<super::ids::FailureCategoryV1>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        match wire.tag.as_str() {
            "success" if wire.category.is_none() => Ok(Self::Success),
            "timeout" if wire.category.is_none() => Ok(Self::Timeout),
            "cancelled" if wire.category.is_none() => Ok(Self::Cancelled),
            "indeterminate" if wire.category.is_none() => Ok(Self::Indeterminate),
            "failure" => match wire.category {
                Some(Some(category)) => Ok(Self::Failure { category }),
                _ => Err(serde::de::Error::custom("failure requires a present category")),
            },
            _ => Err(serde::de::Error::custom("unknown terminal outcome type")),
        }
    }
}

impl<'de> serde::Deserialize<'de> for super::ids::FixtureOriginV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            #[serde(rename = "type")]
            tag: String,
            #[serde(default)]
            request_id: Option<Option<super::ids::CanonicalUuid>>,
            #[serde(default)]
            intent_id: Option<Option<super::ids::CanonicalUuid>>,
            #[serde(default)]
            task_id: Option<Option<super::ids::CanonicalUuid>>,
        }
        let wire = Wire::deserialize(deserializer)?;
        match wire.tag.as_str() {
            "public_request" => match (wire.request_id, wire.intent_id, wire.task_id) {
                (Some(Some(request_id)), None, None) => Ok(Self::PublicRequest { request_id }),
                _ => Err(serde::de::Error::custom("invalid public_request fields")),
            },
            "intent_dispatch" => match (wire.request_id, wire.intent_id, wire.task_id) {
                (None, Some(Some(intent_id)), None) => Ok(Self::IntentDispatch { intent_id }),
                _ => Err(serde::de::Error::custom("invalid intent_dispatch fields")),
            },
            "internal_task" => match (wire.request_id, wire.intent_id, wire.task_id) {
                (None, None, Some(Some(task_id))) => Ok(Self::InternalTask { task_id }),
                _ => Err(serde::de::Error::custom("invalid internal_task fields")),
            },
            _ => Err(serde::de::Error::custom("unknown fixture origin type")),
        }
    }
}

#[cfg(test)]
mod tests {
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
        super::super::hash::tests::flow(format!(
            "data.canonical serialize struct -> {} bytes JCS; parse-back byte-equal -> ok",
            bytes.len()
        ));
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
            let error = parse_canonical::<Sample>(case).unwrap_err();
            assert_eq!(
                error,
                ObservationStoreError::InvalidRequest {
                    category: InvalidRequestCategory::JcsCanonicalizationFailed
                }
            );
            super::super::hash::tests::flow(format!(
                "data.canonical case={index} bytes={} -> invalid_request/jcs_canonicalization_failed",
                case.len()
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
            "data.canonical started-request bytes={} explicit-nulls -> parse ok, None preserved",
            started_text.len()
        ));

        let missing_role = started_text.replacen("\"fixture_role_ref\":null,", "", 1);
        assert!(parse_canonical::<AppendStartedRequestV1>(missing_role.as_bytes()).is_err());
        flow("data.canonical nullable fixture_role_ref missing -> reject (missing != null)");
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
    fn execution_observation_f13_field_level_typed_rejects() {
        use super::super::error::InvalidRequestCategory::{
            DuplicateCid, InvalidAttestation, InvalidCid, InvalidDigest, UnsafeInteger, UnsupportedSchema,
        };
        use super::super::hash::tests::{golden_started_request, golden_terminal_request, hex64};
        use super::super::model::{STARTED_EVENT_SCHEMA, STARTED_REQUEST_SCHEMA};
        use super::super::tests::{err, golden_chain};
        use super::super::validation::{
            validate_monotonic_record, validate_started_request, validate_terminal_request, JSON_SAFE_INTEGER_MAX,
        };

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
                super::super::error::CorruptionCategory::UnsupportedStoredSchema
            ))
        );

        assert_eq!(validate_monotonic_record(100, 99), Err(err(UnsafeInteger)));
        validate_monotonic_record(99, 100).unwrap();
        validate_monotonic_record(100, 100).unwrap();
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
        use super::super::validation::validate_terminal_request;
        use super::super::validation::validate_terminal_transition;

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
}
