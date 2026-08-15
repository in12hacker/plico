//! Memory operations — runtime-only ephemeral context and canonical durable tiers.

use crate::api::permission::{PermissionAction, PermissionContext};
use crate::memory::{DurableMemoryMutationError, MemoryContent, MemoryEntry, MemoryScope, MemoryTier, MemoryType};
use crate::scheduler::AgentId;

use super::observability::{OpType, OperationTimer};
use crate::kernel::event_bus::KernelEvent;

/// Bundled parameters for storing a procedural memory entry.
pub struct ProceduralEntry {
    pub name: String,
    pub description: String,
    pub steps: Vec<crate::memory::layered::ProcedureStep>,
    pub learned_from: String,
    pub tags: Vec<String>,
}

fn role_kind(role_id: &str) -> &'static str {
    if role_id == crate::PERSONAL_OWNER_ROLE_ID {
        "personal_owner"
    } else {
        "agent_role"
    }
}

impl crate::kernel::AIKernel {
    pub(crate) fn agent_memory_quota(&self, agent_id: &str) -> u64 {
        self.scheduler
            .get_resources(&AgentId(agent_id.to_string()))
            .map(|r| r.memory_quota)
            .unwrap_or(0)
    }

    /// Store a memory entry in the agent's ephemeral (L0) tier.
    /// Returns the entry ID on success.
    pub fn remember(&self, agent_id: &str, tenant_id: &str, content: String) -> Result<String, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions
            .check(&ctx, PermissionAction::Write)
            .map_err(|e| e.to_string())?;
        let entry_id = uuid::Uuid::new_v4().to_string();
        let now = crate::memory::layered::now_ms();
        let entry = MemoryEntry {
            memory_id: Default::default(),
            parent_revision_id: None,
            canonical_content_hash: Default::default(),
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Ephemeral,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        self.memory
            .store_checked(entry.clone(), quota)
            .map_err(|e| e.to_string())?;
        Ok(entry_id)
    }

    /// Store a memory entry in the agent's working (L1) tier.
    pub fn remember_working(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        self.remember_working_with_request_id(agent_id, tenant_id, content, tags, None)
    }

    /// Store canonical working memory and return its stable entry ID.
    ///
    /// Vector generation is eventual: a successful return guarantees the
    /// canonical entry is stored and persisted, not that its embedding is ready.
    #[cfg(test)]
    pub(crate) fn remember_working_with_id(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        self.remember_working_with_request_id(agent_id, tenant_id, content, tags, None)
    }

    pub(crate) fn remember_working_with_request_id(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        request_id: Option<uuid::Uuid>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberWorking);
        let span = tracing::info_span!(
            "remember_working",
            operation = "memory.create",
            role_kind = role_kind(agent_id),
            tag_count = tags.len(),
        );
        let _guard = span.enter();

