//! Security operations — centralized request validation.
//!
//! Implements the "Immune System" for the AI Kernel, ensuring all
//! requests are authenticated, authorized, and confined to personal-vault namespaces.

use crate::api::permission::{PermissionAction, PermissionContext};
use crate::api::semantic::ApiRequest;
use crate::DEFAULT_TENANT;

impl crate::kernel::AIKernel {
    /// Consolidate all security checks for an incoming request.
    ///
    /// Verifies:
    /// 1. Identity (Token verification via AgentKeyStore)
    /// 2. Legacy personal-vault namespace boundary
    /// 3. Permission (Action-level capability check)
    pub fn validate_security(&self, req: &ApiRequest) -> Result<(), String> {
        self.validate_security_for_origin(req, false)
    }

    pub(crate) fn validate_security_for_origin(
        &self,
        req: &ApiRequest,
        trusted_internal_origin: bool,
    ) -> Result<(), String> {
        let (agent_id, token, tenant_id) = self.extract_security_info(req);
        let action = self.map_request_to_action(req);

        // `kernel` and `system` are internal principals, not bearer names.
        // Public API callers cannot gain their bypass merely by writing one of
        // those strings into an unauthenticated request.
        if agent_id.as_deref().is_some_and(|aid| self.permissions.is_trusted(aid)) && !trusted_internal_origin {
            return Err(format!(
                "Security Red Line (Identity): reserved agent '{}' is restricted to kernel-owned calls",
                agent_id.as_deref().unwrap_or_default()
            ));
        }

        // Permission mutations are always strongly authenticated, even when the
        // daemon is in Optional mode. This prevents an unauthenticated caller
        // from naming itself (or `kernel`/`system`) and granting capabilities.
        if action == PermissionAction::ManagePermissions {
            let caller = agent_id.as_deref().ok_or_else(|| {
                "Security Red Line (Identity): permission mutation requires caller_agent_id".to_string()
            })?;
            let caller_token = token.as_deref().ok_or_else(|| {
                format!(
                    "Security Red Line (Identity): Agent '{}': token required for permission mutation",
                    caller
                )
            })?;
            if !self.key_store.verify_token(caller, caller_token) {
                return Err(format!(
                    "Security Red Line (Identity): Agent '{}': invalid token",
                    caller
                ));
            }
        }

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

        // 2. Legacy personal-vault namespace boundary & 3. permission check.
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
            ApiRequest::Create {
                agent_id,
                agent_token,
                tenant_id,
                ..
            }
            | ApiRequest::Read {
                agent_id,
                agent_token,
                tenant_id,
                ..
            }
            | ApiRequest::Search {
                agent_id,
                agent_token,
                tenant_id,
                ..
            }
            | ApiRequest::Update {
                agent_id,
                agent_token,
                tenant_id,
                ..
            }
            | ApiRequest::Delete {
                agent_id,
                agent_token,
                tenant_id,
                ..
            } => (Some(agent_id.clone()), agent_token.clone(), tenant_id.clone()),
            // Group 2: Requests with (agent_id, tenant_id) but NO agent_token
            ApiRequest::AddNode {
                agent_id, tenant_id, ..
            }
            | ApiRequest::AddEdge {
                agent_id, tenant_id, ..
            }
            | ApiRequest::ListNodes {
                agent_id, tenant_id, ..
            }
            | ApiRequest::ListNodesAtTime {
                agent_id, tenant_id, ..
            }
            | ApiRequest::GetNode {
                agent_id, tenant_id, ..
            }
            | ApiRequest::ListEdges {
                agent_id, tenant_id, ..
            }
            | ApiRequest::RemoveNode {
                agent_id, tenant_id, ..
            }
            | ApiRequest::RemoveEdge {
                agent_id, tenant_id, ..
            }
            | ApiRequest::UpdateNode {
                agent_id, tenant_id, ..
            }
            | ApiRequest::EdgeHistory {
                agent_id, tenant_id, ..
            }
            | ApiRequest::KGCausalPath {
                agent_id, tenant_id, ..
            }
            | ApiRequest::KGImpactAnalysis {
                agent_id, tenant_id, ..
            }
            | ApiRequest::KGTemporalChanges {
                agent_id, tenant_id, ..
            }
            | ApiRequest::MemoryDeleteEntry {
                agent_id, tenant_id, ..
            }
            | ApiRequest::LoadContext {
                agent_id, tenant_id, ..
            }
            | ApiRequest::BatchCreate {
                agent_id, tenant_id, ..
            }
            | ApiRequest::BatchMemoryStore {
                agent_id, tenant_id, ..
            }
            | ApiRequest::BatchQuery {
                agent_id, tenant_id, ..
            }
            | ApiRequest::MemoryStats {
                agent_id, tenant_id, ..
            }
            | ApiRequest::RememberLongTermBatch {
                agent_id, tenant_id, ..
            }
            | ApiRequest::Remember {
                agent_id, tenant_id, ..
            }
            | ApiRequest::RememberLongTerm {
                agent_id, tenant_id, ..
            }
            | ApiRequest::ImportFiles {
                agent_id, tenant_id, ..
            } => (Some(agent_id.clone()), None, tenant_id.clone()),

            ApiRequest::StartSession {
                agent_id, agent_token, ..
            } => (Some(agent_id.clone()), agent_token.clone(), None),

            ApiRequest::RegisterAgent { name } => (Some(name.clone()), None, None),

            // Permission mutations carry two identities: `agent_id` is the
            // target, while `caller_agent_id` is the authenticated principal.
            ApiRequest::GrantPermission {
                caller_agent_id,
                agent_token,
                ..
            }
            | ApiRequest::RevokePermission {
                caller_agent_id,
                agent_token,
                ..
            } => (caller_agent_id.clone(), agent_token.clone(), None),

            ApiRequest::SendMessage { from, .. } => (Some(from.clone()), None, None),
            ApiRequest::DelegateTask { from_agent, .. } => (Some(from_agent.clone()), None, None),
            ApiRequest::AgentSetResources { caller_agent_id, .. } => (Some(caller_agent_id.clone()), None, None),

            // Group 3: Requests with only agent_id
            ApiRequest::Recall { agent_id, .. }
            | ApiRequest::Explore { agent_id, .. }
            | ApiRequest::ListDeleted { agent_id, .. }
            | ApiRequest::Restore { agent_id, .. }
            | ApiRequest::History { agent_id, .. }
            | ApiRequest::Rollback { agent_id, .. }
            | ApiRequest::CreateEvent { agent_id, .. }
            | ApiRequest::ListEvents { agent_id, .. }
            | ApiRequest::ListEventsText { agent_id, .. }
            | ApiRequest::EventAttach { agent_id, .. }
            | ApiRequest::SubmitIntent { agent_id, .. }
            | ApiRequest::AgentStatus { agent_id }
            | ApiRequest::AgentSuspend { agent_id }
            | ApiRequest::AgentResume { agent_id }
            | ApiRequest::AgentTerminate { agent_id }
            | ApiRequest::ReadMessages { agent_id, .. }
            | ApiRequest::AckMessage { agent_id, .. }
            | ApiRequest::ToolCall { agent_id, .. }
            | ApiRequest::RememberProcedural { agent_id, .. }
            | ApiRequest::RecallProcedural { agent_id, .. }
            | ApiRequest::AgentCheckpoint { agent_id }
            | ApiRequest::AgentComplete { agent_id }
            | ApiRequest::AgentFail { agent_id, .. }
            | ApiRequest::ContextAssemble { agent_id, .. }
            | ApiRequest::AgentUsage { agent_id }
            | ApiRequest::TaskStart { agent_id, .. }
            | ApiRequest::TaskComplete { agent_id, .. }
            | ApiRequest::TaskFail { agent_id, .. }
            | ApiRequest::EndSession { agent_id, .. }
            | ApiRequest::RegisterSkill { agent_id, .. }
            | ApiRequest::DeclareIntent { agent_id, .. }
            | ApiRequest::FetchAssembledContext { agent_id, .. }
            | ApiRequest::BatchSubmitIntent { agent_id, .. }
            | ApiRequest::QueryGrowthReport { agent_id, .. }
            | ApiRequest::ObjectUsage { agent_id, .. }
            | ApiRequest::StorageStats { agent_id, .. }
            | ApiRequest::EventSubscribe { agent_id, .. }
            | ApiRequest::QueryTokenUsage { agent_id, .. }
            | ApiRequest::CostAgentTrend { agent_id, .. }
            | ApiRequest::CostAnomalyCheck { agent_id, .. }
            | ApiRequest::CoreGet { agent_id, .. }
            | ApiRequest::CoreList { agent_id, .. }
            | ApiRequest::CoreSearch { agent_id, .. }
            | ApiRequest::CoreCreate { agent_id, .. }
            | ApiRequest::CoreUpdate { agent_id, .. }
            | ApiRequest::CoreDelete { agent_id, .. }
            | ApiRequest::CoreExec { agent_id, .. }
            | ApiRequest::CoreObserve { agent_id, .. }
            | ApiRequest::CoreLink { agent_id, .. }
            | ApiRequest::CoreAsk { agent_id, .. }
            | ApiRequest::CoreState { agent_id, .. }
            | ApiRequest::ListPermissions { agent_id }
            | ApiRequest::CheckPermission { agent_id, .. } => (Some(agent_id.clone()), None, None),

            _ => (None, None, None),
        }
    }

    /// Map an ApiRequest to its required PermissionAction.
    pub fn map_request_to_action(&self, req: &ApiRequest) -> PermissionAction {
        match req {
            ApiRequest::Read { .. }
            | ApiRequest::Recall { .. }
            | ApiRequest::RecallProcedural { .. }
            | ApiRequest::LoadContext { .. }
            | ApiRequest::Search { .. }
            | ApiRequest::History { .. }
            | ApiRequest::ListDeleted { .. }
            | ApiRequest::ListEvents { .. }
            | ApiRequest::ListEventsText { .. }
            | ApiRequest::ListNodes { .. }
            | ApiRequest::ListNodesAtTime { .. }
            | ApiRequest::ListEdges { .. }
            | ApiRequest::GetNode { .. }
            | ApiRequest::FindPaths { .. }
            | ApiRequest::EdgeHistory { .. }
            | ApiRequest::KGCausalPath { .. }
            | ApiRequest::KGImpactAnalysis { .. }
            | ApiRequest::KGTemporalChanges { .. }
            | ApiRequest::Explore { .. }
            | ApiRequest::ObjectUsage { .. }
            | ApiRequest::StorageStats { .. }
            | ApiRequest::ReadMessages { .. }
            | ApiRequest::AgentStatus { .. }
            | ApiRequest::AgentUsage { .. }
            | ApiRequest::QueryTaskStatus { .. }
            | ApiRequest::QueryTokenUsage { .. }
            | ApiRequest::QueryGrowthReport { .. }
            | ApiRequest::MemoryStats { .. }
            | ApiRequest::FetchAssembledContext { .. }
            | ApiRequest::BatchQuery { .. }
            | ApiRequest::CoreGet { .. }
            | ApiRequest::CoreList { .. }
            | ApiRequest::CoreSearch { .. }
            | ApiRequest::CoreObserve { .. }
            | ApiRequest::CoreAsk { .. } => PermissionAction::Read,

            ApiRequest::Create { .. }
            | ApiRequest::Update { .. }
            | ApiRequest::Restore { .. }
            | ApiRequest::Rollback { .. }
            | ApiRequest::Remember { .. }
            | ApiRequest::RememberLongTerm { .. }
            | ApiRequest::RememberProcedural { .. }
            | ApiRequest::RememberLongTermBatch { .. }
            | ApiRequest::AddNode { .. }
            | ApiRequest::AddEdge { .. }
            | ApiRequest::UpdateNode { .. }
            | ApiRequest::CreateEvent { .. }
            | ApiRequest::EventAttach { .. }
            | ApiRequest::SubmitIntent { .. }
            | ApiRequest::DeclareIntent { .. }
            | ApiRequest::AckMessage { .. }
            | ApiRequest::TaskStart { .. }
            | ApiRequest::TaskComplete { .. }
            | ApiRequest::BatchCreate { .. }
            | ApiRequest::BatchMemoryStore { .. }
            | ApiRequest::BatchSubmitIntent { .. }
            | ApiRequest::ImportFiles { .. }
            | ApiRequest::CoreCreate { .. }
            | ApiRequest::CoreUpdate { .. }
            | ApiRequest::CoreLink { .. }
            | ApiRequest::CoreState { .. } => PermissionAction::Write,

            ApiRequest::Delete { .. }
            | ApiRequest::RemoveNode { .. }
            | ApiRequest::RemoveEdge { .. }
            | ApiRequest::MemoryDeleteEntry { .. }
            | ApiRequest::CoreDelete { .. } => PermissionAction::Delete,

            ApiRequest::CoreExec { action, params, .. }
                if action == "tool_call"
                    && params
                        .get("tool")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|tool| matches!(tool, "permission.grant" | "permission.revoke")) =>
            {
                PermissionAction::ManagePermissions
            }
            ApiRequest::CoreExec { .. } => PermissionAction::Execute,

            ApiRequest::ToolCall { tool, .. } if matches!(tool.as_str(), "permission.grant" | "permission.revoke") => {
                PermissionAction::ManagePermissions
            }
            ApiRequest::ToolCall { .. } => PermissionAction::Execute,

            ApiRequest::SendMessage { .. } | ApiRequest::DelegateTask { .. } => PermissionAction::SendMessage,

            ApiRequest::RegisterAgent { .. }
            | ApiRequest::RegisterSkill { .. }
            | ApiRequest::StartSession { .. }
            | ApiRequest::EndSession { .. }
            | ApiRequest::AgentSuspend { .. }
            | ApiRequest::AgentResume { .. }
            | ApiRequest::AgentTerminate { .. }
            | ApiRequest::AgentCheckpoint { .. }
            | ApiRequest::AgentSetResources { .. }
            | ApiRequest::AgentComplete { .. }
            | ApiRequest::AgentFail { .. }
            | ApiRequest::TaskFail { .. } => PermissionAction::Write, // Lifecycle is Write-equivalent

            ApiRequest::GrantPermission { .. } | ApiRequest::RevokePermission { .. } => {
                PermissionAction::ManagePermissions
            }
            ApiRequest::ListPermissions { .. } | ApiRequest::CheckPermission { .. } => PermissionAction::Read, // Query operations are Read

            _ => PermissionAction::Read, // Default to safe Read
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_extract_security_info_create() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Create {
            api_version: None,
            content: "test".to_string(),
            content_encoding: Default::default(),
            tags: vec![],
            agent_id: "test-agent".to_string(),
            tenant_id: Some("tenant-a".to_string()),
            agent_token: Some("tok123".to_string()),
            intent: None,
            scope: None,
        };
        let (agent_id, token, tenant) = kernel.extract_security_info(&req);
        assert_eq!(agent_id, Some("test-agent".to_string()));
        assert_eq!(token, Some("tok123".to_string()));
        assert_eq!(tenant, Some("tenant-a".to_string()));
    }

    #[test]
    fn test_extract_security_info_register_agent() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::RegisterAgent {
            name: "new-agent".to_string(),
        };
        let (agent_id, token, tenant) = kernel.extract_security_info(&req);
        assert_eq!(agent_id, Some("new-agent".to_string()));
        assert!(token.is_none());
        assert!(tenant.is_none());
    }

    #[test]
    fn test_extract_security_info_system_status() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::SystemStatus;
        let (agent_id, token, tenant) = kernel.extract_security_info(&req);
        assert!(agent_id.is_none());
        assert!(token.is_none());
        assert!(tenant.is_none());
    }

    #[test]
    fn test_map_request_to_action_create() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Create {
            api_version: None,
            content: "test".to_string(),
            content_encoding: Default::default(),
            tags: vec![],
            agent_id: "agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
            scope: None,
        };
        assert_eq!(kernel.map_request_to_action(&req), PermissionAction::Write);
    }

    #[test]
    fn test_map_request_to_action_read() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Read {
            cid: "abc".to_string(),
            agent_id: "agent".to_string(),
            tenant_id: None,
            agent_token: None,
        };
        assert_eq!(kernel.map_request_to_action(&req), PermissionAction::Read);
    }

    #[test]
    fn test_map_request_to_action_delete() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Delete {
            cid: "abc".to_string(),
            agent_id: "agent".to_string(),
            tenant_id: None,
            agent_token: None,
        };
        assert_eq!(kernel.map_request_to_action(&req), PermissionAction::Delete);
    }

    #[test]
    fn test_map_request_to_action_system_status() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::SystemStatus;
        assert_eq!(kernel.map_request_to_action(&req), PermissionAction::Read);
    }

    #[test]
    fn test_validate_security_system_status() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::SystemStatus;
        assert!(kernel.validate_security(&req).is_ok());
    }

    #[test]
    fn test_validate_security_register_agent() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::RegisterAgent {
            name: "brand-new-agent".to_string(),
        };
        assert!(kernel.validate_security(&req).is_ok());
    }

    #[test]
    fn test_public_reserved_identity_cannot_delete_without_token() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Delete {
            cid: "irrelevant".to_string(),
            agent_id: "kernel".to_string(),
            tenant_id: None,
            agent_token: None,
        };
        let response = kernel.handle_api_request(req);
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert!(error.contains("restricted to kernel-owned calls"));
    }

    #[test]
    fn test_public_reserved_identity_cannot_execute_tool() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::ToolCall {
            tool: "cas.delete".to_string(),
            params: serde_json::json!({"cid": "irrelevant"}),
            agent_id: "system".to_string(),
        };
        let response = kernel.handle_api_request(req);
        assert!(!response.ok);
        let error = response.error.unwrap();
        assert!(error.contains("restricted to kernel-owned calls"));
    }

    #[test]
    fn test_public_core_delete_cannot_hide_reserved_identity() {
        let (kernel, _dir) = make_kernel();
        let response = kernel.handle_api_request(ApiRequest::CoreDelete {
            id: "irrelevant".to_string(),
            variant: Some("cas".to_string()),
            agent_id: "kernel".to_string(),
        });
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("restricted to kernel-owned calls"));
    }

    #[test]
    fn test_public_core_permission_exec_requires_strong_authentication() {
        let (kernel, _dir) = make_kernel();
        let response = kernel.handle_api_request(ApiRequest::CoreExec {
            action: "tool_call".to_string(),
            params: serde_json::json!({
                "tool": "permission.grant",
                "params": {"agent_id": "attacker", "action": "all"}
            }),
            agent_id: "attacker".to_string(),
        });
        assert!(!response.ok);
        assert!(response.error.unwrap().contains("token required"));
        assert!(kernel.permission_list("attacker").is_empty());
    }

    #[test]
    fn test_internal_reserved_identity_remains_available() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Search {
            query: String::new(),
            agent_id: "system".to_string(),
            tenant_id: None,
            agent_token: None,
            limit: Some(1),
            offset: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
            intent_context: None,
        };
        assert!(kernel.validate_security_for_origin(&req, true).is_ok());
    }
}
