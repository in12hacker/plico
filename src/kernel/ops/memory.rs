//! Memory tier operations — ephemeral, working, long-term.

use crate::memory::{MemoryEntry, MemoryContent, MemoryTier};
use crate::scheduler::AgentId;

impl crate::kernel::AIKernel {
    fn agent_memory_quota(&self, agent_id: &str) -> u64 {
        self.scheduler
            .get_resources(&AgentId(agent_id.to_string()))
            .map(|r| r.memory_quota)
            .unwrap_or(0)
    }

    /// Store a memory entry in the agent's ephemeral (L0) tier.
    pub fn remember(&self, agent_id: &str, content: String) -> Result<(), String> {
        let entry = MemoryEntry::ephemeral(agent_id.to_string(), content);
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())
    }

    /// Store a memory entry in the agent's working (L1) tier.
    pub fn remember_working(&self, agent_id: &str, content: String, tags: Vec<String>) -> Result<(), String> {
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
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())
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

    /// Store a memory entry in the agent's long-term tier with semantic embedding.
    pub fn remember_long_term(
        &self,
        agent_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
    ) -> Result<(), String> {
        let embedding = self.embedding.embed(&content).ok();
        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content),
            importance,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding,
            ttl_ms: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())
    }

    /// Retrieve semantically relevant long-term memories for an agent.
    pub fn recall_semantic(
        &self,
        agent_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryEntry>, String> {
        let query_emb = self.embedding.embed(query).map_err(|e| e.to_string())?;
        let results = self.memory.recall_semantic(agent_id, &query_emb, k);
        Ok(results.into_iter().map(|(entry, _score)| entry).collect())
    }

    /// Retrieve relevant memories with semantic scoring, within token budget.
    pub fn recall_relevant_semantic(
        &self,
        agent_id: &str,
        query: &str,
        budget_tokens: usize,
    ) -> Vec<MemoryEntry> {
        match self.embedding.embed(query) {
            Ok(emb) => self.memory.recall_relevant_semantic(agent_id, &emb, budget_tokens),
            Err(_) => self.memory.recall_relevant(agent_id, budget_tokens),
        }
    }
}
