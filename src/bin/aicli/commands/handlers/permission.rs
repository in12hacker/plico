//! Permission management commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::api::semantic::ApiRequest;
use super::extract_arg;

pub fn cmd_permission(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match args.get(1).map(|s| s.as_str()) {
        Some("grant") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();
            let scope = extract_arg(args, "--scope");
            let expires_at = extract_arg(args, "--expires-at")
                .and_then(|s| s.parse::<u64>().ok());

            if action_str.is_empty() {
                return ApiResponse::error("permission grant requires --action");
            }

            match parse_permission_action(&action_str) {
                Some(_) => {
                    kernel.handle_api_request(ApiRequest::GrantPermission {
                        agent_id: agent_id.clone(),
                        action: action_str,
                        scope,
                        expires_at,
                    })
                }
                None => ApiResponse::error(
                    format!("Unknown action: '{}'. Valid: read, write, delete, execute, send, cross_tenant, read_any", action_str))
            }
        }
        Some("check") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();

            if action_str.is_empty() {
                return ApiResponse::error("permission check requires --action");
            }

            match parse_permission_action(&action_str) {
                Some(_) => {
                    kernel.handle_api_request(ApiRequest::CheckPermission {
                        agent_id: agent_id.clone(),
                        action: action_str,
                    })
                }
                None => ApiResponse::error(format!("Unknown action: '{}'", action_str))
            }
        }
        Some("revoke") => {
            let action_str = extract_arg(args, "--action")
                .or_else(|| args.get(2).cloned())
                .unwrap_or_default();

            if action_str.is_empty() {
                return ApiResponse::error("permission revoke requires --action");
            }

            match parse_permission_action(&action_str) {
                Some(_) => {
                    kernel.handle_api_request(ApiRequest::RevokePermission {
                        agent_id: agent_id.clone(),
                        action: action_str,
                    })
                }
                None => ApiResponse::error(format!("Unknown action: '{}'", action_str))
            }
        }
        Some("list") | None => {
            kernel.handle_api_request(ApiRequest::ListPermissions {
                agent_id: agent_id.clone(),
            })
        }
        Some(sub) => ApiResponse::error(
            format!("Unknown permission subcommand: '{}'. Try: grant, check, revoke, list", sub))
    }
}

fn parse_permission_action(s: &str) -> Option<plico::api::permission::PermissionAction> {
    match s.to_lowercase().replace('-', "_").as_str() {
        "read" => Some(plico::api::permission::PermissionAction::Read),
        "read_any" | "readany" => Some(plico::api::permission::PermissionAction::ReadAny),
        "write" => Some(plico::api::permission::PermissionAction::Write),
        "delete" => Some(plico::api::permission::PermissionAction::Delete),
        "execute" => Some(plico::api::permission::PermissionAction::Execute),
        "send" | "sendmessage" => Some(plico::api::permission::PermissionAction::SendMessage),
        "network" => Some(plico::api::permission::PermissionAction::Network),
        "cross_tenant" | "crosstenant" => Some(plico::api::permission::PermissionAction::CrossTenant),
        "all" => Some(plico::api::permission::PermissionAction::All),
        _ => None,
    }
}