        let permission = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions
            .check(&permission, PermissionAction::Write)
            .map_err(|_| DurableMemoryMutationError::PermissionDenied)?;

        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = MemoryEntry {
            memory_id: Default::default(),
            parent_revision_id: None,
            canonical_content_hash: Default::default(),
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text(content),
            importance: 50,
            access_count: 0,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: crate::memory::layered::now_ms(),
            tags: tags.clone(),
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        let entry = self.memory.create_working_durable(entry, quota)?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "working".into(),
        });
        self.projection.notify_current(&entry, request_id);
        tracing::info!(tag_count = tags.len(), "working memory stored");
        Ok(entry)
    }

    /// Retrieve all entries from all tiers (filtered by tenant).
    pub fn recall(&self, agent_id: &str, tenant_id: &str) -> Vec<MemoryEntry> {
        let _timer = OperationTimer::new(&self.metrics, OpType::Recall);
        let span = tracing::info_span!("recall", operation = "recall", role_kind = role_kind(agent_id),);
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return Vec::new();
        }
        let entries: Vec<MemoryEntry> = self
            .memory
            .get_active(agent_id)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect();
        tracing::info!(count = entries.len(), "memories recalled");
        entries
    }

    /// Clear ephemeral (L0) memory only.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }

    /// Retrieve entries relevant to a query, within token budget.
    pub fn recall_relevant(&self, agent_id: &str, tenant_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        self.memory
            .recall_relevant(agent_id, budget_tokens)
            .into_iter()
            .filter(|e| e.tenant_id == tenant_id)
            .collect()
    }

    /// Durably append a tombstone when the current policy authorizes the caller.
    pub(crate) fn memory_delete(
        &self,
        role_id: &str,
        namespace: &str,
        entry_id: &str,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        self.memory_delete_with_request_id(role_id, namespace, entry_id, None)
    }

    pub(crate) fn memory_delete_with_request_id(
        &self,
        role_id: &str,
        namespace: &str,
        entry_id: &str,
        request_id: Option<uuid::Uuid>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        let entry = self.memory.delete_working_durable(role_id, namespace, entry_id)?;
        self.projection.notify_current(&entry, request_id);
        Ok(entry)
    }

    /// Update a memory by appending a child revision to the immutable stream.
    pub fn memory_update(
        &self,
        role_id: &str,
        namespace: &str,
        entry_id: &str,
        new_content: String,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        self.memory_update_with_request_id(role_id, namespace, entry_id, new_content, None)
    }

    pub(crate) fn memory_update_with_request_id(
        &self,
        role_id: &str,
        namespace: &str,
        entry_id: &str,
        new_content: String,
        request_id: Option<uuid::Uuid>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        let span = tracing::info_span!(
            "memory_update_pipeline",
            operation = "memory.update",
            role_kind = role_kind(role_id),
            entry_id = %entry_id,
            previous_revision_id = %entry_id,
            new_revision_id = tracing::field::Empty,
        );
        let _guard = span.enter();
        let new_entry = self
            .memory
            .update_working_durable(role_id, namespace, entry_id, new_content)?;
        span.record("new_revision_id", tracing::field::display(&new_entry.id));
        self.projection.notify_current(&new_entry, request_id);
        Ok(new_entry)
    }

    /// Commit a long-term canonical memory, then request derived indexing.
    pub fn remember_long_term(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
    ) -> Result<String, String> {
        self.remember_long_term_private(agent_id, tenant_id, content, tags, importance)
    }

    /// Store a confirmed action as a long-term memory with equal weight.
    ///
    /// Every confirmed agent action is committed as a distinct canonical
    /// memory with equal importance (50). Similarity is a derived projection
    /// and never suppresses or mutates the canonical write.
    pub fn remember_action(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
    ) -> Result<String, String> {
        self.remember_long_term_private(agent_id, tenant_id, content, tags, 50)
    }

    /// Store a long-term memory entry with explicit scope.
    /// Returns the entry ID on success.
    fn remember_long_term_private(
        &self,
        agent_id: &str,
        tenant_id: &str,
        content: String,
        tags: Vec<String>,
        importance: u8,
    ) -> Result<String, String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberLongTerm);
        let span = tracing::info_span!(
            "remember_long_term",
            operation = "remember_long_term",
            role_kind = role_kind(agent_id),
            importance = importance,
            tag_count = tags.len(),
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions
            .check(&ctx, PermissionAction::Write)
            .map_err(|e| e.to_string())?;
        let entry_id = uuid::Uuid::new_v4().to_string();
        let created_at = crate::memory::layered::now_ms();
        let entry = MemoryEntry {
            memory_id: Default::default(),
            parent_revision_id: None,
            canonical_content_hash: Default::default(),
            id: entry_id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content),
            importance,
            access_count: 0,
            last_accessed: created_at,
            created_at,
            tags: tags.clone(),
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::default(),
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        let stored = self.memory.create_durable(entry, quota).map_err(|e| e.to_string())?;
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "long_term".into(),
        });
        self.projection.notify_current(&stored, None);

        tracing::info!(
            tag_count = tags.len(),
            importance = importance,
            "long-term memory stored"
        );
        Ok(entry_id)
    }

    /// Atomically commit multiple long-term canonical roots before requesting
    /// any derived embedding work.
    pub fn remember_long_term_batch(
        &self,
        agent_id: &str,
        tenant_id: &str,
        items: &[(String, Vec<String>, u8)],
    ) -> Result<Vec<String>, String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::RememberLongTerm);
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions
            .check(&ctx, PermissionAction::Write)
            .map_err(|e| e.to_string())?;

        let created_at = crate::memory::layered::now_ms();
        let quota = self.agent_memory_quota(agent_id);
        let mut ids = Vec::with_capacity(items.len());

        let mut entries = Vec::with_capacity(items.len());
        for (content, tags, importance) in items {
            let entry_id = uuid::Uuid::new_v4().to_string();
            let entry = MemoryEntry {
                memory_id: Default::default(),
                parent_revision_id: None,
                canonical_content_hash: Default::default(),
                id: entry_id.clone(),
                agent_id: agent_id.to_string(),
                tenant_id: tenant_id.to_string(),
                tier: MemoryTier::LongTerm,
                content: MemoryContent::Text(content.clone()),
                importance: *importance,
                access_count: 0,
                last_accessed: created_at,
                created_at,
                tags: tags.clone(),
                ttl_ms: None,
                original_ttl_ms: None,
                scope: MemoryScope::Private,
                memory_type: MemoryType::default(),
                causal_parent: None,
                supersedes: None,
                superseded_by: None,
                deleted_at: None,
            };
            entries.push(entry);
            ids.push(entry_id);
        }
        let stored = self
            .memory
            .create_batch_durable(entries, quota)
            .map_err(|e| e.to_string())?;
        for entry in stored {
            self.event_bus.emit(KernelEvent::MemoryStored {
                agent_id: agent_id.to_string(),
                tier: "long_term".into(),
            });
            self.projection.notify_current(&entry, None);
        }

        tracing::info!(count = items.len(), "batch long-term memory stored");
        Ok(ids)
    }

    /// Store a procedural memory entry (L3 tier — learned skills/workflows).
    pub fn remember_procedural(
        &self,
        agent_id: &str,
        tenant_id: &str,
        entry: ProceduralEntry,
    ) -> Result<String, String> {
        let ProceduralEntry {
            name,
            description,
            steps,
            learned_from,
            tags,
        } = entry;
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions
            .check(&ctx, PermissionAction::Write)
            .map_err(|e| e.to_string())?;
        let procedure = crate::memory::layered::Procedure {
            name,
            description,
            steps,
            learned_from,
        };
        let entry_id = uuid::Uuid::new_v4().to_string();
        let entry = MemoryEntry {
            memory_id: Default::default(),
            parent_revision_id: None,
            canonical_content_hash: Default::default(),
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
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Procedural,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        };
        let quota = self.agent_memory_quota(agent_id);
        let entry = self.memory.create_durable(entry, quota).map_err(|e| e.to_string())?;
        self.projection.notify_current(&entry, None);
        self.event_bus.emit(KernelEvent::MemoryStored {
            agent_id: agent_id.to_string(),
            tier: "procedural".into(),
        });
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
        entries
            .into_iter()
            .filter(|e| {
                // Legacy personal-vault namespace filter.
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

    /// Compute memory usage statistics for an agent's tier(s).
    ///
    /// If `tier` is Some, stats are computed only for that tier.
    /// If `tier` is None, stats aggregate all tiers.
    pub fn memory_stats(&self, agent_id: &str, tier: Option<&MemoryTier>) -> crate::api::semantic::MemoryStatsResult {
        use crate::api::semantic::MemoryStatsResult;
        use crate::memory::layered::now_ms;

        let now = now_ms();
        let tiers: Vec<MemoryTier> = match tier {
            Some(t) => vec![*t],
            None => vec![
                MemoryTier::Ephemeral,
                MemoryTier::Working,
                MemoryTier::LongTerm,
                MemoryTier::Procedural,
            ],
        };

        let mut total_entries = 0;
        let mut total_bytes = 0usize;
        let mut oldest_entry_age_ms: u64 = 0;
        let mut total_access_count = 0u64;
        let mut never_accessed_count = 0;
        let mut about_to_expire_count = 0;

        for t in &tiers {
            let entries = self.memory.get_tier(agent_id, *t);
            for entry in entries {
                total_entries += 1;
                total_bytes += entry.content.display().len(); // rough byte estimate

                let age_ms = now.saturating_sub(entry.created_at);
                if age_ms > oldest_entry_age_ms {
                    oldest_entry_age_ms = age_ms;
                }

                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }

                // Check if entry is about to expire (within 10% of TTL)
                if let Some(ttl) = entry.ttl_ms {
                    let remaining = entry.created_at.saturating_add(ttl).saturating_sub(now);
                    if ttl > 0 && remaining < ttl / 10 {
                        about_to_expire_count += 1;
                    }
                }
            }
        }

        let avg_access_count = if total_entries > 0 {
            total_access_count as f32 / total_entries as f32
        } else {
            0.0
        };

        MemoryStatsResult {
            agent_id: agent_id.to_string(),
            tier: tier.map(|t| t.name().to_string()).unwrap_or_default(),
            total_entries,
            total_bytes,
            oldest_entry_age_ms,
            avg_access_count,
            never_accessed_count,
            about_to_expire_count,
        }
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remember_ephemeral_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember("kernel", "default", "ephemeral thought".to_string());
        assert!(id.is_ok());
    }

    #[test]
    fn test_remember_long_term_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel.remember_long_term(
            "kernel",
            "default",
            "important fact".to_string(),
            vec!["fact".to_string()],
            80,
        );
        assert!(id.is_ok());
    }

    #[test]
    fn test_memory_recall_basic() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel
            .remember("kernel", "default", "recallable memory".to_string())
            .ok();

        let entries = kernel.recall("kernel", "default");
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_memory_recall_empty_query() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "test".to_string()).ok();
        let entries = kernel.recall("kernel", "default");
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_memory_count_via_agent_usage() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let agent_id = kernel.register_agent("usage-agent".to_string()).unwrap();
        kernel.remember(&agent_id, "default", "count me".to_string()).ok();
        let usage = kernel.agent_usage(&agent_id);
        assert!(usage.is_some());
        assert!(usage.unwrap().memory_entries >= 1);
    }

    #[test]
    fn test_remember_working() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_working(
            "kernel",
            "default",
            "working memory".to_string(),
            vec!["tag1".to_string()],
        );
        assert!(result.is_ok());
        let entries = kernel.recall("kernel", "default");
        assert!(entries
            .iter()
            .any(|e| e.content.display().to_string().contains("working memory")));
    }

    #[test]
    fn test_forget_ephemeral() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "ephemeral stuff".to_string()).ok();
        kernel.forget_ephemeral("kernel");
        // After forgetting, ephemeral entries should be cleared
        let entries = kernel.recall("kernel", "default");
        assert!(entries.iter().all(|e| e.tier != MemoryTier::Ephemeral));
    }

    #[test]
    fn test_memory_delete() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel
            .remember_working_with_id("kernel", "default", "deletable".to_string(), vec![])
            .unwrap()
            .id;
        kernel.memory_delete("kernel", "default", &id).unwrap();
    }

    #[test]
    fn test_memory_delete_nonexistent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let deleted = kernel.memory_delete("kernel", "default", "nonexistent-id");
        assert!(matches!(deleted, Err(DurableMemoryMutationError::NotFound { .. })));
    }

    #[test]
    fn test_remember_action() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_action(
            "kernel",
            "default",
            "ran a command".to_string(),
            vec!["action".to_string()],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_remember_long_term_batch() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let contents = vec![
            ("fact 1".to_string(), vec!["fact".to_string()], 70u8),
            ("fact 2".to_string(), vec!["fact".to_string()], 80u8),
            ("fact 3".to_string(), vec!["fact".to_string()], 60u8),
        ];
        let result = kernel.remember_long_term_batch("kernel", "default", &contents);
        assert!(result.is_ok());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 3);
        assert!(ids.iter().all(|id| !id.is_empty()));
    }

    #[test]
    fn long_term_similarity_never_suppresses_canonical_commits() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let content = "same observed fact".to_string();

        let first = kernel
            .remember_long_term("kernel", "default", content.clone(), vec![], 50)
            .expect("first canonical long-term commit should succeed");
        let second = kernel
            .remember_long_term("kernel", "default", content, vec![], 50)
            .expect("second canonical long-term commit should succeed");

        assert_ne!(first, second, "each observation needs its own revision root");
        let matching = kernel
            .recall("kernel", "default")
            .into_iter()
            .filter(|entry| entry.tier == MemoryTier::LongTerm && entry.content.display() == "same observed fact")
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 2, "derived similarity must not collapse facts");
        assert_ne!(matching[0].memory_id, matching[1].memory_id);
    }

    #[test]
    fn test_recall_relevant() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel
            .remember_long_term(
                "kernel",
                "default",
                "relevant fact".to_string(),
                vec!["test".to_string()],
                90,
            )
            .ok();
        let entries = kernel.recall_relevant("kernel", "default", 1000);
        // Should return some entries within budget
        let _ = entries;
    }

    #[test]
    fn test_remember_procedural() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "test_proc".into(),
            description: "a procedure".into(),
            steps: vec![crate::memory::layered::ProcedureStep {
                step_number: 1,
                description: "step 1".into(),
                action: "do thing".into(),
                expected_outcome: "done".into(),
            }],
            learned_from: "test".into(),
            tags: vec!["proc".into()],
        };
        let result = kernel.remember_procedural("kernel", "default", proc);
        assert!(result.is_ok());
    }

    #[test]
    fn test_recall_procedural() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "recall_proc".into(),
            description: "recallable".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", None);
        assert!(!entries.is_empty());
    }

    #[test]
    fn test_recall_procedural_by_name() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "named_proc".into(),
            description: "findable by name".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", Some("named_proc"));
        assert!(!entries.is_empty());
        assert!(entries.iter().any(|e| e.tags.iter().any(|t| t.contains("named_proc"))
            || matches!(&e.content, MemoryContent::Procedure(p) if p.name == "named_proc")));
    }

    #[test]
    fn test_agent_memory_quota() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.register_agent("quota_agent".to_string()).unwrap();
        let quota = kernel.agent_memory_quota("quota_agent");
        // Default quota should be some value
        let _ = quota;
    }

    #[test]
    fn test_agent_memory_quota_unregistered() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Unregistered agent should return 0 quota
        let quota = kernel.agent_memory_quota("nonexistent-agent");
        assert_eq!(quota, 0);
    }

    // ─── recall_procedural with filter ────────────────────────────────────

    #[test]
    fn test_recall_procedural_name_filter_no_match() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let proc = ProceduralEntry {
            name: "deploy_flow".into(),
            description: "deploy steps".into(),
            steps: vec![],
            learned_from: "test".into(),
            tags: vec![],
        };
        kernel.remember_procedural("kernel", "default", proc).unwrap();
        let entries = kernel.recall_procedural("kernel", "default", Some("nonexistent_proc"));
        assert!(entries.is_empty(), "Non-matching name filter should return empty");
    }

    #[test]
    fn test_recall_procedural_multiple_entries() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        for name in &["proc_a", "proc_b", "proc_c"] {
            let proc = ProceduralEntry {
                name: (*name).into(),
                description: format!("desc for {}", name),
                steps: vec![],
                learned_from: "test".into(),
                tags: vec![],
            };
            kernel.remember_procedural("kernel", "default", proc).unwrap();
        }
        let all = kernel.recall_procedural("kernel", "default", None);
        assert_eq!(all.len(), 3, "Should return all 3 procedural entries");
        let filtered = kernel.recall_procedural("kernel", "default", Some("proc_b"));
        assert_eq!(filtered.len(), 1);
        match &filtered[0].content {
            MemoryContent::Procedure(p) => assert_eq!(p.name, "proc_b"),
            _ => panic!("Expected Procedure content"),
        }
    }

    // ─── memory_stats ─────────────────────────────────────────────────────

    #[test]
    fn test_memory_stats_all_tiers() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // Store entries in different tiers
        kernel
            .remember("kernel", "default", "ephemeral data".to_string())
            .unwrap();
        kernel
            .remember_working("kernel", "default", "working data".to_string(), vec!["w".into()])
            .unwrap();
        kernel
            .remember_long_term("kernel", "default", "long term data".to_string(), vec!["lt".into()], 80)
            .unwrap();

        let stats = kernel.memory_stats("kernel", None);
        assert!(
            stats.total_entries >= 3,
            "Should have at least 3 entries across tiers, got {}",
            stats.total_entries
        );
        assert!(stats.total_bytes > 0, "Should have non-zero byte count");
        assert_eq!(stats.agent_id, "kernel");
        assert!(stats.tier.is_empty(), "Aggregate stats should have empty tier name");
    }

    #[test]
    fn test_memory_stats_specific_tier() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "eph one".to_string()).unwrap();
        kernel.remember("kernel", "default", "eph two".to_string()).unwrap();

        let stats = kernel.memory_stats("kernel", Some(&MemoryTier::Ephemeral));
        assert_eq!(stats.total_entries, 2, "Should have 2 ephemeral entries");
        assert_eq!(stats.tier, "ephemeral");
    }

    #[test]
    fn test_memory_stats_empty_agent() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let stats = kernel.memory_stats("empty_agent", None);
        assert_eq!(stats.total_entries, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.oldest_entry_age_ms, 0);
        assert_eq!(stats.avg_access_count, 0.0);
        assert_eq!(stats.never_accessed_count, 0);
        assert_eq!(stats.about_to_expire_count, 0);
    }

    #[test]
    fn test_memory_stats_never_accessed_count() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.remember("kernel", "default", "fresh entry".to_string()).unwrap();

        let stats = kernel.memory_stats("kernel", Some(&MemoryTier::Ephemeral));
        assert_eq!(
            stats.never_accessed_count, stats.total_entries,
            "Newly created entries should all be never-accessed"
        );
    }

    // ─── remember_long_term_batch edge cases ──────────────────────────────

    #[test]
    fn test_remember_long_term_batch_empty() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let result = kernel.remember_long_term_batch("kernel", "default", &[]);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty(), "Empty batch should return empty vec");
    }

    #[test]
    fn test_remember_long_term_batch_single_item() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let items = vec![("single fact".to_string(), vec!["tag".to_string()], 65u8)];
        let result = kernel.remember_long_term_batch("kernel", "default", &items);
        assert!(result.is_ok());
        let ids = result.unwrap();
        assert_eq!(ids.len(), 1);
    }

    // Recall filtering for the legacy personal-vault namespace field.

    #[test]
    fn test_recall_legacy_namespace_filtering() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel
            .remember("kernel", "legacy-a", "legacy namespace data".to_string())
            .unwrap();
        kernel.recall("kernel", "legacy-a");

        let entries_b = kernel.recall("kernel", "legacy-b");
        assert!(
            entries_b.iter().all(|e| e.tenant_id != "legacy-a"),
            "legacy namespaces remain isolated during compatibility reads"
        );
    }

    #[test]
    fn test_soft_delete() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel
            .remember_working_with_id("kernel", "default", "to be deleted".to_string(), vec![])
            .unwrap()
            .id;

        // Before delete: visible in recall
        let entries = kernel.recall("kernel", "default");
        assert!(entries.iter().any(|e| e.id == id));

        // Soft-delete
        let deleted = kernel.memory_delete("kernel", "default", &id).unwrap();

        // After delete: not visible in recall
        let entries = kernel.recall("kernel", "default");
        assert!(!entries.iter().any(|e| e.id == id));

        // But still in get_all (physical retention)
        let all = kernel.memory.get_all("kernel");
        assert!(all.iter().any(|e| e.id == id && e.deleted_at.is_none()));
        assert!(all.iter().any(|e| e.id == deleted.id && e.deleted_at.is_some()));
    }

    #[test]
    fn test_memory_update_append_only() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let id = kernel
            .remember_working_with_id("kernel", "default", "original content".to_string(), vec![])
            .unwrap()
            .id;

        // Update appends a new immutable revision.
        let new_entry = kernel
            .memory_update("kernel", "default", &id, "updated content".to_string())
            .unwrap();
        let new_id = new_entry.id;
        assert_ne!(id, new_id);

        // The previous revision is no longer the active head.
        let entries = kernel.recall("kernel", "default");
        assert!(!entries.iter().any(|e| e.id == id));
        // New entry is active
        assert!(entries.iter().any(|e| e.id == new_id));

        // Both immutable revisions remain retained; parent_revision_id links them.
        let all = kernel.memory.get_all("kernel");
        let old = all.iter().find(|e| e.id == id).unwrap();
        assert!(old.supersedes.is_none());
        let new = all.iter().find(|e| e.id == new_id).unwrap();
        assert!(new.supersedes.is_none());
        assert_eq!(new.parent_revision_id.as_ref().map(|id| id.as_str()), Some(id.as_str()));
        assert_eq!(old.memory_id, new.memory_id);
    }
}
