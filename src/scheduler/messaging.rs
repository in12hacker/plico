//! Agent Messaging — kernel-mediated inter-agent communication.
//!
//! Each agent has a bounded mailbox. Messages are delivered through the kernel,
//! which enforces permission checks (sender must have SendMessage grant for the
//! target agent). Trusted agents can read any mailbox.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;

/// A message between two agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub from: String,
    pub to: String,
    pub payload: serde_json::Value,
    pub timestamp_ms: u64,
    pub read: bool,
}

/// Bounded mailbox for an agent.
struct Mailbox {
    messages: Vec<AgentMessage>,
    capacity: usize,
}

impl Mailbox {
    fn new(capacity: usize) -> Self {
        Self {
            messages: Vec::new(),
            capacity,
        }
    }

    fn push(&mut self, msg: AgentMessage) {
        if self.messages.len() >= self.capacity {
            self.messages.remove(0);
        }
        self.messages.push(msg);
    }

    fn read_all(&self, unread_only: bool) -> Vec<AgentMessage> {
        if unread_only {
            self.messages.iter().filter(|m| !m.read).cloned().collect()
        } else {
            self.messages.clone()
        }
    }

    fn ack(&mut self, message_id: &str) -> bool {
        if let Some(msg) = self.messages.iter_mut().find(|m| m.id == message_id) {
            msg.read = true;
            true
        } else {
            false
        }
    }
}

/// The message bus — manages all agent mailboxes.
pub struct MessageBus {
    mailboxes: RwLock<HashMap<String, Mailbox>>,
    default_capacity: usize,
}

impl MessageBus {
    pub fn new() -> Self {
        Self {
            mailboxes: RwLock::new(HashMap::new()),
            default_capacity: 100,
        }
    }

    pub fn send(&self, from: &str, to: &str, payload: serde_json::Value) -> String {
        let msg_id = uuid::Uuid::new_v4().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let msg = AgentMessage {
            id: msg_id.clone(),
            from: from.to_string(),
            to: to.to_string(),
            payload,
            timestamp_ms: now,
            read: false,
        };

        let mut mailboxes = self.mailboxes.write().unwrap();
        let mailbox = mailboxes
            .entry(to.to_string())
            .or_insert_with(|| Mailbox::new(self.default_capacity));
        mailbox.push(msg);
        msg_id
    }

    pub fn read(&self, agent_id: &str, unread_only: bool) -> Vec<AgentMessage> {
        let mailboxes = self.mailboxes.read().unwrap();
        mailboxes
            .get(agent_id)
            .map(|mb| mb.read_all(unread_only))
            .unwrap_or_default()
    }

    pub fn ack(&self, agent_id: &str, message_id: &str) -> bool {
        let mut mailboxes = self.mailboxes.write().unwrap();
        mailboxes
            .get_mut(agent_id)
            .map(|mb| mb.ack(message_id))
            .unwrap_or(false)
    }

    pub fn message_count(&self, agent_id: &str) -> usize {
        let mailboxes = self.mailboxes.read().unwrap();
        mailboxes.get(agent_id).map(|mb| mb.messages.len()).unwrap_or(0)
    }
}

impl Default for MessageBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_and_read() {
        let bus = MessageBus::new();
        let id = bus.send("agent-a", "agent-b", serde_json::json!({"task": "summarize"}));
        assert!(!id.is_empty());

        let msgs = bus.read("agent-b", false);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].from, "agent-a");
        assert!(!msgs[0].read);
    }

    #[test]
    fn test_unread_only() {
        let bus = MessageBus::new();
        bus.send("a", "b", serde_json::json!("msg1"));
        let id2 = bus.send("a", "b", serde_json::json!("msg2"));

        bus.ack("b", &id2);

        let unread = bus.read("b", true);
        assert_eq!(unread.len(), 1);

        let all = bus.read("b", false);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_capacity_eviction() {
        let bus = MessageBus {
            mailboxes: RwLock::new(HashMap::new()),
            default_capacity: 3,
        };
        bus.send("a", "b", serde_json::json!(1));
        bus.send("a", "b", serde_json::json!(2));
        bus.send("a", "b", serde_json::json!(3));
        bus.send("a", "b", serde_json::json!(4));

        let msgs = bus.read("b", false);
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[0].payload, serde_json::json!(2));
    }

    #[test]
    fn test_ack_marks_read() {
        let bus = MessageBus::new();
        let id = bus.send("a", "b", serde_json::json!("hello"));
        assert!(bus.ack("b", &id));

        let msgs = bus.read("b", false);
        assert!(msgs[0].read);
    }
}
