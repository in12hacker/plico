//! Typed `plico.personal.v2` domain service.

use base64::Engine;

use crate::api::agent_auth::PersonalOwnerCredentialState;
use crate::api::public::*;
use crate::api::{PermissionAction, PermissionContext};
use crate::cas::ObjectScope;
use crate::fs::search::{EmbeddingDegradation, EmbeddingQueryState, SearchPath, SearchStageDegradation};
use crate::kernel::ops::projection_runtime::{
    ProjectionRuntimeError, ProjectionRuntimeStatus, ProjectionUnavailableReason,
};
use crate::memory::projection::{
    AbsentReason, CanonicalWatermark, FailureCategory, ProjectionRebuildSelector, ProjectionStatusObservation,
    ProjectionStatusState, QueueReason, StaleReason,
};
use crate::memory::{DurableMemoryMutationError, MemoryContent, MemoryEntry, MemoryTier};
use crate::PERSONAL_OWNER_ROLE_ID;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicTransport {
    Tcp,
    Uds,
    Embedded,
    Mcp,
}

impl PublicTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Uds => "uds",
            Self::Embedded => "embedded",
            Self::Mcp => "mcp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicAccess {
    LocalOwner,
    AuthenticatedRole(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicRequestContext {
    pub access: PublicAccess,
    pub transport: PublicTransport,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("invalid bearer credential")]
pub struct PublicAuthenticationError;

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
#[error("personal owner credential unavailable")]
pub struct PublicCredentialBootstrapError;

impl PublicRequestContext {
    pub const fn local_owner(transport: PublicTransport) -> Self {
        Self {
            access: PublicAccess::LocalOwner,
            transport,
        }
    }

    pub fn authenticated_role(role_id: String, transport: PublicTransport) -> Self {
        Self {
            access: if role_id == PERSONAL_OWNER_ROLE_ID {
                PublicAccess::LocalOwner
            } else {
                PublicAccess::AuthenticatedRole(role_id)
            },
            transport,
        }
    }

    pub fn role_id(&self) -> &str {
        match &self.access {
            PublicAccess::LocalOwner => PERSONAL_OWNER_ROLE_ID,
            PublicAccess::AuthenticatedRole(role_id) => role_id,
        }
    }
}

impl super::AIKernel {
    /// Resolve a public bearer to its local role without exposing keystore
    /// state or authentication failure detail.
    pub fn authenticate_public_bearer(&self, bearer: &str) -> Result<String, PublicAuthenticationError> {
        self.key_store
            .authenticate_bearer(bearer)
            .map_err(|_| PublicAuthenticationError)
    }

    /// Ensure TCP has a durable personal-owner bearer without exposing it in
    /// the return value or logs. The returned owner-only path is the explicit
    /// local distribution point; no public business operation bootstraps auth.
    pub fn ensure_personal_owner_credential(&self) -> Result<std::path::PathBuf, PublicCredentialBootstrapError> {
        let state = self
            .key_store
            .ensure_personal_owner_credential(&self.root)
            .map_err(|_| PublicCredentialBootstrapError)?;
        let path = crate::api::AgentKeyStore::credential_path(&self.root);
        tracing::info!(
            credential_state = match state {
                PersonalOwnerCredentialState::Created => "created",
                PersonalOwnerCredentialState::Existing => "existing",
            },
            credential_file = "agent_tokens.json",
            "personal owner credential ready"
        );
        Ok(path)
    }

    pub fn handle_public_request(&self, context: &PublicRequestContext, request: PublicRequest) -> PublicResponse {
        let request_id = request.request_id;
        let operation = request.command.operation();
        let span = tracing::info_span!(
            "public_request",
            request_id = %request_id,
            operation,
            transport = context.transport.as_str(),
            role_kind = if context.access == PublicAccess::LocalOwner { "personal_owner" } else { "agent_role" },
        );
        let _guard = span.enter();

        if context.transport != PublicTransport::Tcp && request.auth.is_some() {
            tracing::warn!(
                outcome = "error",
                error_category = "invalid_argument",
                "public request rejected"
            );
            return PublicResponse::failure(
                request_id,
                invalid_argument("auth is accepted only by the tcp transport"),
            );
        }

        if let Err(error) = request.validate() {
            tracing::warn!(
                outcome = "error",
                error_category = "invalid_argument",
                "public request rejected"
            );
            return PublicResponse::failure(request_id, invalid_argument(error.message));
        }

        let data = match self.dispatch_public(context, request_id, request.command) {
            Ok(data) => {
                tracing::info!(outcome = "success", "public request completed");
                return PublicResponse::success(request_id, data);
            }
            Err(error) => error,
        };
        let error_category = data
            .details
            .as_ref()
            .and_then(|details| details.get("category"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("public_error");
        tracing::warn!(
            outcome = "error",
            error_category,
            retryable = data.retryable,
            "public request failed"
        );
        PublicResponse::failure(request_id, data)
    }

    fn dispatch_public(
        &self,
        context: &PublicRequestContext,
        request_id: uuid::Uuid,
        command: PublicCommand,
    ) -> Result<PublicData, PublicError> {
        let role_id = context.role_id();
        match command {
            PublicCommand::CapabilitiesDescribe(_) => {
                Ok(PublicData::CapabilitiesDescribe(CapabilityCatalog::default()))
            }
            PublicCommand::RuntimeReadiness(_) => {
                let readiness = self.runtime_readiness();
                Ok(PublicData::RuntimeReadiness(ReadinessView {
                    ready: readiness.ready,
                    canonical_store: component(readiness.canonical_initialized),
                    canonical_memory_persistence: component(readiness.canonical_memory_ledger_present),
                    projection: projection_readiness(&readiness.projection),
                    cognitive_worker: component(readiness.workers.cognitive_present),
                    cognitive_progress: readiness.workers.cognitive_progress.map(|progress| {
                        CognitivePipelineProgressView {
                            accepted: progress.accepted,
                            completed: progress.completed,
                            in_flight: progress.in_flight,
                        }
                    }),
                    embedding_provider: match readiness.embedding.probe_state {
                        super::ops::readiness::ProviderProbeState::Verified => ComponentState::Ready,
                        super::ops::readiness::ProviderProbeState::Unavailable => ComponentState::Unavailable,
                    },
                    configured_embedding_backend: readiness.embedding.configured_backend,
                    active_embedding_provider: readiness.embedding.active_provider,
                }))
            }
            PublicCommand::ObjectPut(input) => {
                let content = match input.encoding {
                    ObjectEncoding::Utf8 => input.content.into_bytes(),
                    ObjectEncoding::Base64 => base64::engine::general_purpose::STANDARD
                        .decode(input.content)
                        .map_err(|_| invalid_argument("content must be valid standard base64"))?,
                };
                let cid = self
                    .semantic_create(content, input.tags, role_id, None, ObjectScope::Private)
                    .map_err(map_io_error)?;
                Ok(PublicData::ObjectPut(ObjectPutResult {
                    cid,
                    commit: CommitState::Canonical,
                }))
            }
            PublicCommand::ObjectGet(input) => {
                let object = self
                    .get_object(&input.cid, role_id, crate::DEFAULT_TENANT)
                    .map_err(map_io_error)?;
                Ok(PublicData::ObjectGet(ObjectView {
                    cid: object.cid,
                    content_base64: base64::engine::general_purpose::STANDARD.encode(object.data),
                    content_type: object.meta.content_type.to_string(),
                    tags: object.meta.tags,
                    created_at: object.meta.created_at,
                }))
            }
            PublicCommand::ObjectSearch(input) => {
                let diagnosed = self
                    .semantic_search_diagnosed(
                        &input.query,
                        role_id,
                        input.limit,
                        input.require_tags,
                        input.exclude_tags,
                    )
                    .map_err(map_io_error)?;
                let hits = diagnosed
                    .results
                    .into_iter()
                    .map(|hit| ObjectSearchHit {
                        cid: hit.cid,
                        score: hit.relevance,
                        snippet: hit.snippet,
                        tags: hit.meta.tags,
                        content_type: hit.meta.content_type.to_string(),
                        created_at: hit.meta.created_at,
                    })
                    .collect();
                let retrieval = diagnosed
                    .execution
                    .paths
                    .into_iter()
                    .map(|path| RetrievalExecution {
                        path: map_search_path(path.path),
                        candidates: path.candidates,
                        accepted: path.accepted,
                        degradation: path.degradation.map(map_stage_degradation),
                    })
                    .collect();
                Ok(PublicData::ObjectSearch(ObjectSearchResult {
                    hits,
                    retrieval,
                    embedding_query: map_embedding_query(diagnosed.execution.embedding),
                }))
            }
            PublicCommand::MemoryCreate(input) => {
                let entry = self
                    .remember_working_with_request_id(
                        role_id,
                        crate::DEFAULT_TENANT,
                        input.content,
                        input.tags,
                        Some(request_id),
                    )
                    .map_err(map_memory_error)?;
                Ok(PublicData::MemoryCreate(MemoryWriteResult {
                    entry: memory_view(entry)?,
                    commit: CommitState::Canonical,
                }))
            }
            PublicCommand::MemoryGet(input) => {
                self.check_public_permission(context, PermissionAction::Read)?;
                Ok(PublicData::MemoryGet(memory_view(
                    self.public_working_entry(role_id, input.entry_id)?,
                )?))
            }
            PublicCommand::MemoryRecall(input) => {
                self.check_public_permission(context, PermissionAction::Read)?;
                let hits = self
                    .memory
                    .recall_working_lexical_authorized(role_id, crate::DEFAULT_TENANT, &input.query, input.limit)
                    .map_err(|error| map_memory_error(DurableMemoryMutationError::Ledger(error)))?
                    .into_iter()
                    .map(|(entry, score)| {
                        Ok(MemoryRecallHit {
                            entry: memory_view(entry)?,
                            score,
                            matched_by: MemoryMatchPath::LexicalOverlap,
                        })
                    })
                    .collect::<Result<Vec<_>, PublicError>>()?;
                Ok(PublicData::MemoryRecall(MemoryRecallResult { hits }))
            }
            PublicCommand::MemoryUpdate(input) => {
                self.check_public_permission(context, PermissionAction::Write)?;
                let entry = self
                    .memory_update_with_request_id(
                        role_id,
                        crate::DEFAULT_TENANT,
                        &input.entry_id.to_string(),
                        input.content,
                        Some(request_id),
                    )
                    .map_err(map_memory_error)?;
                Ok(PublicData::MemoryUpdate(MemoryUpdateResult {
                    previous_entry_id: input.entry_id,
                    entry: memory_view(entry)?,
                    commit: CommitState::Canonical,
                }))
            }
            PublicCommand::MemoryDelete(input) => {
                self.check_public_permission(context, PermissionAction::Delete)?;
                let deleted = self
                    .memory_delete_with_request_id(
                        role_id,
                        crate::DEFAULT_TENANT,
                        &input.entry_id.to_string(),
                        Some(request_id),
                    )
                    .map_err(map_memory_error)?;
                Ok(PublicData::MemoryDelete(MemoryDeleteResult {
                    entry_id: input.entry_id,
                    deleted_at: deleted.deleted_at.ok_or_else(|| internal("deleted_at_missing"))?,
                    commit: CommitState::Canonical,
                }))
            }
            PublicCommand::ProjectionStatus(input) => {
                let status = self
                    .projection
                    .status_authorized(role_id, &input.revision_id.to_string().into())
                    .map_err(map_projection_runtime_error)?
                    .ok_or_else(|| not_found("projection_revision"))?;
                Ok(PublicData::ProjectionStatus(ProjectionStatusResult {
                    kind: input.kind,
                    status: map_projection_status(status)?,
                }))
            }
            PublicCommand::ProjectionRebuild(input) => {
                if context.access != PublicAccess::LocalOwner {
                    return Err(public_error(
                        PublicErrorCode::PermissionDenied,
                        "projection rebuild requires the personal owner",
                        false,
                        "projection_owner_required",
                    ));
                }
                let selector = match input.selector {
                    ProjectionRebuildSelectorInput::CurrentRevision { revision_id } => {
                        ProjectionRebuildSelector::CurrentRevision(revision_id.to_string().into())
                    }
                    ProjectionRebuildSelectorInput::AllEligible => ProjectionRebuildSelector::AllEligible,
                };
                let receipt = self
                    .projection
                    .owner_rebuild(role_id, selector)
                    .map_err(map_projection_runtime_error)?;
                Ok(PublicData::ProjectionRebuild(ProjectionRebuildResult {
                    kind: input.kind,
                    selected_count: receipt.selected_count,
                    manifest_generation: receipt.manifest_generation,
                    event_watermark: receipt.event_watermark,
                    reconciled_source: map_watermark(receipt.reconciled_source),
                }))
            }
            PublicCommand::SessionStart(input) => {
                let started = super::ops::session::start_session(super::ops::session::StartSessionParams {
                    agent_id: role_id,
                    last_seen_seq: input.last_seen_seq,
                    session_store: &self.session_store,
                    event_bus: &self.event_bus,
                    root: &self.root,
                })
                .map_err(map_session_error)?;
                let session_id = parse_internal_uuid(&started.session_id, "session_id")?;
                let changes = started
                    .changes_since_last
                    .into_iter()
                    .map(|change| SessionChangeView {
                        seq: change.seq,
                        change_type: change.change_type,
                        object_id: change.cid,
                        changed_at_ms: change.changed_at_ms,
                    })
                    .collect();
                Ok(PublicData::SessionStart(SessionStartResult {
                    session_id,
                    watermark: started.watermark,
                    changes,
                }))
            }
            PublicCommand::SessionEnd(input) => {
                let ended = super::ops::session::end_session(
                    role_id,
                    &input.session_id.to_string(),
                    &self.session_store,
                    &self.root,
                    &self.event_bus,
                )
                .map_err(map_session_error)?;
                Ok(PublicData::SessionEnd(SessionEndResult {
                    session_id: input.session_id,
                    last_seq: ended.last_seq,
                }))
            }
        }
    }

    fn public_working_entry(&self, role_id: &str, entry_id: uuid::Uuid) -> Result<MemoryEntry, PublicError> {
        self.memory
            .find_active_authorized(role_id, MemoryTier::Working, &entry_id.to_string())
            .map_err(|error| map_memory_error(DurableMemoryMutationError::Ledger(error)))?
            .filter(|entry| entry.tenant_id == crate::DEFAULT_TENANT)
            .ok_or_else(|| not_found("memory_entry"))
    }

    fn check_public_permission(
        &self,
        context: &PublicRequestContext,
        action: PermissionAction,
    ) -> Result<(), PublicError> {
        if context.access == PublicAccess::LocalOwner {
            return Ok(());
        }
        let permission = PermissionContext::new(context.role_id().to_string(), crate::DEFAULT_TENANT.to_string());
        self.permissions.check(&permission, action).map_err(|_| {
            public_error(
                PublicErrorCode::PermissionDenied,
                "operation is not permitted for this local role",
                false,
                "permission_denied",
            )
        })
    }
}

fn component(available: bool) -> ComponentState {
    if available {
        ComponentState::Ready
    } else {
        ComponentState::Unavailable
    }
}

fn projection_readiness(
    snapshot: &super::ops::projection_runtime::ProjectionRuntimeReadinessSnapshot,
) -> ProjectionReadinessView {
    ProjectionReadinessView {
        control_plane: if snapshot.control_plane_ready {
            ComponentState::Ready
        } else {
            ComponentState::Degraded
        },
        worker: if snapshot.worker_ready {
            ComponentState::Ready
        } else {
            ComponentState::Unavailable
        },
        control_plane_reason: snapshot.control_plane_reason.map(map_projection_unavailable),
        worker_reason: snapshot.worker_reason.map(map_projection_unavailable),
    }
}

fn map_projection_status(status: ProjectionRuntimeStatus) -> Result<ProjectionObservationView, PublicError> {
    match status {
        ProjectionRuntimeStatus::Projection(ProjectionStatusObservation::Observed {
            revision_id,
            content_hash,
            state,
            event_watermark,
            reconciled_source,
        }) => Ok(ProjectionObservationView::Observed {
            revision_id: parse_internal_uuid(revision_id.as_str(), "projection_revision_id")?,
            content_hash: content_hash.to_string(),
            state: map_projection_state(state),
            event_watermark,
            reconciled_source: map_watermark(reconciled_source),
        }),
        ProjectionRuntimeStatus::Projection(ProjectionStatusObservation::Unreconciled {
            revision_id,
            content_hash,
            event_watermark,
            reconciled_source,
        }) => Ok(ProjectionObservationView::Unreconciled {
            revision_id: parse_internal_uuid(revision_id.as_str(), "projection_revision_id")?,
            content_hash: content_hash.to_string(),
            event_watermark,
            reconciled_source: map_watermark(reconciled_source),
        }),
        ProjectionRuntimeStatus::Unavailable {
            revision_id,
            content_hash,
            reason,
        } => Ok(ProjectionObservationView::Unavailable {
            revision_id: parse_internal_uuid(revision_id.as_str(), "projection_revision_id")?,
            content_hash: content_hash.to_string(),
            reason: map_projection_unavailable(reason),
        }),
    }
}

fn map_projection_state(state: ProjectionStatusState) -> ProjectionStateView {
    match state {
        ProjectionStatusState::AbsentByPolicy { reason } => ProjectionStateView::AbsentByPolicy {
            reason: match reason {
                AbsentReason::Superseded => ProjectionAbsentReason::Superseded,
                AbsentReason::Deleted => ProjectionAbsentReason::Deleted,
                AbsentReason::UnsupportedTier => ProjectionAbsentReason::UnsupportedTier,
                AbsentReason::UnsupportedContent => ProjectionAbsentReason::UnsupportedContent,
                AbsentReason::BlankText => ProjectionAbsentReason::BlankText,
            },
        },
        ProjectionStatusState::Queued { reason } => ProjectionStateView::Queued {
            reason: match reason {
                QueueReason::CanonicalCommit => ProjectionQueueReason::CanonicalCommit,
                QueueReason::Reconciliation => ProjectionQueueReason::Reconciliation,
                QueueReason::Retry => ProjectionQueueReason::Retry,
                QueueReason::OwnerRebuild => ProjectionQueueReason::OwnerRebuild,
                QueueReason::BuilderChanged => ProjectionQueueReason::BuilderChanged,
                QueueReason::LeaseExpired => ProjectionQueueReason::LeaseExpired,
            },
        },
        ProjectionStatusState::Building => ProjectionStateView::Building,
        ProjectionStatusState::Ready => ProjectionStateView::Ready,
        ProjectionStatusState::Failed {
            attempt,
            failure_category,
            retryable,
            retry_not_before,
        } => ProjectionStateView::Failed {
            attempt,
            failure_category: match failure_category {
                FailureCategory::ProviderUnavailable => ProjectionFailureCategory::ProviderUnavailable,
                FailureCategory::ProviderIdentityChanged => ProjectionFailureCategory::ProviderIdentityChanged,
                FailureCategory::InvalidProjection => ProjectionFailureCategory::InvalidProjection,
                FailureCategory::ArtifactStoreUnavailable => ProjectionFailureCategory::ArtifactStoreUnavailable,
            },
            retryable,
            retry_not_before,
        },
        ProjectionStatusState::Stale { reason } => ProjectionStateView::Stale {
            reason: match reason {
                StaleReason::BuilderSpecChanged => ProjectionStaleReason::BuilderSpecChanged,
                StaleReason::ArtifactMissing => ProjectionStaleReason::ArtifactMissing,
                StaleReason::ArtifactHashMismatch => ProjectionStaleReason::ArtifactHashMismatch,
                StaleReason::ArtifactInvalid => ProjectionStaleReason::ArtifactInvalid,
                StaleReason::OwnerRebuild => ProjectionStaleReason::OwnerRebuild,
            },
        },
    }
}

fn map_projection_unavailable(reason: ProjectionUnavailableReason) -> ProjectionUnavailableCategory {
    match reason {
        ProjectionUnavailableReason::ProjectionNotInitialized => {
            ProjectionUnavailableCategory::ProjectionNotInitialized
        }
        ProjectionUnavailableReason::OwnerResumeRequired => ProjectionUnavailableCategory::OwnerResumeRequired,
        ProjectionUnavailableReason::BuilderChangeRequired => ProjectionUnavailableCategory::BuilderChangeRequired,
        ProjectionUnavailableReason::ResetRequired(_) => ProjectionUnavailableCategory::ResetRequired,
        ProjectionUnavailableReason::UnsupportedFormat => ProjectionUnavailableCategory::UnsupportedFormat,
        ProjectionUnavailableReason::VaultLocked => ProjectionUnavailableCategory::VaultLocked,
        ProjectionUnavailableReason::PermissionDenied => ProjectionUnavailableCategory::PermissionDenied,
        ProjectionUnavailableReason::StorageIo => ProjectionUnavailableCategory::StorageIo,
        ProjectionUnavailableReason::ResourceExhausted => ProjectionUnavailableCategory::ResourceExhausted,
        ProjectionUnavailableReason::ResetPending => ProjectionUnavailableCategory::ResetPending,
        ProjectionUnavailableReason::MaintenanceRequired => ProjectionUnavailableCategory::MaintenanceRequired,
        ProjectionUnavailableReason::ManualIntervention => ProjectionUnavailableCategory::ManualIntervention,
        ProjectionUnavailableReason::IdentityUnavailable("provider_changed_restart_required") => {
            ProjectionUnavailableCategory::ProviderChangedRestartRequired
        }
        ProjectionUnavailableReason::IdentityUnavailable(_) => ProjectionUnavailableCategory::IdentityUnavailable,
        ProjectionUnavailableReason::WorkerRestartRequired => ProjectionUnavailableCategory::WorkerRestartRequired,
        ProjectionUnavailableReason::RuntimeShuttingDown => ProjectionUnavailableCategory::RuntimeShuttingDown,
        ProjectionUnavailableReason::RuntimeStateUnavailable => ProjectionUnavailableCategory::RuntimeStateUnavailable,
    }
}

fn map_watermark(watermark: CanonicalWatermark) -> CanonicalWatermarkView {
    CanonicalWatermarkView {
        generation: watermark.generation,
        revision_watermark: watermark.revision_watermark,
        policy_watermark: watermark.policy_watermark,
        relation_watermark: watermark.relation_watermark,
    }
}

fn map_projection_runtime_error(error: ProjectionRuntimeError) -> PublicError {
    match error {
        ProjectionRuntimeError::NotFound => not_found("projection_revision"),
        ProjectionRuntimeError::NotEligible => invalid_argument("projection revision is not eligible"),
        ProjectionRuntimeError::NothingToRebuild => public_error(
            PublicErrorCode::Conflict,
            "no eligible projection revision exists",
            false,
            "projection_nothing_to_rebuild",
        ),
        ProjectionRuntimeError::AllEligibleRequired => public_error(
            PublicErrorCode::Conflict,
            "this projection operation requires selector all_eligible",
            false,
            "projection_all_eligible_required",
        ),
        ProjectionRuntimeError::Unavailable(category) => public_error(
            PublicErrorCode::DependencyUnavailable,
            "projection is unavailable",
            false,
            category,
        ),
    }
}

fn memory_view(entry: MemoryEntry) -> Result<MemoryEntryView, PublicError> {
    let entry_id = parse_internal_uuid(&entry.id, "memory_entry_id")?;
    let MemoryContent::Text(content) = entry.content else {
        return Err(internal("memory_content_not_text"));
    };
    Ok(MemoryEntryView {
        entry_id,
        content,
        tags: entry.tags,
        created_at: entry.created_at,
    })
}

fn parse_internal_uuid(value: &str, category: &'static str) -> Result<uuid::Uuid, PublicError> {
    uuid::Uuid::parse_str(value).map_err(|_| internal(category))
}

fn map_search_path(path: SearchPath) -> RetrievalPath {
    match path {
        SearchPath::Bm25 => RetrievalPath::Bm25,
        SearchPath::Vector => RetrievalPath::Vector,
        SearchPath::TagFallback => RetrievalPath::TagFallback,
        SearchPath::KnowledgeGraphTemporal => RetrievalPath::KnowledgeGraphTemporal,
        SearchPath::KnowledgeGraphPpr => RetrievalPath::KnowledgeGraphPpr,
        SearchPath::KnowledgeGraphPathDiscovery => RetrievalPath::KnowledgeGraphPathDiscovery,
        SearchPath::KnowledgeGraphCausal => RetrievalPath::KnowledgeGraphCausal,
        SearchPath::Reranker => RetrievalPath::Reranker,
    }
}

fn map_stage_degradation(_: SearchStageDegradation) -> RetrievalDegradation {
    RetrievalDegradation::ExecutionFailed
}

fn map_embedding_query(state: EmbeddingQueryState) -> EmbeddingQueryView {
    match state {
        EmbeddingQueryState::NotProbed { .. } => EmbeddingQueryView {
            state: EmbeddingQueryStatus::NotProbed,
            degradation: None,
        },
        EmbeddingQueryState::Succeeded { .. } => EmbeddingQueryView {
            state: EmbeddingQueryStatus::Succeeded,
            degradation: None,
        },
        EmbeddingQueryState::Degraded { reason, .. } => EmbeddingQueryView {
            state: EmbeddingQueryStatus::Degraded,
            degradation: Some(match reason {
                EmbeddingDegradation::ProviderUnavailable => EmbeddingQueryDegradation::ProviderUnavailable,
                EmbeddingDegradation::ModelUnavailable => EmbeddingQueryDegradation::ModelUnavailable,
                EmbeddingDegradation::InputRejected => EmbeddingQueryDegradation::InputRejected,
                EmbeddingDegradation::ExecutionFailed => EmbeddingQueryDegradation::ExecutionFailed,
            }),
        },
    }
}

fn map_memory_error(error: DurableMemoryMutationError) -> PublicError {
    match error {
        DurableMemoryMutationError::NotFound { .. } | DurableMemoryMutationError::NamespaceMismatch { .. } => {
            not_found("memory_entry")
        }
        DurableMemoryMutationError::Inactive { .. } => public_error(
            PublicErrorCode::Conflict,
            "memory entry is no longer active",
            false,
            "inactive",
        ),
        DurableMemoryMutationError::EmptyContent => invalid_argument("memory content must not be empty"),
        DurableMemoryMutationError::InvalidCanonicalContent { .. } => {
            invalid_argument("memory content cannot be represented canonically")
        }
        DurableMemoryMutationError::QuotaExceeded { .. } => public_error(
            PublicErrorCode::LimitExceeded,
            "memory quota exceeded",
            false,
            "quota_exceeded",
        ),
        DurableMemoryMutationError::PermissionDenied => public_error(
            PublicErrorCode::PermissionDenied,
            "operation is not permitted for this local role",
            false,
            "permission_denied",
        ),
        DurableMemoryMutationError::PersistenceUnavailable => dependency_unavailable("memory_persistence"),
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::HeadConflict { .. }) => public_error(
            PublicErrorCode::Conflict,
            "memory head changed; refresh and retry",
            false,
            "head_conflict",
        ),
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::UnsupportedPolicy {
            category: "role_not_policy_writer",
        }) => public_error(
            PublicErrorCode::PermissionDenied,
            "operation is not permitted for this local role",
            false,
            "permission_denied",
        ),
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::UnsupportedPolicy { .. }) => {
            invalid_argument("memory policy is unsupported")
        }
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::Invalid { .. }) => public_error(
            PublicErrorCode::Internal,
            "canonical memory validation failed",
            false,
            "memory_integrity",
        ),
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::CommitIndeterminate) => public_error(
            PublicErrorCode::DependencyUnavailable,
            "memory commit outcome is uncertain; restart and reconcile before writing",
            false,
            "commit_indeterminate",
        ),
        DurableMemoryMutationError::Ledger(crate::memory::LedgerError::WriterPoisoned) => public_error(
            PublicErrorCode::DependencyUnavailable,
            "memory writer requires restart and reconciliation",
            false,
            "writer_poisoned",
        ),
        DurableMemoryMutationError::Ledger(_) => dependency_unavailable("memory_persistence"),
    }
}

