//! API request dispatch — thin routing table.
//!
//! Each match arm delegates to a domain-specific handler in `handlers/`.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::AgentId;
use super::ops;
use super::ops::observability::{OperationTimer, OpType};

impl super::AIKernel {
    fn extract_agent_id(req: &ApiRequest) -> Option<String> {
        match req {
            ApiRequest::Create { agent_id, .. } |
            ApiRequest::Read { agent_id, .. } |
            ApiRequest::Search { agent_id, .. } |
            ApiRequest::Update { agent_id, .. } |
            ApiRequest::Delete { agent_id, .. } |
            ApiRequest::Remember { agent_id, .. } |
            ApiRequest::Recall { agent_id, .. } |
            ApiRequest::RememberLongTerm { agent_id, .. } |
            ApiRequest::RecallSemantic { agent_id, .. } |
            ApiRequest::Explore { agent_id, .. } |
            ApiRequest::ListDeleted { agent_id, .. } |
            ApiRequest::Restore { agent_id, .. } |
            ApiRequest::History { agent_id, .. } |
            ApiRequest::Rollback { agent_id, .. } |
            ApiRequest::CreateEvent { agent_id, .. } |
            ApiRequest::EventAttach { agent_id, .. } |
            ApiRequest::AddNode { agent_id, .. } |
            ApiRequest::AddEdge { agent_id, .. } |
            ApiRequest::ListNodes { agent_id, .. } |
            ApiRequest::ListNodesAtTime { agent_id, .. } |
            ApiRequest::SubmitIntent { agent_id, .. } |
            ApiRequest::AgentStatus { agent_id } |
            ApiRequest::AgentSuspend { agent_id } |
            ApiRequest::AgentResume { agent_id } |
            ApiRequest::AgentTerminate { agent_id } |
            ApiRequest::ReadMessages { agent_id, .. } |
            ApiRequest::AckMessage { agent_id, .. } |
            ApiRequest::ToolCall { agent_id, .. } |
            ApiRequest::RememberProcedural { agent_id, .. } |
            ApiRequest::RecallProcedural { agent_id, .. } |
            ApiRequest::RecallVisible { agent_id, .. } |
            ApiRequest::AgentSetResources { agent_id, .. } |
            ApiRequest::AgentCheckpoint { agent_id } |
            ApiRequest::AgentRestore { agent_id, .. } |
            ApiRequest::GetNode { agent_id, .. } |
            ApiRequest::ListEdges { agent_id, .. } |
            ApiRequest::RemoveNode { agent_id, .. } |
            ApiRequest::RemoveEdge { agent_id, .. } |
            ApiRequest::UpdateNode { agent_id, .. } |
            ApiRequest::AgentComplete { agent_id } |
            ApiRequest::AgentFail { agent_id, .. } |
            ApiRequest::MemoryMove { agent_id, .. } |
            ApiRequest::MemoryDeleteEntry { agent_id, .. } |
            ApiRequest::EvictExpired { agent_id, .. } |
            ApiRequest::LoadContext { agent_id, .. } |
            ApiRequest::EdgeHistory { agent_id, .. } |
            ApiRequest::ContextAssemble { agent_id, .. } |
            ApiRequest::AgentUsage { agent_id } |
            ApiRequest::TaskStart { agent_id, .. } |
            ApiRequest::TaskComplete { agent_id, .. } |
            ApiRequest::TaskFail { agent_id, .. } |
            ApiRequest::StartSession { agent_id, .. } |
            ApiRequest::EndSession { agent_id, .. } |
            ApiRequest::RegisterSkill { agent_id, .. } |
            ApiRequest::DeclareIntent { agent_id, .. } |
            ApiRequest::FetchAssembledContext { agent_id, .. } |
            ApiRequest::ListTenants { agent_id } |
            ApiRequest::TenantShare { agent_id, .. } |
            ApiRequest::BatchCreate { agent_id, .. } |
            ApiRequest::BatchMemoryStore { agent_id, .. } |
            ApiRequest::BatchSubmitIntent { agent_id, .. } |
            ApiRequest::BatchQuery { agent_id, .. } |
            ApiRequest::KGImpactAnalysis { agent_id, .. } |
            ApiRequest::KGTemporalChanges { agent_id, .. } |
            ApiRequest::QueryGrowthReport { agent_id, .. } |
            ApiRequest::MemoryStats { agent_id, .. } |
            ApiRequest::RememberLongTermBatch { agent_id, .. } |
            ApiRequest::ImportFiles { agent_id, .. } => Some(agent_id.clone()),

            ApiRequest::RegisterAgent { name } => Some(name.clone()),
            ApiRequest::GrantPermission { agent_id, .. } |
            ApiRequest::RevokePermission { agent_id, .. } |
            ApiRequest::ListPermissions { agent_id } |
            ApiRequest::CheckPermission { agent_id, .. } => Some(agent_id.clone()),
            ApiRequest::CostAgentTrend { agent_id, .. } |
            ApiRequest::CostAnomalyCheck { agent_id, .. } => Some(agent_id.clone()),

            ApiRequest::ListAgents |
            ApiRequest::ListEvents { .. } |
            ApiRequest::ListEventsText { .. } |
            ApiRequest::FindPaths { .. } |
            ApiRequest::SendMessage { .. } |
            ApiRequest::ToolList { .. } |
            ApiRequest::ToolDescribe { .. } |
            ApiRequest::DiscoverAgents { .. } |
            ApiRequest::DelegateTask { .. } |
            ApiRequest::QueryTaskStatus { .. } |
            ApiRequest::EventHistory { .. } |
            ApiRequest::DeltaSince { .. } |
            ApiRequest::DiscoverSkills { .. } |
            ApiRequest::IntentFeedback { .. } |
            ApiRequest::CreateTenant { .. } |
            ApiRequest::KGCausalPath { .. } |
            ApiRequest::SwitchEmbeddingModel { .. } |
            ApiRequest::SwitchLlmModel { .. } |
            ApiRequest::CheckModelHealth { .. } |
            ApiRequest::HybridRetrieve { .. } |
            ApiRequest::DiscoverKnowledge { .. } |
            ApiRequest::ObjectUsage { .. } |
            ApiRequest::StorageStats { .. } |
            ApiRequest::EvictCold { .. } |
            ApiRequest::EventSubscribe { .. } |
            ApiRequest::EventPoll { .. } |
            ApiRequest::EventUnsubscribe { .. } |
            ApiRequest::SystemStatus |
            ApiRequest::CacheStats |
            ApiRequest::CacheInvalidate |
            ApiRequest::IntentCacheStats |
            ApiRequest::ClusterStatus |
            ApiRequest::ClusterJoin { .. } |
            ApiRequest::ClusterLeave |
            ApiRequest::NodePing { .. } |
            ApiRequest::QueryTokenUsage { .. } |
            ApiRequest::HookList |
            ApiRequest::HookRegister { .. } |
            ApiRequest::HealthReport |
            ApiRequest::CostSessionSummary { .. } |
            ApiRequest::ListPrompts |
            ApiRequest::GetPromptInfo { .. } |
            ApiRequest::SetPromptOverride { .. } |
            ApiRequest::RemovePromptOverride { .. } => None,
        }
    }

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

        let request_agent_id = Self::extract_agent_id(&req);
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
            req @ ApiRequest::ImportFiles { .. } => self.handle_import(req),
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
