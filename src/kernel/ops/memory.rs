//! Memory tier operations — ephemeral, working, long-term.

use crate::api::permission::{PermissionAction, PermissionContext};
use crate::memory::{MemoryEntry, MemoryContent, MemoryTier, MemoryScope};
use crate::scheduler::AgentId;
use crate::kernel::event_bus::KernelEvent;

impl crate::kernel::AIKernel {
    fn agent_memory_quota(&self, agent_id: &str) -> u64 {
        self.scheduler
            .get_resources(&AgentId(agent_id.to_string()))
            .map(|r| r.memory_quota)
            .unwrap_or(0)
    }

    /// Store a memory entry in the agent's ephemeral (L0) tier.
    pub fn remember(&self, agent_id: &str, tenant_id: &str, content: String) -> Result<(), String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let mut entry = MemoryEntry::ephemeral(agent_id.to_string(), content);
        entry.tenant_id = tenant_id.to_string();
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())
    }

    /// Store a memory entry in the agent's working (L1) tier.
    pub fn remember_working(&self, agent_id: &str, tenant_id: &str, content: String, tags: Vec<String>) -> Result<(), String> {
        self.remember_working_scoped(agent_id, tenant_id, content, tags, MemoryScope::Private)
    }

    /// Store a working memory entry with explicit scope.
    pub fn remember_working_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        scope: MemoryScope,
    ) -> Result<(), String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding: None,
            ttl_ms: None,
            scope,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "working".into(),
        });
        self.persist_memories();
        Ok(())
    }

    /// Retrieve all entries from all tiers (filtered by tenant).
    pub fn recall(&self, agent_id: &str, tenant_id: &str) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        self.memory.get_all(agent_id)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
    }

    /// Retrieve all entries visible to an agent (own + shared + group, filtered by tenant).
    pub fn recall_visible(&self, agent_id: &str, tenant_id: &str, groups: &[String]) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        self.memory.recall_visible(agent_id, groups)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
    }

    /// Clear ephemeral (L0) memory only.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }

    /// Retrieve entries relevant to a query, within token budget.
    pub fn recall_relevant(&self, agent_id: &str, tenant_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        self.memory.recall_relevant(agent_id, budget_tokens)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
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
    pub fn memory_move(&self, agent_id: &str, _tenant_id: &str, entry_id: &str, target_tier: MemoryTier) -> bool {
        let moved = self.memory.move_entry(agent_id, entry_id, target_tier);
        if moved { self.persist_memories(); }
        moved
    }

    /// Delete a specific memory entry by ID across all tiers.
    pub fn memory_delete(&self, agent_id: &str, _tenant_id: &str, entry_id: &str) -> bool {
        let deleted = self.memory.delete_entry(agent_id, entry_id);
        if deleted { self.persist_memories(); }
        deleted
    }

    /// Store a memory entry in the agent's long-term tier with semantic embedding.
    pub fn remember_long_term(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
    ) -> Result<(), String> {
        self.remember_long_term_scoped(agent_id, tenant_id, content, tags, importance, MemoryScope::Private)
    }

    /// Store a long-term memory entry with explicit scope.
    pub fn remember_long_term_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
        scope: MemoryScope,
    ) -> Result<(), String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let embedding = self.embedding.embed(&content).ok();
        let entry = MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content),
            importance,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding,
            ttl_ms: None,
            scope,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota)
            .map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "long_term".into(),
        });
        self.persist_memories();
        Ok(())
    }

    /// Retrieve semantically relevant long-term memories for an agent.
    pub fn recall_semantic(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<MemoryEntry>, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read).map_err(|e| e.to_string())?;
        let query_emb = self.embedding.embed(query).map_err(|e| e.to_string())?;
        let results = self.memory.recall_semantic(agent_id, &query_emb, k);
        Ok(results.into_iter()
            .map(|(entry, _score)| entry)
            .filter(|e| e.tenant_id == tenant_id)
            .collect())
    }

    /// Retrieve relevant memories with semantic scoring, within token budget.
    pub fn recall_relevant_semantic(
        &self,
        agent_id: &str,
        tenant_id: &str,
        query: &str,
        budget_tokens: usize,
    ) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let tenant_id_owned = tenant_id.to_string();
        match self.embedding.embed(query) {
            Ok(emb) => self.memory.recall_relevant_semantic(agent_id, &emb, budget_tokens)
                .into_iter()
                .filter(|e| e.tenant_id == tenant_id_owned)
                .collect(),
            Err(_) => self.memory.recall_relevant(agent_id, budget_tokens)
                .into_iter()
                .filter(|e| e.tenant_id == tenant_id_owned)
                .collect(),
        }
    }

    /// Store a procedural memory entry (L3 tier — learned skills/workflows).
    pub fn remember_procedural(
        &self,
        agent_id: &str,
        tenant_id: &str,
        name: String,
        description: String,
        steps: Vec<crate::memory::layered::ProcedureStep>,
        learned_from: String,
        tags: Vec<String>,
    ) -> Result<String, String> {
        self.remember_procedural_scoped(agent_id, tenant_id, name, description, steps, learned_from, tags, MemoryScope::Private)
    }

    /// Store a procedural memory entry with explicit scope.
    pub fn remember_procedural_scoped(
        &self,
        agent_id: &str,
        tenant_id: &str,
        name: String,
        description: String,
        steps: Vec<crate::memory::layered::ProcedureStep>,
        learned_from: String,
        tags: Vec<String>,
        scope: MemoryScope,
    ) -> Result<String, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;
        let procedure = crate::memory::layered::Procedure {
            name,
            description,
            steps,
            learned_from,
        };
        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = MemoryEntry {
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Procedural,
            content: MemoryContent::Procedure(procedure),
            importance: 100,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags,
            embedding: None,
            ttl_ms: None,
            scope,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory.store_checked(entry, quota).map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "procedural".into(),
        });
        self.persist_memories();
        Ok(entry_id)
    }

    /// Recall procedural memories, optionally filtered by procedure name.
    pub fn recall_procedural(&self, agent_id: &str, tenant_id: &str, name_filter: Option<&str>) -> Vec<MemoryEntry> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let entries = self.memory.get_tier(agent_id, MemoryTier::Procedural);
        let tenant_id_owned = tenant_id.to_string();
        entries.into_iter()
            .filter(|e| {
                // Tenant isolation
                if e.tenant_id != tenant_id_owned {
                    return false;
                }
                match name_filter {
                    None => true,
                    Some(name) => matches!(&e.content, MemoryContent::Procedure(p) if p.name == name),
                }
            })
            .collect()
    }

    /// Recall shared procedural memories from all agents.
    pub fn recall_shared_procedural(&self, name_filter: Option<&str>) -> Vec<MemoryEntry> {
        let entries = self.memory.get_shared(MemoryTier::Procedural);
        match name_filter {
            None => entries,
            Some(name) => entries.into_iter().filter(|e| {
                matches!(&e.content, MemoryContent::Procedure(p) if p.name == name)
            }).collect(),
        }
    }
}
