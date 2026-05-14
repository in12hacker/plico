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
        let ctx = PermissionContext::new(agent_id.to_string(), crate::DEFAULT_TENANT.to_string());
        self.permissions.check(&ctx, action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_permission_grant_and_check() {
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test-agent", PermissionAction::Write, None, None);
        let result = kernel.permission_check("test-agent", PermissionAction::Write);
        assert!(result.is_ok());
    }

    #[test]
    fn test_permission_check_read_default() {
        let (kernel, _dir) = make_kernel();
        // Read should be allowed by default policy without explicit grant
        let result = kernel.permission_check("any-agent", PermissionAction::Read);
        assert!(result.is_ok());
    }

    #[test]
    fn test_permission_revoke() {
        let (kernel, _dir) = make_kernel();
        // Delete requires explicit grant (not allowed by default)
        kernel.permission_grant("test-agent", PermissionAction::Delete, None, None);
        assert!(kernel.permission_check("test-agent", PermissionAction::Delete).is_ok());

        kernel.permission_revoke("test-agent", PermissionAction::Delete);
        assert!(kernel.permission_check("test-agent", PermissionAction::Delete).is_err());
    }

    #[test]
    fn test_permission_list() {
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test-agent", PermissionAction::Write, None, None);
        kernel.permission_grant("test-agent", PermissionAction::Delete, None, None);

        let grants = kernel.permission_list("test-agent");
        assert!(grants.len() >= 2);
    }

    #[test]
    fn test_permission_revoke_all() {
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test-agent", PermissionAction::Write, None, None);
        kernel.permission_grant("test-agent", PermissionAction::Delete, None, None);

        kernel.permission_revoke_all("test-agent");
        let grants = kernel.permission_list("test-agent");
        assert!(grants.is_empty());
    }
}
