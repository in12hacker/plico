//! Security operations — centralized request validation.
//!
//! Implements the "Immune System" for the AI Kernel, ensuring all
//! requests are authenticated, authorized, and tenant-isolated.

use crate::api::semantic::ApiRequest;
use crate::api::permission::{PermissionAction, PermissionContext};
use crate::DEFAULT_TENANT;

impl crate::kernel::AIKernel {
    /// Consolidate all security checks for an incoming request.
    ///
    /// Verifies:
    /// 1. Identity (Token verification via AgentKeyStore)
    /// 2. Tenant Isolation (Multi-tenant boundaries)
    /// 3. Permission (Action-level capability check)
    pub fn validate_security(&self, req: &ApiRequest) -> Result<(), String> {
        let (agent_id, token, tenant_id) = self.extract_security_info(req);
        let action = self.map_request_to_action(req);

        // 1. Identity verification
        // Some requests (like RegisterAgent) don't have an agent_id yet
        if let Some(aid) = &agent_id {
            // RegisterAgent is special: it creates the identity
            if !matches!(req, ApiRequest::RegisterAgent { .. }) {
                if let Err(e) = self.key_store.verify_agent_token(aid, token.as_deref()) {
                    return Err(format!("Security Red Line (Identity): {}", e));
                }
            }
        } else if self.key_store.requires_token() {
            // If token is required globally but no agent_id found, only allow system status
            if !matches!(req, ApiRequest::SystemStatus | ApiRequest::HealthReport) {
                return Err("Security Red Line (Identity): Anonymous request denied".to_string());
            }
        }

        // 2. Tenant isolation & 3. Permission check
        if let Some(aid) = agent_id {
            let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
            let ctx = PermissionContext::new(aid, tenant);
            
            // Check permission (includes default policy for Read/Write)
            if let Err(e) = self.permissions.check(&ctx, action) {
                return Err(format!("Security Red Line (Capability): {}", e));
            }
        }

        Ok(())
    }

