//! Permission grant/revoke/check handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::api::permission::PermissionGuard;

impl super::super::AIKernel {
    pub(crate) fn handle_permission(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::GrantPermission { agent_id, action, scope, expires_at } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        self.permission_grant(&agent_id, act, scope, expires_at);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
            }
            ApiRequest::RevokePermission { agent_id, action } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        self.permission_revoke(&agent_id, act);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
            }
            ApiRequest::ListPermissions { agent_id } => {
                let grants = self.permission_list(&agent_id);
                let dto: Vec<serde_json::Value> = grants.into_iter().map(|g| {
                    serde_json::json!({
                        "action": format!("{:?}", g.action),
                        "scope": g.scope,
                        "expires_at": g.expires_at,
                    })
                }).collect();
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&serde_json::json!({"grants": dto})).unwrap_or_default());
                r
            }
            ApiRequest::CheckPermission { agent_id, action } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        let allowed = self.permission_check(&agent_id, act).is_ok();
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::to_string(&serde_json::json!({"allowed": allowed})).unwrap_or_default());
                        r
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
            }
            _ => unreachable!("non-permission request routed to handle_permission"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_grant_and_check_permission() {
        let (kernel, _dir) = make_kernel();
        // Grant a permission
        let resp = kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(),
            action: "read".to_string(),
            scope: None,
            expires_at: None,
        });
        assert!(resp.ok, "GrantPermission should succeed: {:?}", resp.error);

        // Check it
        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "test_agent".to_string(),
            action: "read".to_string(),
        });
        assert!(resp.ok, "CheckPermission should succeed: {:?}", resp.error);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        assert_eq!(data["allowed"], true);
    }

    #[test]
    fn test_check_permission_not_granted() {
        let (kernel, _dir) = make_kernel();
        // "delete" requires explicit grant (unlike "read"/"write" which are defaults)
        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "unprivileged_agent".to_string(),
            action: "delete".to_string(),
        });
        assert!(resp.ok);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        assert_eq!(data["allowed"], false);
    }

    #[test]
    fn test_grant_permission_unknown_action() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(),
            action: "fly_to_moon".to_string(),
            scope: None,
            expires_at: None,
        });
        assert!(!resp.ok, "Unknown action should fail");
        assert!(resp.error.unwrap().contains("Unknown action"));
    }

    #[test]
    fn test_revoke_permission() {
        let (kernel, _dir) = make_kernel();
        // Use "delete" which requires explicit grant (not a default like "write")
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(),
            action: "delete".to_string(),
            scope: None,
            expires_at: None,
        });

        // Verify granted
        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "test_agent".to_string(),
            action: "delete".to_string(),
        });
        assert!(resp.ok);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        assert_eq!(data["allowed"], true);

        // Revoke
        let resp = kernel.handle_api_request(ApiRequest::RevokePermission {
            agent_id: "test_agent".to_string(),
            action: "delete".to_string(),
        });
        assert!(resp.ok, "RevokePermission should succeed: {:?}", resp.error);

        // Verify revoked
        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "test_agent".to_string(),
            action: "delete".to_string(),
        });
        assert!(resp.ok);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        assert_eq!(data["allowed"], false);
    }

    #[test]
    fn test_revoke_permission_unknown_action() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::RevokePermission {
            agent_id: "test_agent".to_string(),
            action: "teleport".to_string(),
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("Unknown action"));
    }

    #[test]
    fn test_list_permissions_empty() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ListPermissions {
            agent_id: "new_agent".to_string(),
        });
        assert!(resp.ok, "ListPermissions should succeed: {:?}", resp.error);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        let grants = data["grants"].as_array().unwrap();
        assert!(grants.is_empty());
    }

    #[test]
    fn test_list_permissions_after_grant() {
        let (kernel, _dir) = make_kernel();
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "agent1".to_string(),
            action: "read".to_string(),
            scope: Some("scope_a".to_string()),
            expires_at: None,
        });
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "agent1".to_string(),
            action: "write".to_string(),
            scope: None,
            expires_at: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::ListPermissions {
            agent_id: "agent1".to_string(),
        });
        assert!(resp.ok);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        let grants = data["grants"].as_array().unwrap();
        assert_eq!(grants.len(), 2);
    }

    #[test]
    fn test_check_permission_unknown_action() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "test_agent".to_string(),
            action: "levitate".to_string(),
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("Unknown action"));
    }

    #[test]
    fn test_grant_permission_all_action() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "power_agent".to_string(),
            action: "all".to_string(),
            scope: None,
            expires_at: None,
        });
        assert!(resp.ok, "GrantPermission all should succeed: {:?}", resp.error);

        let resp = kernel.handle_api_request(ApiRequest::CheckPermission {
            agent_id: "power_agent".to_string(),
            action: "read".to_string(),
        });
        assert!(resp.ok);
        let data: serde_json::Value = serde_json::from_str(&resp.data.unwrap()).unwrap();
        assert_eq!(data["allowed"], true);
    }
}
