//! Agent-to-agent messaging operations.

use crate::api::permission::{PermissionContext, PermissionAction};
use crate::scheduler::{AgentId, IntentPriority};
use crate::kernel::event_bus::KernelEvent;

impl crate::kernel::AIKernel {
    /// Send a message from one agent to another.
    pub fn send_message(
        &self,
        from: &str,
        to: &str,
        payload: serde_json::Value,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(from.to_string(), crate::DEFAULT_TENANT.to_string());
        self.permissions.check(&ctx, PermissionAction::SendMessage)?;
        let msg_id = self.message_bus.send(from, to, payload);
        Ok(msg_id)
    }

    /// Read messages for an agent.
    pub fn read_messages(&self, agent_id: &str, unread_only: bool) -> Vec<crate::scheduler::messaging::AgentMessage> {
        let ctx = PermissionContext::new(agent_id.to_string(), crate::DEFAULT_TENANT.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        self.message_bus.read(agent_id, unread_only)
    }

    /// Acknowledge (mark as read) a message.
    pub fn ack_message(&self, agent_id: &str, message_id: &str) -> bool {
        let ctx = PermissionContext::new(agent_id.to_string(), crate::DEFAULT_TENANT.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return false;
        }
        self.message_bus.ack(agent_id, message_id)
    }

    /// Delegate a task from one agent to another.
    ///
    /// Submits an intent on the target agent's behalf and sends a delegation
    /// message so the target knows who requested the work.
    /// Returns (intent_id, message_id).
    pub fn delegate_task(
        &self,
        from: &str,
        to: &str,
        description: String,
        action: Option<String>,
        priority: IntentPriority,
    ) -> Result<(String, String), String> {
        let to_aid = AgentId(to.to_string());
        let target = self.scheduler.get(&to_aid)
            .ok_or_else(|| format!("Target agent not found: {}", to))?;
        if target.state().is_terminal() {
            return Err(format!("Target agent {} is in terminal state {:?}", to, target.state()));
        }

        let intent_id = self.submit_intent(
            priority,
            description.clone(),
            action.clone(),
            Some(to.to_string()),
        )?;

        let payload = serde_json::json!({
            "type": "delegation",
            "from": from,
            "intent_id": intent_id,
            "description": description,
        });
        let msg_id = self.send_message("kernel", to, payload)
            .map_err(|e| e.to_string())?;

        self.event_bus.emit(KernelEvent::IntentSubmitted {
            intent_id: intent_id.clone(),
            agent_id: Some(from.to_string()),
            priority: format!("{:?}", priority),
        });

        Ok((intent_id, msg_id))
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::permission::PermissionAction;

    #[test]
    fn test_send_message() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("sender".to_string()).unwrap();
        kernel.register_agent("receiver".to_string()).unwrap();
        kernel.permission_grant("sender", PermissionAction::SendMessage, None, None);
        let msg_id = kernel.send_message("sender", "receiver", serde_json::json!({"text": "hello"})).unwrap();
        assert!(!msg_id.is_empty());
    }

    #[test]
    fn test_read_messages() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("reader".to_string()).unwrap();
        kernel.permission_grant("kernel", PermissionAction::SendMessage, None, None);
        kernel.send_message("kernel", "reader", serde_json::json!({"text": "msg1"})).unwrap();
        let msgs = kernel.read_messages("reader", false);
        assert!(!msgs.is_empty());
    }

    #[test]
    fn test_read_messages_unread_only() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("r2".to_string()).unwrap();
        kernel.permission_grant("kernel", PermissionAction::SendMessage, None, None);
        kernel.send_message("kernel", "r2", serde_json::json!({"text": "msg1"})).unwrap();
        let unread = kernel.read_messages("r2", true);
        assert!(!unread.is_empty());
    }

    #[test]
    fn test_ack_message() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("acker".to_string()).unwrap();
        kernel.permission_grant("kernel", PermissionAction::SendMessage, None, None);
        let msg_id = kernel.send_message("kernel", "acker", serde_json::json!({})).unwrap();
        assert!(kernel.ack_message("acker", &msg_id));
    }

    #[test]
    fn test_ack_nonexistent_message() {
        let (kernel, _tmp) = make_kernel();
        assert!(!kernel.ack_message("test", "nonexistent-id"));
    }
}