    /// Extract (agent_id, agent_token, tenant_id) from any ApiRequest.
    pub fn extract_security_info(&self, req: &ApiRequest) -> (Option<String>, Option<String>, Option<String>) {
        match req {
            // Group 1: Requests with (agent_id, agent_token, tenant_id)
            ApiRequest::Create { agent_id, agent_token, tenant_id, .. } |
            ApiRequest::Read { agent_id, agent_token, tenant_id, .. } |
            ApiRequest::Search { agent_id, agent_token, tenant_id, .. } |
            ApiRequest::Update { agent_id, agent_token, tenant_id, .. } |
            ApiRequest::Delete { agent_id, agent_token, tenant_id, .. } => {
                (Some(agent_id.clone()), agent_token.clone(), tenant_id.clone())
            }

            // Group 2: Requests with (agent_id, tenant_id) but NO agent_token
            ApiRequest::RecallRouted { agent_id, tenant_id, .. } |
            ApiRequest::AddNode { agent_id, tenant_id, .. } |
            ApiRequest::AddEdge { agent_id, tenant_id, .. } |
            ApiRequest::ListNodes { agent_id, tenant_id, .. } |
            ApiRequest::ListNodesAtTime { agent_id, tenant_id, .. } |
            ApiRequest::GetNode { agent_id, tenant_id, .. } |
            ApiRequest::ListEdges { agent_id, tenant_id, .. } |
            ApiRequest::RemoveNode { agent_id, tenant_id, .. } |
            ApiRequest::RemoveEdge { agent_id, tenant_id, .. } |
            ApiRequest::UpdateNode { agent_id, tenant_id, .. } |
            ApiRequest::EdgeHistory { agent_id, tenant_id, .. } |
            ApiRequest::KGCausalPath { agent_id, tenant_id, .. } |
            ApiRequest::KGImpactAnalysis { agent_id, tenant_id, .. } |
            ApiRequest::KGTemporalChanges { agent_id, tenant_id, .. } |
            ApiRequest::HybridRetrieve { agent_id, tenant_id, .. } |
            ApiRequest::MemoryMove { agent_id, tenant_id, .. } |
            ApiRequest::MemoryDeleteEntry { agent_id, tenant_id, .. } |
            ApiRequest::EvictExpired { agent_id, tenant_id, .. } |
            ApiRequest::LoadContext { agent_id, tenant_id, .. } |
            ApiRequest::BatchCreate { agent_id, tenant_id, .. } |
            ApiRequest::BatchMemoryStore { agent_id, tenant_id, .. } |
            ApiRequest::BatchQuery { agent_id, tenant_id, .. } |
            ApiRequest::MemoryStats { agent_id, tenant_id, .. } |
            ApiRequest::RememberLongTermBatch { agent_id, tenant_id, .. } |
            ApiRequest::Remember { agent_id, tenant_id, .. } |
            ApiRequest::RememberLongTerm { agent_id, tenant_id, .. } |
            ApiRequest::ImportFiles { agent_id, tenant_id, .. } => {
                (Some(agent_id.clone()), None, tenant_id.clone())
            }

            ApiRequest::StartSession { agent_id, agent_token, .. } => {
                (Some(agent_id.clone()), agent_token.clone(), None)
            }

            ApiRequest::RegisterAgent { name } => (Some(name.clone()), None, None),

            // Group 3: Requests with only agent_id
            ApiRequest::Recall { agent_id, .. } |
            ApiRequest::RecallSemantic { agent_id, .. } |
            ApiRequest::Explore { agent_id, .. } |
            ApiRequest::ListDeleted { agent_id, .. } |
            ApiRequest::Restore { agent_id, .. } |
            ApiRequest::History { agent_id, .. } |
            ApiRequest::Rollback { agent_id, .. } |
            ApiRequest::CreateEvent { agent_id, .. } |
            ApiRequest::ListEvents { agent_id, .. } |
            ApiRequest::ListEventsText { agent_id, .. } |
            ApiRequest::EventAttach { agent_id, .. } |
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
            ApiRequest::AgentComplete { agent_id } |
            ApiRequest::AgentFail { agent_id, .. } |
            ApiRequest::ContextAssemble { agent_id, .. } |
            ApiRequest::AgentUsage { agent_id } |
            ApiRequest::TaskStart { agent_id, .. } |
            ApiRequest::TaskComplete { agent_id, .. } |
            ApiRequest::TaskFail { agent_id, .. } |
            ApiRequest::EndSession { agent_id, .. } |
            ApiRequest::RegisterSkill { agent_id, .. } |
            ApiRequest::DeclareIntent { agent_id, .. } |
            ApiRequest::FetchAssembledContext { agent_id, .. } |
            ApiRequest::ListTenants { agent_id } |
            ApiRequest::TenantShare { agent_id, .. } |
            ApiRequest::BatchSubmitIntent { agent_id, .. } |
            ApiRequest::QueryGrowthReport { agent_id, .. } |
            ApiRequest::DiscoverKnowledge { agent_id, .. } |
            ApiRequest::ObjectUsage { agent_id, .. } |
            ApiRequest::StorageStats { agent_id, .. } |
            ApiRequest::EvictCold { agent_id, .. } |
            ApiRequest::EventSubscribe { agent_id, .. } |
            ApiRequest::QueryTokenUsage { agent_id, .. } |
            ApiRequest::CostAgentTrend { agent_id, .. } |
            ApiRequest::CostAnomalyCheck { agent_id, .. } |
            ApiRequest::GrantPermission { agent_id, .. } |
            ApiRequest::RevokePermission { agent_id, .. } |
            ApiRequest::ListPermissions { agent_id } |
            ApiRequest::CheckPermission { agent_id, .. } => (Some(agent_id.clone()), None, None),

            _ => (None, None, None),
        }
    }

