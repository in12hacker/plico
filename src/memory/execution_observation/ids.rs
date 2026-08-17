//! Typed identifiers and closed wire enums for execution observations (ADR-0007 §4).
//!
//! UUID wire values are exactly the 36-byte lowercase hyphenated RFC 4122 text
//! form. Uppercase, unhyphenated, braced, URN, and malformed values are rejected
//! at the deserialization boundary; the nil UUID parses here and is rejected by
//! validation with the typed `nil_uuid` category. Terminal outcomes are
//! internally-tagged objects with the closed failure-category string set.

use std::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::error::{InvalidRequestCategory, ObservationStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CanonicalUuid(uuid::Uuid);

impl CanonicalUuid {
    /// Accepts only the exact canonical wire form (lowercase, hyphenated, non-empty form).
    pub(crate) fn from_canonical_str(value: &str) -> Option<Self> {
        parse_canonical_form(value).map(Self)
    }

    /// Nil check is a validation concern: it produces the typed `nil_uuid` category.
    pub(crate) fn is_nil(self) -> bool {
        self.0.is_nil()
    }

    /// Raw big-endian bytes, used for deterministic current-view ordering.
    pub(crate) fn as_bytes(self) -> [u8; 16] {
        *self.0.as_bytes()
    }
}

fn parse_canonical_form(value: &str) -> Option<uuid::Uuid> {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return None;
    }
    for (index, byte) in bytes.iter().enumerate() {
        match index {
            8 | 13 | 18 | 23 => {
                if *byte != b'-' {
                    return None;
                }
            }
            _ => {
                let lowercase_hex = byte.is_ascii_digit() || matches!(byte, b'a'..=b'f');
                if !lowercase_hex {
                    return None;
                }
            }
        }
    }
    uuid::Uuid::parse_str(value).ok()
}

impl Serialize for CanonicalUuid {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.hyphenated().to_string())
    }
}

impl<'de> Deserialize<'de> for CanonicalUuid {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CanonicalUuidVisitor;

        impl serde::de::Visitor<'_> for CanonicalUuidVisitor {
            type Value = CanonicalUuid;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a 36-byte lowercase hyphenated RFC 4122 UUID string")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                CanonicalUuid::from_canonical_str(value).ok_or_else(|| E::custom("non-canonical uuid wire form"))
            }
        }

        deserializer.deserialize_str(CanonicalUuidVisitor)
    }
}

// Deserialize is hand-rolled in `canonical.rs` so attempt=0 is a typed reject
// (the deny-unknown-fields wire form lives on that hand-rolled path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub(crate) struct ExecutionAttemptKeyV1 {
    pub(crate) execution_id: CanonicalUuid,
    pub(crate) attempt: NonZeroU32,
}

impl ExecutionAttemptKeyV1 {
    /// Typed constructor: `attempt` must be in `1..=u32::MAX` (ADR-0007 §5).
    pub(crate) fn from_parts(execution_id: CanonicalUuid, attempt: u32) -> Result<Self, ObservationStoreError> {
        let attempt = NonZeroU32::new(attempt).ok_or(ObservationStoreError::InvalidRequest {
            category: InvalidRequestCategory::ZeroAttempt,
        })?;
        Ok(Self { execution_id, attempt })
    }
}

// Deserialize is implemented by hand in `canonical.rs` (strict internally-tagged
// wire form); Serialize stays derived with the `type` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum FixtureOriginV1 {
    PublicRequest { request_id: CanonicalUuid },
    IntentDispatch { intent_id: CanonicalUuid },
    InternalTask { task_id: CanonicalUuid },
}

impl FixtureOriginV1 {
    /// The embedded origin identifier, validated non-nil by request validation.
    pub(crate) fn id(&self) -> &CanonicalUuid {
        match self {
            Self::PublicRequest { request_id } => request_id,
            Self::IntentDispatch { intent_id } => intent_id,
            Self::InternalTask { task_id } => task_id,
        }
    }
}

// Deserialize is implemented by hand in `canonical.rs` (strict internally-tagged
// wire form); Serialize stays derived with the `type` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TerminalOutcomeV1 {
    Success,
    Failure { category: FailureCategoryV1 },
    Timeout,
    Cancelled,
    Indeterminate,
}

// Deserialize is hand-rolled in `error.rs` (typed unknown-category marker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureCategoryV1 {
    InvalidInput,
    PolicyDenied,
    DependencyUnavailable,
    ExecutorRejected,
    ExecutorFailed,
    ExecutorPanicked,
    ToolFailed,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventKind {
    Started,
    Terminal,
}

#[cfg(test)]
mod tests {
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
        super::super::hash::tests::flow(
            "data.ids uuid wire=36-byte lowercase hyphenated -> serialize/parse roundtrip ok",
        );
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
}
