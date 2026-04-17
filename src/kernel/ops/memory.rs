//! Memory tier operations — ephemeral, working, long-term.

use crate::memory::{MemoryEntry, MemoryContent, MemoryTier};

impl crate::kernel::AIKernel {
    /// Store a memory entry in the agent's ephemeral (L0) tier.
    pub fn remember(&self, agent_id: &str, content: String) {
        let entry = MemoryEntry::ephemeral(agent_id.to_string(), content);
        self.memory.store(entry);
    }

    /// Store a memory entry in the agent's working (L1) tier.
    pub fn remember_working(&self, agent_id: &str, content: String, tags: Vec<String>) {
        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding: None,
            ttl_ms: None,
        };
        self.memory.store(entry);
    }

    /// Retrieve all entries from all tiers.
    pub fn recall(&self, agent_id: &str) -> Vec<MemoryEntry> {
        self.memory.get_all(agent_id)
    }

    /// Clear ephemeral (L0) memory only.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }

    /// Retrieve entries relevant to a query, within token budget.
    pub fn recall_relevant(&self, agent_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        self.memory.recall_relevant(agent_id, budget_tokens)
    }

    /// Evict expired entries from all tiers.
    pub fn evict_expired(&self, agent_id: &str) -> usize {
        self.memory.evict_expired(agent_id)
    }

    /// Check and promote entries between tiers if thresholds are met.
    pub fn promote_check(&self, agent_id: &str) {
        self.memory.promote_check(agent_id);
    }

    /// Move a memory entry to a different tier.
    ///
    /// Allows an agent to explicitly promote or demote a memory entry
    /// to a different tier. Returns `true` if the entry was found and moved.
    pub fn memory_move(&self, agent_id: &str, entry_id: &str, target_tier: MemoryTier) -> bool {
        self.memory.move_entry(agent_id, entry_id, target_tier)
    }

    /// Delete a specific memory entry by ID across all tiers.
    ///
    /// Returns `true` if the entry was found and deleted.
    pub fn memory_delete(&self, agent_id: &str, entry_id: &str) -> bool {
        self.memory.delete_entry(agent_id, entry_id)
    }
}
