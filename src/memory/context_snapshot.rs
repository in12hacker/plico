//! Agent Context Snapshot — cognitive continuity across suspend/resume.
//!
//! When an agent is suspended, the kernel automatically captures a snapshot
//! of its cognitive state (what it was doing, what memories were active,
//! how many intents were pending). On resume, this snapshot is loaded into
//! the agent's Ephemeral memory as the first piece of context.
//!
//! This gives AI agents continuity: they can pick up where they left off.

use serde::{Deserialize, Serialize};
use super::layered::{MemoryEntry, MemoryTier, MemoryContent};

/// Internal tag used to identify context snapshots in memory.
pub const SNAPSHOT_TAG: &str = "plico:internal:snapshot";

/// Captures an agent's cognitive state at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextSnapshot {
    pub agent_id: String,
    pub timestamp_ms: u64,
    pub state_before_suspend: String,
    pub pending_intents: usize,
    pub active_memory_count: usize,
    pub last_intent_description: Option<String>,
}

impl ContextSnapshot {
    /// Convert this snapshot into a Working-tier memory entry for persistence.
    pub fn to_memory_entry(&self) -> MemoryEntry {
        let now = crate::memory::layered::now_ms();
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: self.agent_id.clone(),
            tenant_id: crate::DEFAULT_TENANT.to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Structured(serde_json::to_value(self).unwrap_or_default()),
            importance: 80,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: vec![SNAPSHOT_TAG.to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: crate::memory::layered::MemoryScope::Private,
            memory_type: crate::memory::layered::MemoryType::Episodic,
        }
    }

    /// Create a human-readable summary for loading into Ephemeral context.
    pub fn to_context_string(&self) -> String {
        let mut ctx = format!(
            "Context restored: agent={}, suspended at timestamp={}, state_before={}",
            self.agent_id, self.timestamp_ms, self.state_before_suspend,
        );
        ctx.push_str(&format!(", pending_intents={}", self.pending_intents));
        ctx.push_str(&format!(", active_memories={}", self.active_memory_count));
        if let Some(ref desc) = self.last_intent_description {
            ctx.push_str(&format!(", last_task=\"{}\"", desc));
        }
        ctx
    }
}

/// Find the most recent context snapshot for an agent from their memories.
pub fn find_latest_snapshot(entries: &[MemoryEntry]) -> Option<ContextSnapshot> {
    entries.iter()
        .filter(|e| e.tags.contains(&SNAPSHOT_TAG.to_string()))
        .max_by_key(|e| e.created_at)
        .and_then(|e| {
            if let MemoryContent::Structured(ref v) = e.content {
                serde_json::from_value::<ContextSnapshot>(v.clone()).ok()
            } else {
                None
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_roundtrip() {
        let snap = ContextSnapshot {
            agent_id: "agent-1".into(),
            timestamp_ms: 1234567890,
            state_before_suspend: "Running".into(),
            pending_intents: 2,
            active_memory_count: 5,
            last_intent_description: Some("process batch data".into()),
        };

        let entry = snap.to_memory_entry();
        assert_eq!(entry.tier, MemoryTier::Working);
        assert!(entry.tags.contains(&SNAPSHOT_TAG.to_string()));
        assert_eq!(entry.importance, 80);

        let recovered = find_latest_snapshot(&[entry]).unwrap();
        assert_eq!(recovered.agent_id, "agent-1");
        assert_eq!(recovered.pending_intents, 2);
        assert_eq!(recovered.last_intent_description.as_deref(), Some("process batch data"));
    }

    #[test]
    fn context_string_contains_key_info() {
        let snap = ContextSnapshot {
            agent_id: "a".into(),
            timestamp_ms: 999,
            state_before_suspend: "Waiting".into(),
            pending_intents: 1,
            active_memory_count: 3,
            last_intent_description: None,
        };
        let ctx = snap.to_context_string();
        assert!(ctx.contains("agent=a"));
        assert!(ctx.contains("pending_intents=1"));
    }

    #[test]
    fn find_latest_picks_most_recent() {
        let snap1 = ContextSnapshot {
            agent_id: "a".into(),
            timestamp_ms: 100,
            state_before_suspend: "Running".into(),
            pending_intents: 0,
            active_memory_count: 1,
            last_intent_description: None,
        };
        let snap2 = ContextSnapshot {
            agent_id: "a".into(),
            timestamp_ms: 200,
            state_before_suspend: "Waiting".into(),
            pending_intents: 3,
            active_memory_count: 5,
            last_intent_description: Some("latest".into()),
        };

        let mut e1 = snap1.to_memory_entry();
        e1.created_at = 100;
        let mut e2 = snap2.to_memory_entry();
        e2.created_at = 200;

        let latest = find_latest_snapshot(&[e1, e2]).unwrap();
        assert_eq!(latest.pending_intents, 3);
    }
}
