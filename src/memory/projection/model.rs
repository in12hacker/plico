use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::memory::{CanonicalContentHash, MemoryId, MemoryRevisionId};

pub const BUILDER_SPEC_SCHEMA: &str = "plico.projection.builder-spec/v1";
pub const MANIFEST_RECORD_SCHEMA: &str = "plico.projection.manifest-record/v1";
pub const MANIFEST_SEGMENT_SCHEMA: &str = "plico.projection.manifest-segment/v1";
pub const MANIFEST_ROOT_SCHEMA: &str = "plico.projection.manifest-root/v1";
pub const CURRENT_VIEW_SCHEMA: &str = "plico.projection.current-view/v1";
pub const ROOT_POINTER_SCHEMA: &str = "plico.projection.root-pointer/v1";
pub const EMBEDDING_ARTIFACT_SCHEMA: &str = "plico.projection.embedding-artifact/v1";
pub const MAX_EMBEDDING_DIMENSION: u32 = 65_536;
pub const MAX_EMBEDDING_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_PROJECTION_POINTER_BYTES: u64 = 4 * 1024;
pub const MAX_PROJECTION_MANIFEST_OBJECT_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    MemoryEmbedding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingInputContract {
    MemoryTextUtf8V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingOperationContract {
    DocumentV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingNormalization {
    ProviderNative,
    L2AfterMatryoshkaTruncationV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalSourceIdentity {
    pub canonical_kind: String,
    pub memory_id: MemoryId,
    pub revision_id: MemoryRevisionId,
    pub revision_sequence: u64,
    pub content_hash: CanonicalContentHash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalWatermark {
    pub root_hash: String,
    pub generation: u64,
    pub revision_watermark: u64,
    pub policy_watermark: u64,
    pub relation_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuilderSpec {
    pub schema: String,
    pub projection_kind: ProjectionKind,
    pub builder_id: String,
    pub builder_version: String,
    pub provider_family: String,
    pub provider_compatibility_id: String,
    pub model_id: String,
    pub raw_dimension: u32,
    pub dimension: u32,
    pub input_contract: EmbeddingInputContract,
    pub operation_contract: EmbeddingOperationContract,
    pub normalization: EmbeddingNormalization,
    pub transform_contract_id: String,
    pub artifact_schema: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactDescriptor {
    pub artifact_hash: String,
    pub byte_length: u64,
    pub artifact_schema: String,
    pub dimension: u32,
    pub source_revision_id: MemoryRevisionId,
    pub source_content_hash: CanonicalContentHash,
    pub builder_spec_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbsentReason {
    Superseded,
    Deleted,
    UnsupportedTier,
    UnsupportedContent,
    BlankText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueReason {
    CanonicalCommit,
    Reconciliation,
    Retry,
    OwnerRebuild,
    BuilderChanged,
    LeaseExpired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StaleReason {
    BuilderSpecChanged,
    ArtifactMissing,
    ArtifactHashMismatch,
    ArtifactInvalid,
    OwnerRebuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    ProviderUnavailable,
    ProviderIdentityChanged,
    InvalidProjection,
    ArtifactStoreUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectionState {
    AbsentByPolicy {
        reason: AbsentReason,
    },
    Queued {
        reason: QueueReason,
    },
    Building {
        attempt: u32,
        attempt_id: Uuid,
        lease_expires_at: u64,
    },
    Ready {
        attempt: u32,
        attempt_id: Uuid,
        artifact: ArtifactDescriptor,
    },
    Failed {
        attempt: u32,
        attempt_id: Uuid,
        failure_category: FailureCategory,
        retryable: bool,
        retry_not_before: Option<u64>,
    },
    Stale {
        reason: StaleReason,
        artifact: ArtifactDescriptor,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestEvent {
    BuilderActivated {
        projection_kind: ProjectionKind,
        builder_spec: BuilderSpec,
        builder_spec_hash: String,
        previous_builder_spec_hash: Option<String>,
    },
    ProjectionTransition {
        projection_id: Uuid,
        projection_kind: ProjectionKind,
        projection_version: u64,
        previous_sequence: Option<u64>,
        source: CanonicalSourceIdentity,
        desired_builder_spec_hash: String,
        state: ProjectionState,
    },
    ReconciliationAdvanced {
        previous_source: CanonicalWatermark,
        reconciled_source: CanonicalWatermark,
        classified_revision_count: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestRecord {
    pub schema: String,
    pub sequence: u64,
    pub committed_at: u64,
    pub committed_by_role: String,
    pub event: ManifestEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionSegment {
    pub schema: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub previous_segment_hash: Option<String>,
    pub records: Vec<ManifestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionRoot {
    pub schema: String,
    pub generation: u64,
    pub previous_root_hash: Option<String>,
    pub manifest_head: Option<String>,
    pub event_watermark: u64,
    pub current_view_hash: String,
    pub reconciled_source: CanonicalWatermark,
    pub committed_at: u64,
    pub committed_by_role: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionEntry {
    pub projection_id: Uuid,
    pub projection_kind: ProjectionKind,
    pub projection_version: u64,
    pub last_transition_sequence: u64,
    pub attempt_count: u32,
    pub source: CanonicalSourceIdentity,
    pub desired_builder_spec_hash: String,
    pub state: ProjectionState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveBuilderSpec {
    pub projection_kind: ProjectionKind,
    pub builder_spec_hash: String,
    pub builder_spec: BuilderSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionCurrentView {
    pub schema: String,
    pub generation: u64,
    pub event_watermark: u64,
    pub reconciled_source: CanonicalWatermark,
    pub active_builder_specs: Vec<ActiveBuilderSpec>,
    pub entries: Vec<ProjectionEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootPointer {
    pub schema: String,
    pub root_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingArtifact {
    pub schema: String,
    pub projection_id: Uuid,
    pub source_revision_id: MemoryRevisionId,
    pub source_content_hash: CanonicalContentHash,
    pub builder_spec_hash: String,
    pub dimension: u32,
    pub encoding: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("projection I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("projection serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid projection state: {category}")]
    Invalid { category: &'static str },
    #[error("unsupported projection format: {component}")]
    UnsupportedFormat { component: &'static str },
    #[error("projection cutover requires the all-eligible selector")]
    AllEligibleRequired,
    #[error("projection manifest head conflict")]
    HeadConflict,
    #[error("projection publish outcome is indeterminate; restart is required")]
    CommitIndeterminate,
    #[error("projection writer is poisoned; restart is required")]
    WriterPoisoned,
    #[error("projection artifact repair is required for {count} entries")]
    ArtifactRepairRequired { count: usize },
    #[error("projection artifact maintenance is required")]
    ArtifactMaintenanceRequired,
    #[error("projection store maintenance is required")]
    ProjectionMaintenanceRequired,
    #[error("projection reset transaction requires owner recovery")]
    ResetPending,
    #[error("projection reset transaction requires manual intervention")]
    ManualInterventionRequired,
    #[error("projection artifact store is unavailable")]
    ArtifactStoreUnavailable,
}