    /// Map an ApiRequest to its required PermissionAction.
    pub fn map_request_to_action(&self, req: &ApiRequest) -> PermissionAction {
        match req {
            ApiRequest::Read { .. } |
            ApiRequest::Recall { .. } |
            ApiRequest::RecallSemantic { .. } |
            ApiRequest::RecallRouted { .. } |
            ApiRequest::RecallProcedural { .. } |
            ApiRequest::RecallVisible { .. } |
            ApiRequest::LoadContext { .. } |
            ApiRequest::Search { .. } |
            ApiRequest::History { .. } |
            ApiRequest::ListDeleted { .. } |
            ApiRequest::ListEvents { .. } |
            ApiRequest::ListEventsText { .. } |
            ApiRequest::ListNodes { .. } |
            ApiRequest::ListNodesAtTime { .. } |
            ApiRequest::ListEdges { .. } |
            ApiRequest::GetNode { .. } |
            ApiRequest::FindPaths { .. } |
            ApiRequest::EdgeHistory { .. } |
            ApiRequest::KGCausalPath { .. } |
            ApiRequest::KGImpactAnalysis { .. } |
            ApiRequest::KGTemporalChanges { .. } |
            ApiRequest::HybridRetrieve { .. } |
            ApiRequest::Explore { .. } |
            ApiRequest::DiscoverKnowledge { .. } |
            ApiRequest::ObjectUsage { .. } |
            ApiRequest::StorageStats { .. } |
            ApiRequest::ReadMessages { .. } |
            ApiRequest::AgentStatus { .. } |
            ApiRequest::AgentUsage { .. } |
            ApiRequest::QueryTaskStatus { .. } |
            ApiRequest::QueryTokenUsage { .. } |
            ApiRequest::QueryGrowthReport { .. } |
            ApiRequest::MemoryStats { .. } |
            ApiRequest::FetchAssembledContext { .. } |
            ApiRequest::BatchQuery { .. } => PermissionAction::Read,

            ApiRequest::Create { .. } |
            ApiRequest::Update { .. } |
            ApiRequest::Restore { .. } |
            ApiRequest::Rollback { .. } |
            ApiRequest::Remember { .. } |
            ApiRequest::RememberLongTerm { .. } |
            ApiRequest::RememberProcedural { .. } |
            ApiRequest::RememberLongTermBatch { .. } |
            ApiRequest::AddNode { .. } |
            ApiRequest::AddEdge { .. } |
            ApiRequest::UpdateNode { .. } |
            ApiRequest::CreateEvent { .. } |
            ApiRequest::EventAttach { .. } |
            ApiRequest::SubmitIntent { .. } |
            ApiRequest::DeclareIntent { .. } |
            ApiRequest::AckMessage { .. } |
            ApiRequest::TaskStart { .. } |
            ApiRequest::TaskComplete { .. } |
            ApiRequest::BatchCreate { .. } |
            ApiRequest::BatchMemoryStore { .. } |
            ApiRequest::BatchSubmitIntent { .. } |
            ApiRequest::ImportFiles { .. } => PermissionAction::Write,

            ApiRequest::Delete { .. } |
            ApiRequest::RemoveNode { .. } |
            ApiRequest::RemoveEdge { .. } |
            ApiRequest::MemoryDeleteEntry { .. } |
            ApiRequest::EvictExpired { .. } |
            ApiRequest::EvictCold { .. } => PermissionAction::Delete,

            ApiRequest::ToolCall { .. } => PermissionAction::Execute,
            
            ApiRequest::SendMessage { .. } |
            ApiRequest::DelegateTask { .. } => PermissionAction::SendMessage,

            ApiRequest::TenantShare { .. } => PermissionAction::CrossTenant,

            ApiRequest::RegisterAgent { .. } |
            ApiRequest::RegisterSkill { .. } |
            ApiRequest::StartSession { .. } |
            ApiRequest::EndSession { .. } |
            ApiRequest::AgentSuspend { .. } |
            ApiRequest::AgentResume { .. } |
            ApiRequest::AgentTerminate { .. } |
            ApiRequest::AgentCheckpoint { .. } |
            ApiRequest::AgentRestore { .. } |
            ApiRequest::AgentSetResources { .. } |
            ApiRequest::AgentComplete { .. } |
            ApiRequest::AgentFail { .. } |
            ApiRequest::TaskFail { .. } => PermissionAction::Write, // Lifecycle is Write-equivalent

            ApiRequest::GrantPermission { .. } |
            ApiRequest::RevokePermission { .. } |
            ApiRequest::ListPermissions { .. } |
            ApiRequest::CheckPermission { .. } => PermissionAction::All, // Permission mgmt requires All

            _ => PermissionAction::Read, // Default to safe Read
        }
    }
}
