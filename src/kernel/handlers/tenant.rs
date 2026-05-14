//! Tenant management handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::DEFAULT_TENANT;

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_create_tenant() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "tenant_a".to_string(),
            admin_agent_id: "admin_agent".to_string(),
            caller_agent_id: "system".to_string(),
        });
        assert!(resp.ok, "CreateTenant should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_create_tenant_unauthorized() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "tenant_b".to_string(),
            admin_agent_id: "admin_agent".to_string(),
            caller_agent_id: "regular_agent".to_string(),
        });
        assert!(!resp.ok, "CreateTenant by untrusted agent should fail");
    }

    #[test]
    fn test_list_tenants() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "tenant_list".to_string(),
            admin_agent_id: "admin_agent".to_string(),
            caller_agent_id: "system".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::ListTenants {
            agent_id: "admin_agent".to_string(),
        });
        assert!(resp.ok, "ListTenants should succeed: {:?}", resp.error);
        let tenants = resp.tenants.unwrap();
        assert!(!tenants.is_empty(), "should have at least one tenant");
    }

    #[test]
    fn test_tenant_share() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "src_tenant".to_string(),
            admin_agent_id: "admin_agent".to_string(),
            caller_agent_id: "system".to_string(),
        });
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "dst_tenant".to_string(),
            admin_agent_id: "admin_agent".to_string(),
            caller_agent_id: "system".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::TenantShare {
            from_tenant: "src_tenant".to_string(),
            to_tenant: "dst_tenant".to_string(),
            resource_type: "kg".to_string(),
            resource_pattern: "*".to_string(),
            agent_id: "system".to_string(),
        });
        assert!(resp.ok, "TenantShare should succeed: {:?}", resp.error);
        assert!(resp.data.is_some());
    }

    #[test]
    fn test_tenant_share_invalid_resource_type() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "src2".to_string(),
            admin_agent_id: "admin".to_string(),
            caller_agent_id: "system".to_string(),
        });
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "dst2".to_string(),
            admin_agent_id: "admin".to_string(),
            caller_agent_id: "system".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::TenantShare {
            from_tenant: "src2".to_string(),
            to_tenant: "dst2".to_string(),
            resource_type: "invalid_type".to_string(),
            resource_pattern: "*".to_string(),
            agent_id: "system".to_string(),
        });
        assert!(!resp.ok, "TenantShare with invalid resource_type should fail");
    }

    #[test]
    fn test_tenant_share_nonexistent_tenant() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::TenantShare {
            from_tenant: "nonexistent".to_string(),
            to_tenant: "also_nonexistent".to_string(),
            resource_type: "cas".to_string(),
            resource_pattern: "*".to_string(),
            agent_id: "system".to_string(),
        });
        assert!(!resp.ok, "TenantShare with nonexistent tenants should fail");
    }

    #[test]
    fn test_tenant_share_requires_permission() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "perm_src".to_string(),
            admin_agent_id: "admin".to_string(),
            caller_agent_id: "system".to_string(),
        });
        kernel.handle_api_request(ApiRequest::CreateTenant {
            tenant_id: "perm_dst".to_string(),
            admin_agent_id: "admin".to_string(),
            caller_agent_id: "system".to_string(),
        });
        let resp = kernel.handle_api_request(ApiRequest::TenantShare {
            from_tenant: "perm_src".to_string(),
            to_tenant: "perm_dst".to_string(),
            resource_type: "memory".to_string(),
            resource_pattern: "*".to_string(),
            agent_id: "untrusted_agent".to_string(),
        });
        assert!(!resp.ok, "TenantShare by agent without CrossTenant permission should fail");
    }
}

impl super::super::AIKernel {
    pub(crate) fn handle_tenant(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::CreateTenant { tenant_id, admin_agent_id, caller_agent_id } => {
                if !self.permissions.is_trusted(&caller_agent_id) {
                    let ctx = crate::api::permission::PermissionContext::new(
                        caller_agent_id.clone(), DEFAULT_TENANT.to_string(),
                    );
                    if let Err(e) = self.permissions.check(&ctx, crate::api::permission::PermissionAction::CrossTenant) {
                        return ApiResponse::error(format!(
                            "Agent '{}' cannot create tenants: {}. Only trusted agents or those with CrossTenant permission can create tenants.",
                            caller_agent_id, e
                        ));
                    }
                }
                match self.tenant_store.create(&tenant_id, &admin_agent_id) {
                    Ok(_tenant) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListTenants { agent_id } => {
                let tenants = self.tenant_store.list_for_agent(&agent_id);
                let dtos: Vec<crate::api::semantic::TenantDto> = tenants.into_iter().map(|t| {
                    crate::api::semantic::TenantDto {
                        id: t.id, admin_agent_id: t.admin_agent_id, created_at_ms: t.created_at_ms,
                    }
                }).collect();
                let mut r = ApiResponse::ok();
                r.tenants = Some(dtos);
                r
            }
            ApiRequest::TenantShare { from_tenant, to_tenant, resource_type, resource_pattern, agent_id } => {
                if !crate::kernel::ops::tenant::TenantShare::is_valid_resource_type(&resource_type) {
                    return ApiResponse::error(format!(
                        "Invalid resource_type '{}'. Must be 'kg', 'memory', or 'cas'.", resource_type
                    ));
                }
                let ctx = crate::api::permission::PermissionContext::new(
                    agent_id.clone(), from_tenant.clone(),
                );
                if let Err(e) = self.permissions.check(&ctx, crate::api::permission::PermissionAction::CrossTenant) {
                    return ApiResponse::error(format!(
                        "Agent '{}' in tenant '{}' cannot share resources with tenant '{}': {}. CrossTenant permission required.",
                        agent_id, from_tenant, to_tenant, e
                    ));
                }
                if !self.tenant_store.exists(&from_tenant) {
                    return ApiResponse::error(format!("Source tenant '{}' does not exist.", from_tenant));
                }
                if !self.tenant_store.exists(&to_tenant) {
                    return ApiResponse::error(format!("Destination tenant '{}' does not exist.", to_tenant));
                }
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::json!({
                    "message": format!(
                        "Share {} resources matching '{}' from tenant '{}' to tenant '{}' initiated by agent '{}'.",
                        resource_type, resource_pattern, from_tenant, to_tenant, agent_id
                    ),
                    "from_tenant": from_tenant, "to_tenant": to_tenant,
                    "resource_type": resource_type, "resource_pattern": resource_pattern,
                }).to_string());
                r
            }
            _ => unreachable!("non-tenant request routed to handle_tenant"),
        }
    }
}
