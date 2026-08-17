//! RFC 8785/JCS canonicalization boundary (ADR-0007 §8): bytes load only from
//! their exact JCS encoding; non-canonical bytes, unknown fields, and misplaced
//! nulls are typed rejects, never panics.

use serde::{de::DeserializeOwned, Serialize};

use super::error::{InvalidRequestCategory, ObservationStoreError};

pub(crate) fn to_canonical_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, ObservationStoreError> {
    serde_json_canonicalizer::to_vec(value).map_err(|_| ObservationStoreError::InvalidRequest {
        category: InvalidRequestCategory::JcsCanonicalizationFailed,
    })
}

pub(crate) fn parse_canonical<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T, ObservationStoreError> {
    // Step 1: prove the bytes are exact JCS BEFORE any typed semantic parse,
    // so non-canonical input (whitespace, key order, escapes, duplicate keys)
    // reports jcs first and never surfaces a semantic category. The Value is a
    // transient probe only — never stored in or returned from a model type.
    let probe: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| jcs_error())?;
    if to_canonical_vec(&probe)? != bytes {
        return Err(jcs_error());
    }
    // Step 2: typed semantic deserialize on proven-canonical bytes.
    let value: T = serde_json::from_slice(bytes).map_err(|error| classify_wire_error(&error))?;
    // Step 3: typed encoding must equal the input — this rejects a missing
    // nullable field (serde fills None; the typed encoding re-emits the null).
    if to_canonical_vec(&value)? == bytes {
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
/// Wire classification (§4/§11): hand-rolled deserializers report the frozen
/// error Display as the serde message; matching is EXACT against
/// `<display> at line L column C` so echoed input cannot forge a category.
fn classify_wire_error(error: &serde_json::Error) -> ObservationStoreError {
    let expected_tail = format!(" at line {} column {}", error.line(), error.column());
    for category in [
        InvalidRequestCategory::ZeroAttempt,
        InvalidRequestCategory::InvalidFailureCategory,
    ] {
        let marker = ObservationStoreError::invalid(category).to_string();
        if error.to_string() == format!("{marker}{expected_tail}") {
            return ObservationStoreError::invalid(category);
        }
    }
    jcs_error()
}

impl<'de> serde::Deserialize<'de> for super::ids::ExecutionAttemptKeyV1 {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            execution_id: super::ids::CanonicalUuid,
            attempt: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        super::ids::ExecutionAttemptKeyV1::from_parts(wire.execution_id, wire.attempt).map_err(serde::de::Error::custom)
    }
}

/// Canonical size cap: oversize JCS bytes are typed rejects (§5).
pub(crate) fn check_object_bytes<T: Serialize>(value: &T, maximum_bytes: usize) -> Result<(), ObservationStoreError> {
    if to_canonical_vec(value)?.len() > maximum_bytes {
        Err(ObservationStoreError::limit(super::error::LimitCategory::ObjectBytes))
    } else {
        Ok(())
    }
}

// serde's internally-tagged enums do not enforce `deny_unknown_fields`, so
// they get hand-rolled strict deserializers: a flat deny-unknown wire struct
// plus field-combination checks; `Option<Option<T>>` splits absent vs null.

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
mod tests;