fn map_session_error(error: super::ops::session::SessionError) -> PublicError {
    match error {
        super::ops::session::SessionError::NotFound(_) | super::ops::session::SessionError::Ownership { .. } => {
            not_found("session")
        }
        super::ops::session::SessionError::Persistence { .. } => dependency_unavailable("session_persistence"),
    }
}

fn map_io_error(error: std::io::Error) -> PublicError {
    match error.kind() {
        std::io::ErrorKind::NotFound => not_found("object"),
        std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
            invalid_argument("invalid object request")
        }
        std::io::ErrorKind::PermissionDenied => public_error(
            PublicErrorCode::PermissionDenied,
            "access denied",
            false,
            "permission_denied",
        ),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => dependency_unavailable("canonical_storage"),
        _ => dependency_unavailable("canonical_storage"),
    }
}

fn invalid_argument(message: impl Into<String>) -> PublicError {
    public_error(PublicErrorCode::InvalidArgument, message, false, "invalid_argument")
}

fn not_found(category: &'static str) -> PublicError {
    public_error(PublicErrorCode::NotFound, "resource not found", false, category)
}

fn dependency_unavailable(category: &'static str) -> PublicError {
    public_error(
        PublicErrorCode::DependencyUnavailable,
        "required durable dependency is unavailable",
        true,
        category,
    )
}

