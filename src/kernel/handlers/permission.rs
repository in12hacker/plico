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
