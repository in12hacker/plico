//! Agent-to-agent messaging operations.

use crate::api::permission::{PermissionContext, PermissionAction};

impl crate::kernel::AIKernel {
    /// Send a message from one agent to another.
    pub fn send_message(
        &self,
        from: &str,
        to: &str,
        payload: serde_json::Value,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(from.to_string());
        self.permissions.check(&ctx, PermissionAction::SendMessage)?;
        let msg_id = self.message_bus.send(from, to, payload);
        Ok(msg_id)
    }

    /// Read messages for an agent.
    pub fn read_messages(&self, agent_id: &str, unread_only: bool) -> Vec<crate::scheduler::messaging::AgentMessage> {
        self.message_bus.read(agent_id, unread_only)
    }

    /// Acknowledge (mark as read) a message.
    pub fn ack_message(&self, agent_id: &str, message_id: &str) -> bool {
        self.message_bus.ack(agent_id, message_id)
    }
}
