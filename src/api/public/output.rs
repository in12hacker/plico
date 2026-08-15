use serde::{Deserialize, Serialize};

use super::{
    PublicRequest, ValidationError, MAX_LIMIT, MAX_OBJECT_BYTES, MAX_QUERY_BYTES, MAX_TAGS, MAX_TAG_BYTES,
    MAX_TEXT_BYTES, PERSONAL_PROTOCOL, PUBLIC_OPERATIONS,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicResponse {
    pub protocol: String,
    pub request_id: uuid::Uuid,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<PublicData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PublicError>,
}

impl PublicResponse {
    pub fn success(request_id: uuid::Uuid, data: PublicData) -> Self {
        Self {
            protocol: PERSONAL_PROTOCOL.to_string(),
            request_id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(request_id: uuid::Uuid, error: PublicError) -> Self {
        Self {
            protocol: PERSONAL_PROTOCOL.to_string(),
            request_id,
            ok: false,
            data: None,
            error: Some(error),
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.protocol != PERSONAL_PROTOCOL {
            return Err(ValidationError::new(format!(
                "protocol must be exactly '{PERSONAL_PROTOCOL}'"
            )));
        }
        super::validate_uuid(self.request_id, "request_id")?;
        match (self.ok, self.data.is_some(), self.error.is_some()) {
            (true, true, false) | (false, false, true) => Ok(()),
            _ => Err(ValidationError::new(
                "response must contain exactly one of data or error matching ok",
            )),
        }
    }

    pub fn validate_for(&self, request: &PublicRequest) -> Result<(), ValidationError> {
        self.validate()?;
        if self.request_id != request.request_id {
            return Err(ValidationError::new("response request_id does not match request"));
        }
        if let Some(data) = &self.data {
            if data.operation() != request.command.operation() {
                return Err(ValidationError::new(
                    "response operation does not match request operation",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "operation", content = "result")]
pub enum PublicData {
    #[serde(rename = "capabilities.describe")]
    CapabilitiesDescribe(CapabilityCatalog),
    #[serde(rename = "runtime.readiness")]
    RuntimeReadiness(ReadinessView),
    #[serde(rename = "object.put")]
    ObjectPut(ObjectPutResult),
    #[serde(rename = "object.get")]
    ObjectGet(ObjectView),
    #[serde(rename = "object.search")]
    ObjectSearch(ObjectSearchResult),
    #[serde(rename = "memory.create")]
    MemoryCreate(MemoryWriteResult),
    #[serde(rename = "memory.get")]
    MemoryGet(MemoryEntryView),
    #[serde(rename = "memory.recall")]
    MemoryRecall(MemoryRecallResult),
    #[serde(rename = "memory.update")]
    MemoryUpdate(MemoryUpdateResult),
    #[serde(rename = "memory.delete")]
    MemoryDelete(MemoryDeleteResult),
    #[serde(rename = "projection.status")]
    ProjectionStatus(ProjectionStatusResult),
    #[serde(rename = "projection.rebuild")]
    ProjectionRebuild(ProjectionRebuildResult),
    #[serde(rename = "session.start")]
    SessionStart(SessionStartResult),
    #[serde(rename = "session.end")]
    SessionEnd(SessionEndResult),
}

impl PublicData {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PublicErrorCode {
    InvalidArgument,
    Unauthenticated,
    PermissionDenied,
    NotFound,
    Conflict,
    LimitExceeded,
    Busy,
    ProviderUnavailable,
    DependencyUnavailable,
    UnsupportedCapability,
    Internal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublicError {
    pub code: PublicErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CapabilityCatalog {
    pub protocol: String,
    pub operations: Vec<String>,
    pub limits: PublicLimits,
    pub projections: ProjectionCapabilities,
    pub consistency: Vec<String>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionCapabilities {
    pub memory_embedding: MemoryEmbeddingCapabilities,
    pub memory_vector_recall: CapabilitySupport,
    pub memory_hybrid_recall: CapabilitySupport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingCapabilities {
    pub control_plane: CapabilitySupport,
    pub retrieval: CapabilitySupport,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilitySupport {
    Supported,
    Unsupported,
}

impl Default for CapabilityCatalog {
    fn default() -> Self {
        Self {
            protocol: PERSONAL_PROTOCOL.to_string(),
            operations: PUBLIC_OPERATIONS
                .iter()
                .map(|operation| (*operation).to_string())
                .collect(),
            limits: PublicLimits::default(),
            projections: ProjectionCapabilities {
                memory_embedding: MemoryEmbeddingCapabilities {
                    control_plane: CapabilitySupport::Supported,
                    retrieval: CapabilitySupport::Unsupported,
                },
                memory_vector_recall: CapabilitySupport::Unsupported,
                memory_hybrid_recall: CapabilitySupport::Unsupported,
            },
            consistency: vec![
                "object CID reads are content-verified".to_string(),
                "memory writes acknowledge canonical persistence before projection work".to_string(),
                "memory lexical recall is immediately visible after commit".to_string(),
            ],
            unsupported: vec![
                "memory BM25 or hybrid recall".to_string(),
                "thermal or deep recall".to_string(),
                "evidence projection".to_string(),
                "human document projection".to_string(),
                "session checkpoint".to_string(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PublicLimits {
    pub max_text_bytes: usize,
    pub max_object_bytes: usize,
    pub max_query_bytes: usize,
    pub max_tags: usize,
    pub max_tag_bytes: usize,
    pub max_results: usize,
}

impl Default for PublicLimits {
    fn default() -> Self {
        Self {
            max_text_bytes: MAX_TEXT_BYTES,
            max_object_bytes: MAX_OBJECT_BYTES,
            max_query_bytes: MAX_QUERY_BYTES,
            max_tags: MAX_TAGS,
            max_tag_bytes: MAX_TAG_BYTES,
            max_results: MAX_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComponentState {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReadinessView {
    pub ready: bool,
    pub canonical_store: ComponentState,
    pub canonical_memory_persistence: ComponentState,
    pub projection: ProjectionReadinessView,
    pub cognitive_worker: ComponentState,
    pub embedding_provider: ComponentState,
    pub configured_embedding_backend: String,
    pub active_embedding_provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectPutResult {
    pub cid: String,
    pub commit: CommitState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ObjectView {
    pub cid: String,
    pub content_base64: String,
    pub content_type: String,
    pub tags: Vec<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObjectSearchResult {
    pub hits: Vec<ObjectSearchHit>,
    pub retrieval: Vec<RetrievalExecution>,
    pub embedding_query: EmbeddingQueryView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObjectSearchHit {
    pub cid: String,
    pub score: f32,
    pub snippet: String,
    pub tags: Vec<String>,
    pub content_type: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalPath {
    Bm25,
    Vector,
    TagFallback,
    KnowledgeGraphTemporal,
    KnowledgeGraphPpr,
    KnowledgeGraphPathDiscovery,
    KnowledgeGraphCausal,
    Reranker,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetrievalExecution {
    pub path: RetrievalPath,
    pub candidates: usize,
    pub accepted: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation: Option<RetrievalDegradation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalDegradation {
    ExecutionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingQueryView {
    pub state: EmbeddingQueryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation: Option<EmbeddingQueryDegradation>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingQueryStatus {
    NotProbed,
    Succeeded,
    Degraded,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingQueryDegradation {
    ProviderUnavailable,
    ModelUnavailable,
    InputRejected,
    ExecutionFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntryView {
    pub entry_id: uuid::Uuid,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryWriteResult {
    pub entry: MemoryEntryView,
    pub commit: CommitState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecallResult {
    pub hits: Vec<MemoryRecallHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecallHit {
    pub entry: MemoryEntryView,
    pub score: f32,
    pub matched_by: MemoryMatchPath,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryMatchPath {
    LexicalOverlap,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryUpdateResult {
    pub previous_entry_id: uuid::Uuid,
    pub entry: MemoryEntryView,
    pub commit: CommitState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MemoryDeleteResult {
    pub entry_id: uuid::Uuid,
    pub deleted_at: u64,
    pub commit: CommitState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionReadinessView {
    pub control_plane: ComponentState,
    pub worker: ComponentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_plane_reason: Option<ProjectionUnavailableCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_reason: Option<ProjectionUnavailableCategory>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionUnavailableCategory {
    ProjectionNotInitialized,
    OwnerResumeRequired,
    BuilderChangeRequired,
    ResetRequired,
    UnsupportedFormat,
    VaultLocked,
    PermissionDenied,
    StorageIo,
    ResourceExhausted,
    ResetPending,
    MaintenanceRequired,
    ManualIntervention,
    IdentityUnavailable,
    ProviderChangedRestartRequired,
    WorkerRestartRequired,
    RuntimeShuttingDown,
    RuntimeStateUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CanonicalWatermarkView {
    pub generation: u64,
    pub revision_watermark: u64,
    pub policy_watermark: u64,
    pub relation_watermark: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionStateView {
    AbsentByPolicy {
        reason: ProjectionAbsentReason,
    },
    Queued {
        reason: ProjectionQueueReason,
    },
    Building,
    Ready,
    Failed {
        attempt: u32,
        failure_category: ProjectionFailureCategory,
        retryable: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_not_before: Option<u64>,
    },
    Stale {
        reason: ProjectionStaleReason,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionAbsentReason {
    Superseded,
    Deleted,
    UnsupportedTier,
    UnsupportedContent,
    BlankText,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionQueueReason {
    CanonicalCommit,
    Reconciliation,
    Retry,
    OwnerRebuild,
    BuilderChanged,
    LeaseExpired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionStaleReason {
    BuilderSpecChanged,
    ArtifactMissing,
    ArtifactHashMismatch,
    ArtifactInvalid,
    OwnerRebuild,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionFailureCategory {
    ProviderUnavailable,
    ProviderIdentityChanged,
    InvalidProjection,
    ArtifactStoreUnavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "observation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionObservationView {
    Observed {
        revision_id: uuid::Uuid,
        content_hash: String,
        state: ProjectionStateView,
        event_watermark: u64,
        reconciled_source: CanonicalWatermarkView,
    },
    Unreconciled {
        revision_id: uuid::Uuid,
        content_hash: String,
        event_watermark: u64,
        reconciled_source: CanonicalWatermarkView,
    },
    Unavailable {
        revision_id: uuid::Uuid,
        content_hash: String,
        reason: ProjectionUnavailableCategory,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionStatusResult {
    pub kind: super::ProjectionKindInput,
    pub status: ProjectionObservationView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRebuildResult {
    pub kind: super::ProjectionKindInput,
    pub selected_count: u64,
    pub manifest_generation: u64,
    pub event_watermark: u64,
    pub reconciled_source: CanonicalWatermarkView,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommitState {
    Canonical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionChangeView {
    pub seq: u64,
    pub change_type: String,
    pub object_id: String,
    pub changed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionStartResult {
    pub session_id: uuid::Uuid,
    pub watermark: u64,
    pub changes: Vec<SessionChangeView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SessionEndResult {
    pub session_id: uuid::Uuid,
    pub last_seq: u64,
}
