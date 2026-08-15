use base64::Engine;
use serde::{Deserialize, Serialize};

use super::{
    validate_cid, validate_limit, validate_non_empty_bounded, validate_tags, validate_uuid, DEFAULT_LIMIT,
    MAX_AUTH_BYTES, MAX_OBJECT_BYTES, MAX_QUERY_BYTES, MAX_TEXT_BYTES, PERSONAL_PROTOCOL,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub message: String,
}

impl ValidationError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ValidationError {}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicAuth {
    pub bearer: String,
}

impl std::fmt::Debug for PublicAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicAuth")
            .field("bearer", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicRequest {
    pub protocol: String,
    pub request_id: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<PublicAuth>,
    #[serde(flatten)]
    pub command: PublicCommand,
}

/// Bounded request metadata used only to classify wire decoding failures.
///
/// This is not dispatchable: `input` remains opaque until `PublicRequest` is
/// decoded and validated. It lets transports distinguish an unsupported
/// operation from malformed input without constructing a legacy command.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicRequestHead {
    pub protocol: String,
    pub request_id: uuid::Uuid,
    #[serde(default)]
    pub auth: Option<PublicAuth>,
    pub operation: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

impl PublicRequestHead {
    pub fn validate_metadata(&self) -> Result<(), ValidationError> {
        if self.protocol != PERSONAL_PROTOCOL {
            return Err(ValidationError::new(format!(
                "protocol must be exactly '{PERSONAL_PROTOCOL}'"
            )));
        }
        super::validate_uuid(self.request_id, "request_id")?;
        validate_non_empty_bounded(&self.operation, 128, "operation")?;
        if let Some(auth) = &self.auth {
            validate_non_empty_bounded(&auth.bearer, MAX_AUTH_BYTES, "auth.bearer")?;
        }
        Ok(())
    }

    pub fn operation_supported(&self) -> bool {
        super::PUBLIC_OPERATIONS.contains(&self.operation.as_str())
    }
}

impl PublicRequest {
    pub fn new(request_id: uuid::Uuid, auth: Option<PublicAuth>, command: PublicCommand) -> Self {
        Self {
            protocol: PERSONAL_PROTOCOL.to_string(),
            request_id,
            auth,
            command,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol != PERSONAL_PROTOCOL {
            return Err(ValidationError::new(format!(
                "protocol must be exactly '{PERSONAL_PROTOCOL}'"
            )));
        }
        validate_uuid(self.request_id, "request_id")?;
        if let Some(auth) = &self.auth {
            validate_non_empty_bounded(&auth.bearer, MAX_AUTH_BYTES, "auth.bearer")?;
        }
        self.command.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", content = "input")]
pub enum PublicCommand {
    #[serde(rename = "capabilities.describe")]
    CapabilitiesDescribe(EmptyInput),
    #[serde(rename = "runtime.readiness")]
    RuntimeReadiness(EmptyInput),
    #[serde(rename = "object.put")]
    ObjectPut(ObjectPutInput),
    #[serde(rename = "object.get")]
    ObjectGet(ObjectGetInput),
    #[serde(rename = "object.search")]
    ObjectSearch(ObjectSearchInput),
    #[serde(rename = "memory.create")]
    MemoryCreate(MemoryCreateInput),
    #[serde(rename = "memory.get")]
    MemoryGet(MemoryEntryInput),
    #[serde(rename = "memory.recall")]
    MemoryRecall(MemoryRecallInput),
    #[serde(rename = "memory.update")]
    MemoryUpdate(MemoryUpdateInput),
    #[serde(rename = "memory.delete")]
    MemoryDelete(MemoryEntryInput),
    #[serde(rename = "projection.status")]
    ProjectionStatus(ProjectionStatusInput),
    #[serde(rename = "projection.rebuild")]
    ProjectionRebuild(ProjectionRebuildInput),
    #[serde(rename = "session.start")]
    SessionStart(SessionStartInput),
    #[serde(rename = "session.end")]
    SessionEnd(SessionEndInput),
}

impl PublicCommand {
    pub fn operation(&self) -> &'static str {
        match self {
            Self::CapabilitiesDescribe(_) => "capabilities.describe",
            Self::RuntimeReadiness(_) => "runtime.readiness",
            Self::ObjectPut(_) => "object.put",
            Self::ObjectGet(_) => "object.get",
            Self::ObjectSearch(_) => "object.search",
            Self::MemoryCreate(_) => "memory.create",
            Self::MemoryGet(_) => "memory.get",
            Self::MemoryRecall(_) => "memory.recall",
            Self::MemoryUpdate(_) => "memory.update",
            Self::MemoryDelete(_) => "memory.delete",
            Self::ProjectionStatus(_) => "projection.status",
            Self::ProjectionRebuild(_) => "projection.rebuild",
            Self::SessionStart(_) => "session.start",
            Self::SessionEnd(_) => "session.end",
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::CapabilitiesDescribe(_) | Self::RuntimeReadiness(_) | Self::SessionStart(_) => Ok(()),
            Self::ObjectPut(input) => input.validate(),
            Self::ObjectGet(input) => validate_cid(&input.cid),
            Self::ObjectSearch(input) => input.validate(),
            Self::MemoryCreate(input) => input.validate(),
            Self::MemoryGet(input) | Self::MemoryDelete(input) => validate_uuid(input.entry_id, "entry_id"),
            Self::MemoryRecall(input) => input.validate(),
            Self::MemoryUpdate(input) => input.validate(),
            Self::ProjectionStatus(input) => input.validate(),
            Self::ProjectionRebuild(input) => input.validate(),
            Self::SessionEnd(input) => validate_uuid(input.session_id, "session_id"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmptyInput {}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObjectEncoding {
    #[default]
    Utf8,
    Base64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectPutInput {
    pub content: String,
    #[serde(default)]
    pub encoding: ObjectEncoding,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ObjectPutInput {
    fn validate(&self) -> Result<(), ValidationError> {
        match self.encoding {
            ObjectEncoding::Utf8 => {
                validate_non_empty_bounded(&self.content, MAX_OBJECT_BYTES, "content")?;
            }
            ObjectEncoding::Base64 => {
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&self.content)
                    .map_err(|_| ValidationError::new("content must be valid standard base64"))?;
                if decoded.is_empty() || decoded.len() > MAX_OBJECT_BYTES {
                    return Err(ValidationError::new(format!(
                        "decoded content must contain 1..={MAX_OBJECT_BYTES} bytes"
                    )));
                }
            }
        }
        validate_tags(&self.tags)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectGetInput {
    pub cid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectSearchInput {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub require_tags: Vec<String>,
    #[serde(default)]
    pub exclude_tags: Vec<String>,
}

impl ObjectSearchInput {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_bounded(&self.query, MAX_QUERY_BYTES, "query")?;
        validate_limit(self.limit)?;
        validate_tags(&self.require_tags)?;
        validate_tags(&self.exclude_tags)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryCreateInput {
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl MemoryCreateInput {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_bounded(&self.content, MAX_TEXT_BYTES, "content")?;
        validate_tags(&self.tags)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntryInput {
    pub entry_id: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecallInput {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

impl MemoryRecallInput {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_non_empty_bounded(&self.query, MAX_QUERY_BYTES, "query")?;
        validate_limit(self.limit)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryUpdateInput {
    pub entry_id: uuid::Uuid,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKindInput {
    MemoryEmbedding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionStatusInput {
    pub kind: ProjectionKindInput,
    pub revision_id: uuid::Uuid,
}

impl ProjectionStatusInput {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid(self.revision_id, "revision_id")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionRebuildSelectorInput {
    CurrentRevision { revision_id: uuid::Uuid },
    AllEligible,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRebuildInput {
    pub kind: ProjectionKindInput,
    pub selector: ProjectionRebuildSelectorInput,
}

impl ProjectionRebuildInput {
    fn validate(&self) -> Result<(), ValidationError> {
        if let ProjectionRebuildSelectorInput::CurrentRevision { revision_id } = self.selector {
            validate_uuid(revision_id, "selector.revision_id")?;
        }
        Ok(())
    }
}

impl MemoryUpdateInput {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_uuid(self.entry_id, "entry_id")?;
        validate_non_empty_bounded(&self.content, MAX_TEXT_BYTES, "content")
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionStartInput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_seq: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionEndInput {
    pub session_id: uuid::Uuid,
}

const fn default_limit() -> usize {
    DEFAULT_LIMIT
}
