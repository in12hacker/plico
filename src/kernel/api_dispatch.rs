//! API request dispatch — thin routing table.
//!
//! Each match arm delegates to a domain-specific handler in `handlers/`.

use super::ops;
use super::ops::observability::{OpType, OperationTimer};
use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::AgentId;

impl super::AIKernel {
    pub fn handle_api_request(&self, req: ApiRequest) -> ApiResponse {
        self.handle_api_request_with_origin(req, false)
    }

    /// Dispatch a request produced by kernel-owned code.
    ///
    /// This is crate-private so transport adapters and library consumers cannot
    /// turn a self-reported `kernel`/`system` name into a trusted request.
    pub(crate) fn handle_internal_api_request(&self, req: ApiRequest) -> ApiResponse {
        self.handle_api_request_with_origin(req, true)
    }

    fn handle_api_request_with_origin(&self, req: ApiRequest, trusted_internal_origin: bool) -> ApiResponse {
        let correlation_id = ops::observability::CorrelationId::new();
        let _timer = OperationTimer::new(&self.metrics, OpType::HandleApiRequest);
        let span = tracing::info_span!(
            "handle_api_request",
            operation = "handle_api_request",
            correlation_id = %correlation_id,
        );
        let _guard = span.enter();
        let _corr_id = correlation_id;

        // Trace: record start time and extract tool name
        let trace_start = std::time::Instant::now();
        let trace_tool_name = Self::api_request_tool_name(&req);

        // --- UNIFIED SECURITY GUARDRAIL (Soul 3.0 Red Lines) ---
        if let Err(e) = self.validate_security_for_origin(&req, trusted_internal_origin) {
            tracing::warn!("Security validation failed: {}", e);
            return ApiResponse::error(e);
        }

        let (request_agent_id, _, _) = self.extract_security_info(&req);
        let skip_auto_register = matches!(
            &req,
            ApiRequest::AgentStatus { .. }
                | ApiRequest::AgentSuspend { .. }
                | ApiRequest::AgentResume { .. }
                | ApiRequest::AgentTerminate { .. }
                | ApiRequest::AgentComplete { .. }
                | ApiRequest::AgentFail { .. }
                | ApiRequest::AgentCheckpoint { .. }
                | ApiRequest::AgentUsage { .. }
                | ApiRequest::AgentSetResources { .. }
        );
        if let Some(ref aid) = request_agent_id {
            if !skip_auto_register {
                self.ensure_agent_registered(aid);
            }
            self.scheduler.record_tool_call(&AgentId(aid.clone()));
        }

        let mut response = match req {
            // ── CAS ──
            req @ (ApiRequest::Create { .. }
            | ApiRequest::Read { .. }
            | ApiRequest::Search { .. }
            | ApiRequest::Update { .. }
            | ApiRequest::Delete { .. }
            | ApiRequest::ListDeleted { .. }
            | ApiRequest::Restore { .. }
            | ApiRequest::History { .. }
            | ApiRequest::Rollback { .. }
            | ApiRequest::BatchCreate { .. }) => self.handle_cas(req),

            // ── Memory ──
            req @ (ApiRequest::Remember { .. }
            | ApiRequest::Recall { .. }
            | ApiRequest::RememberLongTerm { .. }
            | ApiRequest::RememberProcedural { .. }
            | ApiRequest::RecallProcedural { .. }
            | ApiRequest::MemoryDeleteEntry { .. }
            | ApiRequest::LoadContext { .. }
            | ApiRequest::BatchMemoryStore { .. }
            | ApiRequest::MemoryStats { .. }
            | ApiRequest::RememberLongTermBatch { .. }) => self.handle_memory(req),

            // ── Agent ──
            req @ (ApiRequest::RegisterAgent { .. }
            | ApiRequest::ListAgents
            | ApiRequest::AgentStatus { .. }
            | ApiRequest::AgentSuspend { .. }
            | ApiRequest::AgentResume { .. }
            | ApiRequest::AgentTerminate { .. }
            | ApiRequest::AgentComplete { .. }
            | ApiRequest::AgentFail { .. }
            | ApiRequest::AgentSetResources { .. }
            | ApiRequest::AgentCheckpoint { .. }
            | ApiRequest::AgentUsage { .. }) => self.handle_agent(req),

            // ── Graph ──
            req @ (ApiRequest::Explore { .. }
            | ApiRequest::AddNode { .. }
            | ApiRequest::AddEdge { .. }
            | ApiRequest::ListNodes { .. }
            | ApiRequest::ListNodesAtTime { .. }
            | ApiRequest::FindPaths { .. }
            | ApiRequest::GetNode { .. }
            | ApiRequest::ListEdges { .. }
            | ApiRequest::RemoveNode { .. }
            | ApiRequest::RemoveEdge { .. }
            | ApiRequest::UpdateNode { .. }
            | ApiRequest::EdgeHistory { .. }
            | ApiRequest::KGCausalPath { .. }
            | ApiRequest::KGImpactAnalysis { .. }
            | ApiRequest::KGTemporalChanges { .. }) => self.handle_graph(req),

            // ── Intent ──
            req @ (ApiRequest::SubmitIntent { .. }
            | ApiRequest::ContextAssemble { .. }
            | ApiRequest::DeclareIntent { .. }
            | ApiRequest::FetchAssembledContext { .. }
            | ApiRequest::IntentFeedback { .. }
            | ApiRequest::BatchSubmitIntent { .. }
            | ApiRequest::BatchQuery { .. }) => self.handle_intent(req),

            // ── Events ──
            req @ (ApiRequest::CreateEvent { .. }
            | ApiRequest::ListEvents { .. }
            | ApiRequest::ListEventsText { .. }
            | ApiRequest::EventAttach { .. }
            | ApiRequest::EventSubscribe { .. }
            | ApiRequest::EventPoll { .. }
            | ApiRequest::EventUnsubscribe { .. }
            | ApiRequest::EventHistory { .. }
            | ApiRequest::DeltaSince { .. }) => self.handle_events(req),

            // ── Session ──
            req @ (ApiRequest::StartSession { .. }
            | ApiRequest::EndSession { .. }
            | ApiRequest::RegisterSkill { .. }
            | ApiRequest::DiscoverSkills { .. }) => self.handle_session(req),

            // ── System ──
            req @ (ApiRequest::SystemStatus
            | ApiRequest::CacheStats
            | ApiRequest::CacheInvalidate
            | ApiRequest::IntentCacheStats
            | ApiRequest::QueryTokenUsage { .. }
            | ApiRequest::HealthReport
            | ApiRequest::CostSessionSummary { .. }
            | ApiRequest::CostAgentTrend { .. }
            | ApiRequest::CostAnomalyCheck { .. }
            | ApiRequest::QueryGrowthReport { .. }) => self.handle_system(req),

            // ── Tools ──
            req @ (ApiRequest::ToolCall { .. }
            | ApiRequest::ToolList { .. }
            | ApiRequest::ToolDescribe { .. }
            | ApiRequest::HookList
            | ApiRequest::HookRegister { .. }) => self.handle_tools(req),

            // ── Messaging ──
            req @ (ApiRequest::SendMessage { .. }
            | ApiRequest::ReadMessages { .. }
            | ApiRequest::AckMessage { .. }
            | ApiRequest::DiscoverAgents { .. }
            | ApiRequest::DelegateTask { .. }
            | ApiRequest::QueryTaskStatus { .. }
            | ApiRequest::TaskStart { .. }
            | ApiRequest::TaskComplete { .. }
            | ApiRequest::TaskFail { .. }) => self.handle_messaging(req),

            // ── Permission ──
            req @ (ApiRequest::GrantPermission { .. }
            | ApiRequest::RevokePermission { .. }
            | ApiRequest::ListPermissions { .. }
            | ApiRequest::CheckPermission { .. }) => self.handle_permission(req),

            // ── Model ──
            req @ (ApiRequest::SwitchLlmModel { .. } | ApiRequest::CheckModelHealth { .. }) => self.handle_model(req),

            // ── Trace (v52) ──
            req @ (ApiRequest::TraceList { .. } | ApiRequest::TraceShow { .. } | ApiRequest::TraceFailures { .. }) => {
                self.handle_trace(req)
            }

            // ── Storage ──
            req @ (ApiRequest::ObjectUsage { .. } | ApiRequest::StorageStats { .. }) => self.handle_storage(req),

            // ── Prompt ──
            req @ (ApiRequest::ListPrompts
            | ApiRequest::GetPromptInfo { .. }
            | ApiRequest::SetPromptOverride { .. }
            | ApiRequest::RemovePromptOverride { .. }) => self.handle_prompt(req),

            // ── File Import (v33) ──
            ApiRequest::ImportFiles { .. } => self.handle_import(req),

            // ── Plico Core Verbs (v1.0) ──
            ApiRequest::CoreGet { .. }
            | ApiRequest::CoreList { .. }
            | ApiRequest::CoreSearch { .. }
            | ApiRequest::CoreCreate { .. }
            | ApiRequest::CoreUpdate { .. }
            | ApiRequest::CoreDelete { .. }
            | ApiRequest::CoreExec { .. }
            | ApiRequest::CoreObserve { .. }
            | ApiRequest::CoreLink { .. }
            | ApiRequest::CoreAsk { .. }
            | ApiRequest::CoreState { .. } => self.handle_core_ops(req),
        };

        self.maybe_persist_event_log();
        let json = serde_json::to_string(&response).unwrap_or_default();
        let token_est = crate::api::semantic::estimate_tokens(&json);
        if let Some(ref aid) = request_agent_id {
            self.scheduler
                .record_token_usage(&AgentId(aid.clone()), token_est as u64);
            self.persist_usage();
        }
        response.token_estimate = Some(token_est);

        // Trace: record span asynchronously
        if let Some(tool_name) = trace_tool_name {
            let latency_ms = trace_start.elapsed().as_millis() as u64;
            let agent_id = request_agent_id.clone().unwrap_or_else(|| "unknown".into());
            let status = if response.ok {
                super::trace::SpanStatus::Success
            } else {
                super::trace::SpanStatus::Error
            };
            let span = super::trace::Span {
                trace_id: super::trace::new_id(),
                parent_id: None,
                span_id: super::trace::new_id(),
                agent_id,
                tool_name,
                input: serde_json::json!({}),
                output: serde_json::json!({"ok": response.ok}),
                status,
                latency_ms,
                timestamp: super::trace::today_str(),
                session_id: None,
                intent_id: None,
            };
            self.trace_writer.record(span);
        }

        response
    }

    fn api_request_tool_name(req: &ApiRequest) -> Option<String> {
        let name = match req {
            ApiRequest::Create { .. } => "create",
            ApiRequest::Read { .. } => "read",
            ApiRequest::Search { .. } => "search",
            ApiRequest::Update { .. } => "update",
            ApiRequest::Delete { .. } => "delete",
            ApiRequest::Remember { .. } => "remember",
            ApiRequest::Recall { .. } => "recall",
            ApiRequest::RememberLongTerm { .. } => "remember_long_term",
            ApiRequest::SubmitIntent { .. } => "submit_intent",
            ApiRequest::ContextAssemble { .. } => "context_assemble",
            ApiRequest::StartSession { .. } => "start_session",
            ApiRequest::EndSession { .. } => "end_session",
            ApiRequest::RegisterAgent { .. } => "register_agent",
            ApiRequest::SendMessage { .. } => "send_message",
            ApiRequest::ReadMessages { .. } => "read_messages",
            _ => return None,
        };
        Some(name.into())
    }
}
