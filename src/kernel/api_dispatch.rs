//! API request dispatch — thin routing table.
//!
//! Each match arm delegates to a domain-specific handler in `handlers/`.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::AgentId;
use super::ops;
use super::ops::observability::{OperationTimer, OpType};

impl super::AIKernel {
    pub fn handle_api_request(&self, req: ApiRequest) -> ApiResponse {
        let correlation_id = ops::observability::CorrelationId::new();
        let _timer = OperationTimer::new(&self.metrics, OpType::HandleApiRequest);
        let span = tracing::info_span!(
            "handle_api_request",
            operation = "handle_api_request",
            correlation_id = %correlation_id,
        );
        let _guard = span.enter();
        let _corr_id = correlation_id;

        // --- UNIFIED SECURITY GUARDRAIL (Soul 3.0 Red Lines) ---
        if let Err(e) = self.validate_security(&req) {
            tracing::warn!("Security validation failed: {}", e);
            return ApiResponse::error(e);
        }

        let (request_agent_id, _, _) = self.extract_security_info(&req);
        let skip_auto_register = matches!(&req,
            ApiRequest::AgentStatus { .. } |
            ApiRequest::AgentSuspend { .. } |
            ApiRequest::AgentResume { .. } |
            ApiRequest::AgentTerminate { .. } |
            ApiRequest::AgentComplete { .. } |
            ApiRequest::AgentFail { .. } |
            ApiRequest::AgentCheckpoint { .. } |
            ApiRequest::AgentRestore { .. } |
            ApiRequest::AgentUsage { .. } |
            ApiRequest::AgentSetResources { .. }
        );
        if let Some(ref aid) = request_agent_id {
            if !skip_auto_register {
                self.ensure_agent_registered(aid);
            }
            self.scheduler.record_tool_call(&AgentId(aid.clone()));
        }

        let mut response = match req {
            // ── CAS ──
            req @ (ApiRequest::Create { .. } | ApiRequest::Read { .. } | ApiRequest::Search { .. } |
                   ApiRequest::Update { .. } | ApiRequest::Delete { .. } |
                   ApiRequest::ListDeleted { .. } | ApiRequest::Restore { .. } |
                   ApiRequest::History { .. } | ApiRequest::Rollback { .. } |
                   ApiRequest::BatchCreate { .. }) => self.handle_cas(req),

            // ── Memory ──
            req @ (ApiRequest::Remember { .. } | ApiRequest::Recall { .. } |
                   ApiRequest::RememberLongTerm { .. } | ApiRequest::RecallSemantic { .. } |
                   ApiRequest::RecallRouted { .. } |
                   ApiRequest::RememberProcedural { .. } | ApiRequest::RecallProcedural { .. } |
                   ApiRequest::RecallVisible { .. } | ApiRequest::MemoryMove { .. } |
                   ApiRequest::MemoryDeleteEntry { .. } | ApiRequest::EvictExpired { .. } |
                   ApiRequest::LoadContext { .. } | ApiRequest::BatchMemoryStore { .. } |
                   ApiRequest::MemoryStats { .. } | ApiRequest::DiscoverKnowledge { .. } |
                   ApiRequest::RememberLongTermBatch { .. }) => self.handle_memory(req),

            // ── Agent ──
            req @ (ApiRequest::RegisterAgent { .. } | ApiRequest::ListAgents |
                   ApiRequest::AgentStatus { .. } | ApiRequest::AgentSuspend { .. } |
                   ApiRequest::AgentResume { .. } | ApiRequest::AgentTerminate { .. } |
                   ApiRequest::AgentComplete { .. } | ApiRequest::AgentFail { .. } |
                   ApiRequest::AgentSetResources { .. } | ApiRequest::AgentCheckpoint { .. } |
                   ApiRequest::AgentRestore { .. } | ApiRequest::AgentUsage { .. }) => self.handle_agent(req),

            // ── Graph ──
            req @ (ApiRequest::Explore { .. } | ApiRequest::AddNode { .. } | ApiRequest::AddEdge { .. } |
                   ApiRequest::ListNodes { .. } | ApiRequest::ListNodesAtTime { .. } |
                   ApiRequest::FindPaths { .. } | ApiRequest::GetNode { .. } |
                   ApiRequest::ListEdges { .. } | ApiRequest::RemoveNode { .. } |
                   ApiRequest::RemoveEdge { .. } | ApiRequest::UpdateNode { .. } |
                   ApiRequest::EdgeHistory { .. } | ApiRequest::KGCausalPath { .. } |
                   ApiRequest::KGImpactAnalysis { .. } | ApiRequest::KGTemporalChanges { .. }) => self.handle_graph(req),

            // ── Intent ──
            req @ (ApiRequest::SubmitIntent { .. } | ApiRequest::ContextAssemble { .. } |
                   ApiRequest::DeclareIntent { .. } | ApiRequest::FetchAssembledContext { .. } |
                   ApiRequest::IntentFeedback { .. } | ApiRequest::BatchSubmitIntent { .. } |
                   ApiRequest::BatchQuery { .. }) => self.handle_intent(req),

            // ── Events ──
            req @ (ApiRequest::CreateEvent { .. } | ApiRequest::ListEvents { .. } |
                   ApiRequest::ListEventsText { .. } | ApiRequest::EventAttach { .. } |
                   ApiRequest::EventSubscribe { .. } | ApiRequest::EventPoll { .. } |
                   ApiRequest::EventUnsubscribe { .. } | ApiRequest::EventHistory { .. } |
                   ApiRequest::DeltaSince { .. }) => self.handle_events(req),

            // ── Session ──
            req @ (ApiRequest::StartSession { .. } | ApiRequest::EndSession { .. } |
                   ApiRequest::RegisterSkill { .. } | ApiRequest::DiscoverSkills { .. }) => self.handle_session(req),

            // ── System ──
            req @ (ApiRequest::SystemStatus | ApiRequest::CacheStats | ApiRequest::CacheInvalidate |
                   ApiRequest::IntentCacheStats | ApiRequest::ClusterStatus |
                   ApiRequest::ClusterJoin { .. } | ApiRequest::ClusterLeave |
                   ApiRequest::NodePing { .. } | ApiRequest::QueryTokenUsage { .. } |
                   ApiRequest::HealthReport | ApiRequest::CostSessionSummary { .. } |
                   ApiRequest::CostAgentTrend { .. } | ApiRequest::CostAnomalyCheck { .. } |
                   ApiRequest::QueryGrowthReport { .. }) => self.handle_system(req),

            // ── Tools ──
            req @ (ApiRequest::ToolCall { .. } | ApiRequest::ToolList { .. } |
                   ApiRequest::ToolDescribe { .. } | ApiRequest::HookList |
                   ApiRequest::HookRegister { .. }) => self.handle_tools(req),

            // ── Messaging ──
            req @ (ApiRequest::SendMessage { .. } | ApiRequest::ReadMessages { .. } |
                   ApiRequest::AckMessage { .. } | ApiRequest::DiscoverAgents { .. } |
                   ApiRequest::DelegateTask { .. } | ApiRequest::QueryTaskStatus { .. } |
                   ApiRequest::TaskStart { .. } | ApiRequest::TaskComplete { .. } |
                   ApiRequest::TaskFail { .. }) => self.handle_messaging(req),

            // ── Permission ──
            req @ (ApiRequest::GrantPermission { .. } | ApiRequest::RevokePermission { .. } |
                   ApiRequest::ListPermissions { .. } | ApiRequest::CheckPermission { .. }) => self.handle_permission(req),

            // ── Tenant ──
            req @ (ApiRequest::CreateTenant { .. } | ApiRequest::ListTenants { .. } |
                   ApiRequest::TenantShare { .. }) => self.handle_tenant(req),

            // ── Model ──
            req @ (ApiRequest::SwitchEmbeddingModel { .. } | ApiRequest::SwitchLlmModel { .. } |
                   ApiRequest::CheckModelHealth { .. }) => self.handle_model(req),

            // ── Storage ──
            req @ (ApiRequest::HybridRetrieve { .. } | ApiRequest::ObjectUsage { .. } |
                   ApiRequest::StorageStats { .. } | ApiRequest::EvictCold { .. }) => self.handle_storage(req),

            // ── Prompt ──
            req @ (ApiRequest::ListPrompts | ApiRequest::GetPromptInfo { .. } |
                   ApiRequest::SetPromptOverride { .. } | ApiRequest::RemovePromptOverride { .. }) => self.handle_prompt(req),

            // ── File Import (v33) ──
            ApiRequest::ImportFiles { .. } => self.handle_import(req),

            // ── Plico Core Verbs (v1.0) ──
            ApiRequest::CoreGet { .. } |
            ApiRequest::CoreList { .. } |
            ApiRequest::CoreSearch { .. } |
            ApiRequest::CoreCreate { .. } |
            ApiRequest::CoreUpdate { .. } |
            ApiRequest::CoreDelete { .. } |
            ApiRequest::CoreExec { .. } |
            ApiRequest::CoreObserve { .. } |
            ApiRequest::CoreLink { .. } |
            ApiRequest::CoreAsk { .. } |
            ApiRequest::CoreState { .. } => self.handle_core_ops(req),
            };


        self.maybe_persist_event_log();
        let json = serde_json::to_string(&response).unwrap_or_default();
        let token_est = crate::api::semantic::estimate_tokens(&json);
        if let Some(ref aid) = request_agent_id {
            self.scheduler.record_token_usage(&AgentId(aid.clone()), token_est as u64);
            self.persist_usage();
        }
        response.token_estimate = Some(token_est);
        response
    }
}
