//! Messaging commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::{extract_arg, extract_agent_id};

pub fn cmd_send_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_agent_id(args);
    let to = extract_arg(args, "--to").unwrap_or_default();
    let payload_str = extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
    let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();

    kernel.handle_api_request(ApiRequest::SendMessage { from, to, payload })
}

pub fn cmd_read_messages(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let unread_only = args.iter().any(|a| a == "--unread");

    kernel.handle_api_request(ApiRequest::ReadMessages {
        agent_id, unread_only, limit: None, offset: None,
    })
}

pub fn cmd_ack_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let message_id = args.get(1).cloned().unwrap_or_default();

    kernel.handle_api_request(ApiRequest::AckMessage { agent_id, message_id })
}
