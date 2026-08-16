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
    }

    #[test]
    fn execution_observation_canonical_rejects_noncanonical_and_unknown() {
        let cases: [&[u8]; 4] = [
            br#"{"b":1,"a":"x"}"#,                // key order not JCS-sorted
            b"{\n  \"a\": \"x\",\n  \"b\": 1\n}", // whitespace
            br#"{"a":"x","b":1,"c":0}"#,          // unknown field
            br#"{"a":"x"}"#,                      // missing field
        ];
        for case in cases {
            let error = parse_canonical::<Sample>(case).unwrap_err();
            assert_eq!(
                error,
                ObservationStoreError::InvalidRequest {
                    category: InvalidRequestCategory::JcsCanonicalizationFailed
                }
            );
        }
    }

    #[test]
    fn execution_observation_canonical_missing_versus_null_nullable_fields() {
        use super::super::hash::tests::{golden_started_request, golden_terminal_request, hex64};
        use super::super::model::{AppendStartedRequestV1, AppendTerminalRequestV1};

        let started_text = String::from_utf8(to_canonical_vec(&golden_started_request()).unwrap()).unwrap();
        assert!(started_text.contains("\"fixture_role_ref\":null,\"fixture_session_ref\":null"));
        let parsed: AppendStartedRequestV1 = parse_canonical(started_text.as_bytes()).expect("explicit nulls parse");
        assert!(parsed.fixture_role_ref.is_none() && parsed.fixture_session_ref.is_none());

        let missing_role = started_text.replacen("\"fixture_role_ref\":null,", "", 1);
        assert!(parse_canonical::<AppendStartedRequestV1>(missing_role.as_bytes()).is_err());
        let null_policy = started_text.replacen(
            &format!("\"policy_sha256\":\"{}\"", hex64('b')),
            "\"policy_sha256\":null",
            1,
        );
        assert!(parse_canonical::<AppendStartedRequestV1>(null_policy.as_bytes()).is_err());

        let terminal_text = String::from_utf8(to_canonical_vec(&golden_terminal_request()).unwrap()).unwrap();
        assert!(terminal_text.contains("\"execution_elapsed_ms\":null"));
        let missing_elapsed = terminal_text.replacen("\"execution_elapsed_ms\":null,", "", 1);
        assert!(parse_canonical::<AppendTerminalRequestV1>(missing_elapsed.as_bytes()).is_err());
    }
}
