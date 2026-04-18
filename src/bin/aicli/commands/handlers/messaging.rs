//! Messaging commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

pub fn cmd_send_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
    let to = extract_arg(args, "--to").unwrap_or_default();
    let payload_str = extract_arg(args, "--payload").unwrap_or_else(|| "{}".to_string());
    let payload: serde_json::Value = serde_json::from_str(&payload_str).unwrap_or_default();

    match kernel.send_message(&from, &to, payload) {
        Ok(msg_id) => {
            let mut r = ApiResponse::ok();
            r.data = Some(msg_id);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_read_messages(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let unread_only = args.iter().any(|a| a == "--unread");

    let msgs = kernel.read_messages(&agent_id, unread_only);
    let mut r = ApiResponse::ok();
    r.messages = Some(msgs);
    r
}

pub fn cmd_ack_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let message_id = args.get(1).cloned().unwrap_or_default();

    if kernel.ack_message(&agent_id, &message_id) {
        ApiResponse::ok()
    } else {
        ApiResponse::error(format!("Message not found: {}", message_id))
    }
}
