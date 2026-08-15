//! Permission grant/revoke/check handlers.

use crate::api::permission::PermissionGuard;
use crate::api::semantic::{ApiRequest, ApiResponse};

impl super::super::AIKernel {
    pub(crate) fn handle_permission(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::GrantPermission {
                agent_id,
                caller_agent_id: _,
                agent_token: _,
                action,
                scope,
                expires_at,
            } => match PermissionGuard::parse_action(&action) {
                Some(act) => {
                    self.permission_grant(&agent_id, act, scope, expires_at);
                    ApiResponse::ok()
                }
                None => ApiResponse::error(format!("Unknown action: {}", action)),
            },
            ApiRequest::RevokePermission {
                agent_id,
                caller_agent_id: _,
                agent_token: _,
                action,
            } => match PermissionGuard::parse_action(&action) {
                Some(act) => {
                    self.permission_revoke(&agent_id, act);
                    ApiResponse::ok()
                }
                None => ApiResponse::error(format!("Unknown action: {}", action)),
            },
            ApiRequest::ListPermissions { agent_id } => {
                let grants = self.permission_list(&agent_id);
                let dto: Vec<serde_json::Value> = grants
                    .into_iter()
                    .map(|g| {
                        serde_json::json!({
                            "action": format!("{:?}", g.action),
                            "scope": g.scope,
                            "expires_at": g.expires_at,
                        })
                    })
                    .collect();
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&serde_json::json!({"grants": dto})).unwrap_or_default());
                r
            }
            ApiRequest::CheckPermission { agent_id, action } => match PermissionGuard::parse_action(&action) {
                Some(act) => {
                    let allowed = self.permission_check(&agent_id, act).is_ok();
                    let mut r = ApiResponse::ok();
                    r.data = Some(serde_json::to_string(&serde_json::json!({"allowed": allowed})).unwrap_or_default());
                    r
                }
                None => ApiResponse::error(format!("Unknown action: {}", action)),
            },
            _ => unreachable!("non-permission request routed to handle_permission"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::permission::PermissionAction;
    use crate::api::semantic::ApiRequest;
    use crate::kernel::tests::make_kernel;

    fn registered_admin(kernel: &crate::kernel::AIKernel) -> (String, String) {
        let registered = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: format!("permission-admin-{}", rand::random::<u64>()),
        });
        let agent_id = registered.agent_id.expect("admin id");
        let token = registered.token.expect("admin token");
        // Direct kernel API is the trusted bootstrap path. Remote callers must
        // subsequently authenticate and hold this explicit capability.
        kernel.permission_grant(&agent_id, PermissionAction::ManagePermissions, None, None);
        (agent_id, token)
    }

    fn grant_request(target: &str, caller: &str, token: &str, action: &str) -> ApiRequest {
        ApiRequest::GrantPermission {
            agent_id: target.to_string(),
            caller_agent_id: Some(caller.to_string()),
            agent_token: Some(token.to_string()),
            action: action.to_string(),
            scope: None,
            expires_at: None,
        }
    }

    #[test]
    fn test_grant_and_check_permission() {
        let (kernel, _dir) = make_kernel();
        let (admin_id, token) = registered_admin(&kernel);
        // Grant a permission
        let resp = kernel.handle_api_request(grant_request("test_agent", &admin_id, &token, "read"));
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
    fn test_unauthenticated_agent_cannot_self_grant() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "attacker".to_string(),
            caller_agent_id: Some("attacker".to_string()),
            agent_token: None,
            action: "all".to_string(),
            scope: None,
            expires_at: None,
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("token required"));
        assert!(kernel.permission_list("attacker").is_empty());
    }

    #[test]
    fn test_legacy_permission_payload_deserializes_but_fails_closed() {
        let (kernel, _dir) = make_kernel();
        let req: ApiRequest = serde_json::from_value(serde_json::json!({
            "method": "grant_permission",
            "agent_id": "attacker",
            "action": "all",
            "scope": null,
            "expires_at": null
        }))
        .expect("legacy payload remains wire-compatible");
        let resp = kernel.handle_api_request(req);
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("caller_agent_id"));
        assert!(kernel.permission_list("attacker").is_empty());
    }

    #[test]
    fn test_authenticated_agent_without_admin_capability_cannot_self_grant() {
        let (kernel, _dir) = make_kernel();
        let registered = kernel.handle_api_request(ApiRequest::RegisterAgent {
            name: "non-admin".to_string(),
        });
        let agent_id = registered.agent_id.unwrap();
        let token = registered.token.unwrap();
        let resp = kernel.handle_api_request(grant_request(&agent_id, &agent_id, &token, "all"));
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("ManagePermissions"));
        assert!(kernel.permission_list(&agent_id).is_empty());
    }

    #[test]
    fn test_forged_trusted_identity_cannot_grant() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "attacker".to_string(),
            caller_agent_id: Some("kernel".to_string()),
            agent_token: Some("forged-token".to_string()),
            action: "all".to_string(),
            scope: None,
            expires_at: None,
        });
        assert!(!resp.ok);
        assert!(resp.error.unwrap().contains("restricted to kernel-owned calls"));
        assert!(kernel.permission_list("attacker").is_empty());
    }

    #[test]
    fn test_grant_permission_unknown_action() {
        let (kernel, _dir) = make_kernel();
        let (admin_id, token) = registered_admin(&kernel);
        let resp = kernel.handle_api_request(grant_request("test_agent", &admin_id, &token, "fly_to_moon"));
        assert!(!resp.ok, "Unknown action should fail");
        assert!(resp.error.unwrap().contains("Unknown action"));
    }

    #[test]
    fn test_revoke_permission() {
        let (kernel, _dir) = make_kernel();
        let (admin_id, token) = registered_admin(&kernel);
        // Use "delete" which requires explicit grant (not a default like "write")
        kernel.handle_api_request(grant_request("test_agent", &admin_id, &token, "delete"));

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
            caller_agent_id: Some(admin_id),
            agent_token: Some(token),
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
        let (admin_id, token) = registered_admin(&kernel);
        let resp = kernel.handle_api_request(ApiRequest::RevokePermission {
            agent_id: "test_agent".to_string(),
            caller_agent_id: Some(admin_id),
            agent_token: Some(token),
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
        kernel.permission_grant("agent1", PermissionAction::Read, Some("scope_a".to_string()), None);
        kernel.permission_grant("agent1", PermissionAction::Write, None, None);

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
        let (admin_id, token) = registered_admin(&kernel);
        let resp = kernel.handle_api_request(grant_request("power_agent", &admin_id, &token, "all"));
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
