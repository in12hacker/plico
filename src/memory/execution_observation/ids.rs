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
mod tests;
