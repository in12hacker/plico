//! Messaging tool handlers (send, read, ack).

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "message.send" => {
            let to = params.get("to").and_then(|v| v.as_str()).unwrap_or("");
            let payload = params.get("payload").cloned().unwrap_or(serde_json::Value::Null);
            match kernel.send_message(agent_id, to, payload) {
                Ok(id) => ToolResult::ok(json!({"message_id": id})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "message.read" => {
            let unread = params.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false);
            let msgs = kernel.read_messages(agent_id, unread);
            ToolResult::ok(serde_json::to_value(&msgs).unwrap_or_default())
        }
        "message.ack" => {
            let msg_id = params.get("message_id").and_then(|v| v.as_str()).unwrap_or("");
            if kernel.ack_message(agent_id, msg_id) {
                ToolResult::ok(json!({"acked": msg_id}))
            } else {
                ToolResult::error(format!("message not found: {}", msg_id))
            }
        }
        _ => ToolResult::error(format!("unknown messaging tool: {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use crate::kernel::tools::messaging::handle;
    use crate::api::permission::PermissionAction;

    #[test]
    fn test_message_send() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("receiver".to_string()).unwrap();
        kernel.permission_grant("test", PermissionAction::SendMessage, None, None);
        let result = handle(&*kernel, "message.send", &json!({"to": "receiver", "payload": {"text": "hello"}}), "test");
        assert!(result.error.is_none(), "send should succeed: {:?}", result.error);
        assert!(result.output["message_id"].as_str().unwrap().len() > 0);
    }

    #[test]
    fn test_message_read_empty() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "message.read", &json!({}), "test");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_message_send_and_read() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("reader".to_string()).unwrap();
        kernel.permission_grant("test", PermissionAction::SendMessage, None, None);
        handle(&*kernel, "message.send", &json!({"to": "reader", "payload": {"msg": "hi"}}), "test");

        let result = handle(&*kernel, "message.read", &json!({}), "reader");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_message_ack_nonexistent() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "message.ack", &json!({"message_id": "nonexistent"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_message_unknown_tool() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "message.nonexistent", &json!({}), "test");
        assert!(result.error.is_some());
    }
}