fn internal(category: &'static str) -> PublicError {
    public_error(PublicErrorCode::Internal, "internal invariant failed", false, category)
}

fn public_error(
    code: PublicErrorCode,
    message: impl Into<String>,
    retryable: bool,
    category: &'static str,
) -> PublicError {
    PublicError {
        code,
        message: message.into(),
        retryable,
        details: Some(serde_json::json!({ "category": category })),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::fs::{EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider};

    use super::*;

    #[derive(Clone, Default)]
    struct CapturedTrace {
        events: Arc<std::sync::Mutex<String>>,
        next_span: Arc<std::sync::atomic::AtomicU64>,
    }

    struct TraceVisitor<'a>(&'a mut String);

    impl tracing::field::Visit for TraceVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, " {}={value:?}", field.name());
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            use std::fmt::Write;
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    impl tracing::Subscriber for CapturedTrace {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn register_callsite(&self, _metadata: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }

        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::TRACE)
        }

        fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            use std::fmt::Write;
            let mut events = self.events.lock().unwrap();
            let _ = write!(events, " span={}", attributes.metadata().name());
            attributes.record(&mut TraceVisitor(&mut events));
            tracing::span::Id::from_u64(self.next_span.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            let mut events = self.events.lock().unwrap();
            values.record(&mut TraceVisitor(&mut events));
        }

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            use std::fmt::Write;
            let mut events = self.events.lock().unwrap();
            let _ = write!(events, " event={}", event.metadata().name());
            event.record(&mut TraceVisitor(&mut events));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    #[derive(Debug)]
    struct FailingLedger;

    struct DriftingReadinessProvider {
        drifted: Arc<AtomicBool>,
        identity_calls: Arc<AtomicUsize>,
    }

    impl EmbeddingProvider for DriftingReadinessProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(vec![1.0, 0.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            Ok(texts.iter().map(|_| EmbedResult::new(vec![1.0, 0.0], 1)).collect())
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            self.identity_calls.fetch_add(1, Ordering::AcqRel);
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "readiness-drift",
                2,
                if self.drifted.load(Ordering::Acquire) {
                    "readiness-drift-v2"
                } else {
                    "readiness-drift-v1"
                },
            ))
        }

        fn model_name(&self) -> String {
            "readiness-drift".into()
        }
    }

    impl crate::memory::CanonicalLedger for FailingLedger {
        fn commit_expected(
            &self,
            _role_id: &str,
            _tier: MemoryTier,
            _expected_head: crate::memory::ExpectedHead,
            _revision: crate::memory::CanonicalRevision,
        ) -> Result<crate::memory::LedgerCommit, crate::memory::LedgerError> {
            Err(crate::memory::LedgerError::Cas("injected".into()))
        }

        fn commit_roots(
            &self,
            _role_id: &str,
            _tier: MemoryTier,
            _revisions: Vec<crate::memory::CanonicalRevision>,
        ) -> Result<Vec<crate::memory::LedgerCommit>, crate::memory::LedgerError> {
            Err(crate::memory::LedgerError::Cas("injected".into()))
        }

        fn rebuild_origin_role(
            &self,
            _role_id: &str,
        ) -> Result<Vec<(MemoryTier, Vec<crate::memory::CanonicalRevision>)>, crate::memory::LedgerError> {
            Ok(Vec::new())
        }

        fn list_origin_roles(&self) -> Result<Vec<String>, crate::memory::LedgerError> {
            Ok(Vec::new())
        }

        fn readable_active_revision_ids(
            &self,
            _role_id: &str,
        ) -> Result<Vec<crate::memory::MemoryRevisionId>, crate::memory::LedgerError> {
            Ok(Vec::new())
        }

        fn origin_for_revision(
            &self,
            _role_id: &str,
            _revision_id: &str,
            _write: bool,
        ) -> Result<Option<String>, crate::memory::LedgerError> {
            Ok(None)
        }

        fn flush(&self) -> Result<(), crate::memory::LedgerError> {
            Ok(())
        }
    }

    fn request(command: PublicCommand) -> PublicRequest {
        PublicRequest::new(uuid::Uuid::new_v4(), None, command)
    }

    fn context() -> PublicRequestContext {
        PublicRequestContext::local_owner(PublicTransport::Embedded)
    }

    #[test]
    fn catalog_is_the_exact_service_surface() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let response = kernel.handle_public_request(
            &context(),
            request(PublicCommand::CapabilitiesDescribe(EmptyInput::default())),
        );
        let PublicData::CapabilitiesDescribe(catalog) = response.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert_eq!(catalog.operations, PUBLIC_OPERATIONS.map(str::to_string));
    }

    #[test]
    fn runtime_readiness_is_one_coherent_cached_snapshot_after_provider_drift() {
        let directory = tempfile::tempdir().unwrap();
        let drifted = Arc::new(AtomicBool::new(false));
        let identity_calls = Arc::new(AtomicUsize::new(0));
        let kernel = crate::kernel::AIKernel::with_providers(
            directory.path().join("fresh-vault"),
            Arc::new(DriftingReadinessProvider {
                drifted: Arc::clone(&drifted),
                identity_calls: Arc::clone(&identity_calls),
            }),
            Arc::new(crate::llm::StubProvider::empty()),
        )
        .unwrap();
        drifted.store(true, Ordering::Release);
        assert!(matches!(
            kernel.projection.owner_rebuild(
                crate::PERSONAL_OWNER_ROLE_ID,
                crate::memory::projection::ProjectionRebuildSelector::AllEligible,
            ),
            Err(ProjectionRuntimeError::Unavailable("provider_changed_restart_required"))
        ));
        let calls_before = identity_calls.load(Ordering::Acquire);

        let response = kernel.handle_public_request(
            &context(),
            request(PublicCommand::RuntimeReadiness(EmptyInput::default())),
        );
        let PublicData::RuntimeReadiness(readiness) = response.data.unwrap() else {
            panic!("wrong typed response")
        };
        assert!(readiness.ready);
        assert_eq!(readiness.projection.control_plane, ComponentState::Ready);
        assert_eq!(readiness.projection.worker, ComponentState::Unavailable);
        assert_eq!(readiness.projection.control_plane_reason, None);
        assert_eq!(
            readiness.projection.worker_reason,
            Some(ProjectionUnavailableCategory::ProviderChangedRestartRequired)
        );
        assert_eq!(readiness.embedding_provider, ComponentState::Unavailable);
        assert_eq!(identity_calls.load(Ordering::Acquire), calls_before);
    }

    #[test]
    fn runtime_readiness_reports_projection_shutdown_without_provider_probe() {
        let directory = tempfile::tempdir().unwrap();
        let drifted = Arc::new(AtomicBool::new(false));
        let identity_calls = Arc::new(AtomicUsize::new(0));
        let kernel = crate::kernel::AIKernel::with_providers(
            directory.path().join("fresh-vault"),
            Arc::new(DriftingReadinessProvider {
                drifted,
                identity_calls: Arc::clone(&identity_calls),
            }),
            Arc::new(crate::llm::StubProvider::empty()),
        )
        .unwrap();
        let calls_before = identity_calls.load(Ordering::Acquire);
        kernel.shutdown_projection_worker();

        let response = kernel.handle_public_request(
            &context(),
            request(PublicCommand::RuntimeReadiness(EmptyInput::default())),
        );
        let PublicData::RuntimeReadiness(readiness) = response.data.unwrap() else {
            panic!("wrong typed response")
        };
        assert!(!readiness.ready);
        assert_eq!(readiness.projection.control_plane, ComponentState::Ready);
        assert_eq!(readiness.projection.worker, ComponentState::Unavailable);
        assert_eq!(readiness.projection.control_plane_reason, None);
        assert_eq!(
            readiness.projection.worker_reason,
            Some(ProjectionUnavailableCategory::RuntimeShuttingDown)
        );
        assert_eq!(identity_calls.load(Ordering::Acquire), calls_before);
    }

    #[test]
    fn runtime_readiness_exposes_coherent_cognitive_progress() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let (handle, _receiver) = crate::kernel::ops::cognitive_pipeline::CognitivePipelineHandle::channel_for_test(1);
        handle
            .enqueue_sync(crate::kernel::ops::cognitive_pipeline::CognitiveTask::LinkSimilarity {
                cid: "progress-cid".to_string(),
                agent_id: "kernel".to_string(),
            })
            .unwrap();
        *kernel.cognitive_pipeline.write().unwrap() = Some(handle);

        let response = kernel.handle_public_request(
            &context(),
            request(PublicCommand::RuntimeReadiness(EmptyInput::default())),
        );
        let PublicData::RuntimeReadiness(readiness) = response.data.unwrap() else {
            panic!("wrong typed response")
        };

        assert_eq!(readiness.cognitive_worker, ComponentState::Ready);
        assert_eq!(
            readiness.cognitive_progress,
            Some(CognitivePipelineProgressView {
                accepted: 1,
                completed: 0,
                in_flight: 1,
            })
        );
    }

    #[test]
    fn memory_create_read_update_delete_is_one_durable_flow() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let created = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "canonical fact".into(),
                tags: vec!["fact".into()],
            })),
        );
        let PublicData::MemoryCreate(created) = created.data.unwrap() else {
            panic!("wrong typed response");
        };
        let original_id = created.entry.entry_id;

        let read = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: original_id })),
        );
        assert_eq!(read.data.unwrap().operation(), "memory.get");

        let updated = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                entry_id: original_id,
                content: "corrected fact".into(),
            })),
        );
        let PublicData::MemoryUpdate(updated) = updated.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert_ne!(updated.entry.entry_id, original_id);

        let deleted = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryDelete(MemoryEntryInput {
                entry_id: updated.entry.entry_id,
            })),
        );
        assert!(deleted.ok);
        let missing = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryGet(MemoryEntryInput {
                entry_id: updated.entry.entry_id,
            })),
        );
        assert_eq!(missing.error.unwrap().code, PublicErrorCode::NotFound);
    }

    #[test]
    fn public_memory_policy_matrix_survives_restart() {
        let (kernel, directory) = crate::kernel::tests::make_kernel();
        let role_a = "role-a";
        let role_b = "role-b";
        let role_a_context = PublicRequestContext::authenticated_role(role_a.into(), PublicTransport::Tcp);
        let role_b_context = PublicRequestContext::authenticated_role(role_b.into(), PublicTransport::Tcp);
        kernel.permission_grant(role_b, PermissionAction::Delete, None, None);

        let created = kernel.handle_public_request(
            &role_a_context,
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "origin private sentinel".into(),
                tags: vec!["origin-private".into()],
            })),
        );
        let PublicData::MemoryCreate(created) = created.data.unwrap() else {
            panic!("wrong typed response");
        };
        let original_id = created.entry.entry_id;

        for command in [
            PublicCommand::MemoryGet(MemoryEntryInput { entry_id: original_id }),
            PublicCommand::ProjectionStatus(ProjectionStatusInput {
                kind: ProjectionKindInput::MemoryEmbedding,
                revision_id: original_id,
            }),
            PublicCommand::MemoryUpdate(MemoryUpdateInput {
                entry_id: original_id,
                content: "unauthorized rewrite".into(),
            }),
            PublicCommand::MemoryDelete(MemoryEntryInput { entry_id: original_id }),
        ] {
            let denied = kernel.handle_public_request(&role_b_context, request(command));
            assert_eq!(denied.error.unwrap().code, PublicErrorCode::NotFound);
        }
        let hidden = kernel.handle_public_request(
            &role_b_context,
            request(PublicCommand::MemoryRecall(MemoryRecallInput {
                query: "origin private sentinel".into(),
                limit: 10,
            })),
        );
        let PublicData::MemoryRecall(hidden) = hidden.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert!(hidden.hits.is_empty());

        assert!(
            kernel
                .handle_public_request(
                    &context(),
                    request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: original_id })),
                )
                .ok
        );
        assert!(
            kernel
                .handle_public_request(
                    &context(),
                    request(PublicCommand::ProjectionStatus(ProjectionStatusInput {
                        kind: ProjectionKindInput::MemoryEmbedding,
                        revision_id: original_id,
                    })),
                )
                .ok
        );
        let owner_recall = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryRecall(MemoryRecallInput {
                query: "origin private sentinel".into(),
                limit: 10,
            })),
        );
        let PublicData::MemoryRecall(owner_recall) = owner_recall.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert_eq!(owner_recall.hits.len(), 1);

        let updated = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                entry_id: original_id,
                content: "owner corrected sentinel".into(),
            })),
        );
        let PublicData::MemoryUpdate(updated) = updated.data.unwrap() else {
            panic!("wrong typed response");
        };
        let updated_id = updated.entry.entry_id;
        assert_eq!(updated.entry.content, "owner corrected sentinel");
        assert_eq!(
            kernel
                .memory
                .find_entry(role_a, &updated_id.to_string())
                .unwrap()
                .agent_id,
            role_a
        );

        drop(kernel);
        let kernel = crate::kernel::AIKernel::new(directory.path().to_path_buf()).expect("restart kernel");
        let owner_read = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: updated_id })),
        );
        let PublicData::MemoryGet(owner_read) = owner_read.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert_eq!(owner_read.content, "owner corrected sentinel");
        let role_b_read = kernel.handle_public_request(
            &role_b_context,
            request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: updated_id })),
        );
        assert_eq!(role_b_read.error.unwrap().code, PublicErrorCode::NotFound);

        let first_delete = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryDelete(MemoryEntryInput { entry_id: updated_id })),
        );
        assert!(first_delete.ok);
        let repeated_delete = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryDelete(MemoryEntryInput { entry_id: updated_id })),
        );
        assert!(repeated_delete.ok);
        assert_eq!(first_delete.data, repeated_delete.data);

        drop(kernel);
        let kernel = crate::kernel::AIKernel::new(directory.path().to_path_buf()).expect("second restart kernel");
        let missing = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryGet(MemoryEntryInput { entry_id: updated_id })),
        );
        assert_eq!(missing.error.unwrap().code, PublicErrorCode::NotFound);
    }

    #[test]
    fn concurrent_public_updates_return_one_head_conflict() {
        let (kernel, directory) = crate::kernel::tests::make_kernel();
        let created = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "concurrent base".into(),
                tags: vec![],
            })),
        );
        let PublicData::MemoryCreate(created) = created.data.unwrap() else {
            panic!("wrong typed response");
        };
        let original_id = created.entry.entry_id;
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for suffix in ["one", "two"] {
            let kernel = kernel.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                kernel.handle_public_request(
                    &context(),
                    request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                        entry_id: original_id,
                        content: format!("concurrent winner {suffix}"),
                    })),
                )
            }));
        }
        barrier.wait();
        let responses: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("update thread"))
            .collect();
        assert_eq!(responses.iter().filter(|response| response.ok).count(), 1);
        let loser = responses
            .iter()
            .find_map(|response| response.error.as_ref())
            .expect("one update must conflict");
        assert_eq!(loser.code, PublicErrorCode::Conflict);
        assert_eq!(loser.details.as_ref().unwrap()["category"], "head_conflict");

        drop(kernel);
        let kernel = crate::kernel::AIKernel::new(directory.path().to_path_buf()).expect("restart kernel");
        let recalled = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryRecall(MemoryRecallInput {
                query: "concurrent winner".into(),
                limit: 10,
            })),
        );
        let PublicData::MemoryRecall(recalled) = recalled.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert_eq!(recalled.hits.len(), 1);
    }

    #[test]
    fn canonical_memory_traces_keep_private_values_out_of_success_and_failure_paths() {
        const CHILD_ENV: &str = "PLICO_CANONICAL_TRACE_CANARY_CHILD";
        if std::env::var_os(CHILD_ENV).is_none() {
            let status = std::process::Command::new(std::env::current_exe().expect("current test executable"))
                .env(CHILD_ENV, "1")
                .args([
                    "--exact",
                    "kernel::public_service::tests::canonical_memory_traces_keep_private_values_out_of_success_and_failure_paths",
                    "--nocapture",
                ])
                .status()
                .expect("spawn isolated trace canary");
            assert!(status.success(), "isolated trace canary failed");
            return;
        }

        const CONTENT: &str = "TRACE_CONTENT_CANARY_7d5e";
        const TAG: &str = "TRACE_TAG_CANARY_b843";
        const QUERY: &str = "TRACE_QUERY_CANARY_c29a";
        const ROLE: &str = "TRACE_ROLE_CANARY_41f6";
        const BEARER: &str = "TRACE_BEARER_CANARY_2ac9";

        let _trace_guard = crate::TRACE_CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let captured = CapturedTrace::default();
        let (canonical_hash, vault_path) = tracing::subscriber::with_default(captured.clone(), || {
            tracing::callsite::rebuild_interest_cache();
            let (kernel, directory) = crate::kernel::tests::make_kernel();
            let role_context = PublicRequestContext::authenticated_role(ROLE.into(), PublicTransport::Tcp);
            let created = kernel.handle_public_request(
                &role_context,
                request(PublicCommand::MemoryCreate(MemoryCreateInput {
                    content: CONTENT.into(),
                    tags: vec![TAG.into()],
                })),
            );
            let PublicData::MemoryCreate(created) = created.data.unwrap() else {
                panic!("wrong typed response");
            };
            let original_id = created.entry.entry_id;
            let canonical_hash = kernel
                .memory
                .find_entry(ROLE, &original_id.to_string())
                .unwrap()
                .canonical_content_hash
                .as_str()
                .to_string();

            let _ = kernel.handle_public_request(
                &role_context,
                request(PublicCommand::MemoryRecall(MemoryRecallInput {
                    query: QUERY.into(),
                    limit: 10,
                })),
            );
            let first_update = kernel.handle_public_request(
                &role_context,
                request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                    entry_id: original_id,
                    content: "safe correction".into(),
                })),
            );
            assert!(first_update.ok);
            let stale_update = kernel.handle_public_request(
                &role_context,
                request(PublicCommand::MemoryUpdate(MemoryUpdateInput {
                    entry_id: original_id,
                    content: "stale correction".into(),
                })),
            );
            assert_eq!(stale_update.error.unwrap().code, PublicErrorCode::Conflict);

            let vault_path = directory.path().display().to_string();
            drop(kernel);
            let restarted = crate::kernel::AIKernel::new(directory.path().to_path_buf()).expect("restart kernel");
            restarted.memory.set_ledger(Arc::new(FailingLedger));
            let failed = restarted.handle_public_request(
                &context(),
                request(PublicCommand::MemoryCreate(MemoryCreateInput {
                    content: "forced failure".into(),
                    tags: vec![],
                })),
            );
            assert_eq!(failed.error.unwrap().code, PublicErrorCode::DependencyUnavailable);

            let mut bearer_request = request(PublicCommand::CapabilitiesDescribe(EmptyInput::default()));
            bearer_request.auth = Some(PublicAuth { bearer: BEARER.into() });
            let rejected = restarted.handle_public_request(&context(), bearer_request);
            assert_eq!(rejected.error.unwrap().code, PublicErrorCode::InvalidArgument);
            (canonical_hash, vault_path)
        });

        let logs = captured.events.lock().unwrap().clone();
        for private_value in [CONTENT, TAG, QUERY, ROLE, BEARER, &canonical_hash, &vault_path] {
            assert!(!logs.contains(private_value), "private trace value leaked");
        }
        for required_field in [
            "operation=\"memory.create\"",
            "phase=",
            "outcome=\"success\"",
            "error_category=\"head_conflict\"",
            "error_category=\"ledger_cas\"",
            "error_category=\"memory_persistence\"",
        ] {
            assert!(
                logs.contains(required_field),
                "missing trace field: {required_field}\n{logs}"
            );
        }
    }

    #[test]
    fn object_search_reports_actual_execution_metadata() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let put = kernel.handle_public_request(
            &context(),
            request(PublicCommand::ObjectPut(ObjectPutInput {
                content: "searchable canonical object".into(),
                encoding: ObjectEncoding::Utf8,
                tags: vec!["searchable".into()],
            })),
        );
        assert!(put.ok);
        let searched = kernel.handle_public_request(
            &context(),
            request(PublicCommand::ObjectSearch(ObjectSearchInput {
                query: "searchable".into(),
                limit: 10,
                require_tags: vec![],
                exclude_tags: vec![],
            })),
        );
        let PublicData::ObjectSearch(result) = searched.data.unwrap() else {
            panic!("wrong typed response");
        };
        assert!(!result.retrieval.is_empty());
        assert!(!result.hits.is_empty());
    }

    #[test]
    fn memory_create_persistence_failure_is_typed_and_not_published() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        kernel.memory.set_ledger(Arc::new(FailingLedger));
        let response = kernel.handle_public_request(
            &context(),
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "must not publish".into(),
                tags: vec![],
            })),
        );
        let error = response.error.unwrap();
        assert_eq!(error.code, PublicErrorCode::DependencyUnavailable);
        assert!(error.retryable);
        assert!(kernel.memory.get_all(PERSONAL_OWNER_ROLE_ID).is_empty());
    }

    #[test]
    fn indeterminate_and_poisoned_writes_are_not_retryable() {
        for (ledger_error, category) in [
            (crate::memory::LedgerError::CommitIndeterminate, "commit_indeterminate"),
            (crate::memory::LedgerError::WriterPoisoned, "writer_poisoned"),
        ] {
            let error = map_memory_error(DurableMemoryMutationError::Ledger(ledger_error));
            assert_eq!(error.code, PublicErrorCode::DependencyUnavailable);
            assert!(!error.retryable);
            assert_eq!(error.details.as_ref().unwrap()["category"], category);
        }
    }

    #[test]
    fn authenticated_role_delete_requires_explicit_capability() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let role = "bounded-role";
        let role_context = PublicRequestContext::authenticated_role(role.into(), PublicTransport::Tcp);
        let created = kernel.handle_public_request(
            &role_context,
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "role-owned fact".into(),
                tags: vec![],
            })),
        );
        let PublicData::MemoryCreate(created) = created.data.unwrap() else {
            panic!("wrong typed response");
        };
        let denied = kernel.handle_public_request(
            &role_context,
            request(PublicCommand::MemoryDelete(MemoryEntryInput {
                entry_id: created.entry.entry_id,
            })),
        );
        assert_eq!(denied.error.unwrap().code, PublicErrorCode::PermissionDenied);
        assert!(kernel
            .memory
            .find_entry(role, &created.entry.entry_id.to_string())
            .unwrap()
            .deleted_at
            .is_none());
    }

    #[test]
    fn public_bearer_authentication_returns_only_the_local_role() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let role_id = kernel.register_agent("authenticated-role".into()).unwrap();
        let token = kernel.key_store.generate_token(&role_id);
        assert_eq!(kernel.authenticate_public_bearer(&token.token).unwrap(), role_id);
        assert_eq!(
            kernel.authenticate_public_bearer("invalid").unwrap_err(),
            PublicAuthenticationError
        );
    }

    #[test]
    fn bootstrapped_owner_bearer_resolves_to_owner_capabilities() {
        let (kernel, directory) = crate::kernel::tests::make_kernel();
        let credential_path = kernel.ensure_personal_owner_credential().unwrap();
        assert_eq!(
            credential_path,
            crate::api::AgentKeyStore::credential_path(directory.path())
        );
        let credentials: std::collections::HashMap<String, crate::api::AgentToken> =
            serde_json::from_slice(&std::fs::read(credential_path).unwrap()).unwrap();
        let bearer = &credentials[PERSONAL_OWNER_ROLE_ID].token;
        let role = kernel.authenticate_public_bearer(bearer).unwrap();
        let owner = PublicRequestContext::authenticated_role(role, PublicTransport::Tcp);
        assert_eq!(owner.access, PublicAccess::LocalOwner);

        let created = kernel.handle_public_request(
            &owner,
            request(PublicCommand::MemoryCreate(MemoryCreateInput {
                content: "owner-correctable fact".into(),
                tags: vec![],
            })),
        );
        let PublicData::MemoryCreate(created) = created.data.unwrap() else {
            panic!("wrong typed response");
        };
        let deleted = kernel.handle_public_request(
            &owner,
            request(PublicCommand::MemoryDelete(MemoryEntryInput {
                entry_id: created.entry.entry_id,
            })),
        );
        assert!(deleted.ok);
    }

    #[test]
    fn non_tcp_transports_reject_payload_bearers() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let mut request = request(PublicCommand::CapabilitiesDescribe(EmptyInput::default()));
        request.auth = Some(PublicAuth {
            bearer: "must-not-be-consumed".into(),
        });
        let response = kernel.handle_public_request(&context(), request);
        assert_eq!(response.error.unwrap().code, PublicErrorCode::InvalidArgument);
    }

    #[test]
    fn session_start_and_end_use_durable_primitives() {
        let (kernel, _directory) = crate::kernel::tests::make_kernel();
        let started = kernel.handle_public_request(
            &context(),
            request(PublicCommand::SessionStart(SessionStartInput::default())),
        );
        let PublicData::SessionStart(started) = started.data.unwrap() else {
            panic!("wrong typed response");
        };
        let ended = kernel.handle_public_request(
            &context(),
            request(PublicCommand::SessionEnd(SessionEndInput {
                session_id: started.session_id,
            })),
        );
        assert!(ended.ok);
    }
}
