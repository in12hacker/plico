//! Permission management operations — grant, revoke, list, check.

use crate::api::permission::{PermissionAction, PermissionContext, PermissionGrant};

impl crate::kernel::AIKernel {
    pub fn permission_grant(
        &self,
        agent_id: &str,
        action: PermissionAction,
        scope: Option<String>,
        expires_at: Option<u64>,
    ) {
        let mut grant = PermissionGrant::new(action);
        if let Some(s) = scope {
            grant = grant.with_scope(s);
        }
        if let Some(exp) = expires_at {
            grant = grant.with_expiry(exp);
        }
        self.permissions.grant(agent_id, grant);
        self.persist_permissions();
    }

    pub fn permission_revoke(&self, agent_id: &str, action: PermissionAction) {
        self.permissions.revoke(agent_id, action);
        self.persist_permissions();
    }

    pub fn permission_revoke_all(&self, agent_id: &str) {
        self.permissions.revoke_all(agent_id);
        self.persist_permissions();
    }

    pub fn permission_list(&self, agent_id: &str) -> Vec<PermissionGrant> {
        self.permissions.list_grants(agent_id)
    }

    pub fn permission_check(&self, agent_id: &str, action: PermissionAction) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, action)
    }
}
