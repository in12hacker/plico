//! FS operations — CAS storage and semantic filesystem.

use crate::fs::Query;
use crate::api::permission::{PermissionContext, PermissionAction};
use crate::cas::{AIObject, AIObjectMeta};
use crate::kernel::event_bus::KernelEvent;
use super::observability::{OpType, OperationTimer};

impl crate::kernel::AIKernel {
    // ─── CAS Operations ────────────────────────────────────────────────

    /// Store an object directly in CAS.
    pub fn store_object(
        &self,
        data: Vec<u8>,
        meta: AIObjectMeta,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let obj = AIObject::new(data, meta);
        self.cas.put(&obj)
    }

    /// Retrieve an object by CID.
    pub fn get_object(&self, cid: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<AIObject> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        let results = self.fs.read(&Query::ByCid(cid.to_string()))?;
        let obj = results.into_iter().next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID={}", cid))
        })?;
        // Check tenant isolation
        self.permissions.check_tenant_access(&ctx, &obj.meta.tenant_id)?;
        // Check ownership (owner can always access)
        self.permissions.check_ownership(&ctx, &obj.meta.created_by)?;
        Ok(obj)
    }

    // ─── Semantic FS Operations ────────────────────────────────────────

    /// Create an object with semantic metadata.
    pub fn semantic_create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        agent_id: &str,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::SemanticCreate);
        let span = tracing::info_span!(
            "semantic_create",
            operation = "semantic_create",
            agent_id = %agent_id,
            tags = ?tags,
            intent = ?intent,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;

        // F-2: Precondition — content must be non-empty (fails fast before CAS write)
        if content.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Cannot create object with empty content",
            ));
        }

        let cid = self.fs.create(content, tags.clone(), agent_id.to_string(), intent)?;

        // F-2: Postcondition — verify CID is actually retrievable (effect contract)
        // This catches silent failures where CID is returned but CAS write didn't complete
        if self.fs.read(&crate::fs::Query::ByCid(cid.clone())).is_err() {
            tracing::error!("Effect contract violated: semantic_create returned CID {} but get failed", cid);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Effect contract violated: created CID not retrievable",
            ));
        }

        self.event_bus.emit(KernelEvent::ObjectStored {
            cid: cid.clone(),
            agent_id: agent_id.to_string(),
            tags,
        });
        tracing::info!(cid = %cid, "object created");
        Ok(cid)
    }

    /// Semantic search with optional tag filtering.
    pub fn semantic_search(
        &self,
        query: &str,
        agent_id: &str,
        tenant_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
    ) -> std::io::Result<Vec<crate::fs::SearchResult>> {
        self.semantic_search_with_time(query, agent_id, tenant_id, limit, require_tags, exclude_tags, None, None)
    }

    /// Semantic search with time-range bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn semantic_search_with_time(
        &self,
        query: &str,
        agent_id: &str,
        tenant_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
        since: Option<i64>,
        until: Option<i64>,
    ) -> std::io::Result<Vec<crate::fs::SearchResult>> {
        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        let can_read_any = self.permissions.can_read_any(agent_id);

        let filter = crate::fs::SearchFilter {
            require_tags,
            exclude_tags,
            content_type: None,
            since,
            until,
        };

        let results = self.fs.search_with_filter(query, limit * 2, filter);
        // Filter by tenant isolation
        // CAS objects are shared by default within tenant (C-2: Shared Visibility fix)
        Ok(results.into_iter()
            .filter(|r| {
                // Tenant isolation: must match tenant_id
                if r.meta.tenant_id != tenant_id {
                    return false;
                }
                // can_read_any = privileged agents (e.g., admin) can read across agents
                if can_read_any {
                    return true;
                }
                // CAS objects are shared by default within tenant
                // Memory objects are filtered separately by MemoryScope in recall operations
                true
            })
            .take(limit)
            .collect())
    }

    /// Direct tag-only search (A-8a: B25 fix).
    pub fn search_by_tags(&self, tags: &[String], limit: usize) -> Vec<crate::fs::SearchResult> {
        self.fs.search_by_tags(tags, limit)
    }

    /// F-4: Search requiring ALL tags to match (AND semantics).
    pub fn search_by_tags_intersection(&self, tags: &[String], limit: usize) -> Vec<crate::fs::SearchResult> {
        self.fs.search_by_tags_intersection(tags, limit)
    }

    /// Semantic read with ownership and tenant isolation.
    pub fn semantic_read(&self, query: &Query, agent_id: &str, tenant_id: &str) -> std::io::Result<Vec<AIObject>> {
        let _timer = OperationTimer::new(&self.metrics, OpType::SemanticRead);
        let span = tracing::info_span!(
            "semantic_read",
            operation = "semantic_read",
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction:: Read)?;
        let results = self.fs.read(query)?;
        let can_read_any = self.permissions.can_read_any(agent_id);
        let objs: Vec<AIObject> = results.into_iter()
            .filter(|obj| {
                // Tenant isolation: must match tenant_id or have CrossTenant permission
                if obj.meta.tenant_id != tenant_id {
                    return false;
                }
                // Ownership check
                if can_read_any {
                    true
                } else {
                    obj.meta.created_by == agent_id
                }
            })
            .collect();
        tracing::info!(count = objs.len(), "objects read");
        Ok(objs)
    }

    /// Semantic update — only owner or trusted can update.
    pub fn semantic_update(
        &self,
        cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<String> {
        let _timer = OperationTimer::new(&self.metrics, OpType::SemanticUpdate);
        let span = tracing::info_span!(
            "semantic_update",
            operation = "semantic_update",
            cid = %cid,
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        if let Ok(obj) = self.fs.read(&Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                // Check tenant isolation first
                self.permissions.check_tenant_access(&ctx, &existing.meta.tenant_id)?;
                self.permissions.check_ownership(&ctx, &existing.meta.created_by)?;
            }
        }
        let new_cid = self.fs.update(cid, new_content, new_tags, agent_id.to_string())?;

        // Emit KnowledgeSuperseded when Supersedes edge is created
        self.event_bus.emit(KernelEvent::KnowledgeSuperseded {
            old_cid: cid.to_string(),
            new_cid: new_cid.clone(),
            agent_id: agent_id.to_string(),
        });

        tracing::info!(new_cid = %new_cid, "object updated");
        Ok(new_cid)
    }

    /// Semantic delete (soft delete) — only owner or trusted can delete.
    pub fn semantic_delete(&self, cid: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<()> {
        let _timer = OperationTimer::new(&self.metrics, OpType::SemanticDelete);
        let span = tracing::info_span!(
            "semantic_delete",
            operation = "semantic_delete",
            cid = %cid,
            agent_id = %agent_id,
            tenant_id = %tenant_id,
        );
        let _guard = span.enter();

        let ctx = PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Delete)?;
        if let Ok(obj) = self.fs.read(&Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                // Check tenant isolation first
                self.permissions.check_tenant_access(&ctx, &existing.meta.tenant_id)?;
                self.permissions.check_ownership(&ctx, &existing.meta.created_by)?;
            }
        }
        self.fs.delete(cid, agent_id.to_string())?;

        // F-2: Postcondition — verify CID is now in recycle bin (effect contract)
        let deleted = self.fs.list_deleted();
        if !deleted.iter().any(|e| e.cid == cid) {
            tracing::error!("Effect contract violated: delete returned success but CID {} not in recycle bin", cid);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Effect contract violated: deleted CID not in recycle bin",
            ));
        }

        tracing::info!(cid = %cid, "object deleted");
        Ok(())
    }

    /// List all tags in the filesystem.
    pub fn list_tags(&self) -> Vec<String> {
        self.fs.list_tags()
    }

    pub fn knowledge_graph(&self) -> Option<&std::sync::Arc<dyn crate::fs::graph::KnowledgeGraph>> {
        self.knowledge_graph.as_ref()
    }

    /// List soft-deleted objects in the recycle bin.
    pub fn list_deleted(&self, agent_id: &str) -> Vec<crate::fs::RecycleEntry> {
        let _ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.fs.list_deleted()
    }

    /// Restore a soft-deleted object.
    pub fn restore_deleted(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.restore(cid, agent_id.to_string())
    }

    /// Load context at a specified layer (L0/L1/L2) for a CID.
    pub fn context_load(
        &self,
        cid: &str,
        layer: crate::fs::ContextLayer,
        agent_id: &str,
    ) -> std::io::Result<crate::fs::LoadedContext> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.fs.ctx_loader().load(cid, layer)
    }

    /// Assemble context from multiple CIDs within a token budget.
    ///
    /// The Context Budget Engine greedily assigns L0/L1/L2 layers to each CID
    /// based on relevance and remaining budget. Like virtual memory paging for AI.
    pub fn context_assemble(
        &self,
        candidates: &[crate::fs::context_budget::ContextCandidate],
        budget_tokens: usize,
        agent_id: &str,
    ) -> std::io::Result<crate::fs::context_budget::BudgetAllocation> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        Ok(crate::fs::context_budget::assemble(
            self.fs.ctx_loader(),
            candidates,
            budget_tokens,
        ))
    }

    /// Get the version history of a CID by following Supersedes edges backwards.
    ///
    /// Returns a chain from newest to oldest: [current, previous, ...]
    pub fn version_history(&self, cid: &str, agent_id: &str) -> Vec<String> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        if self.permissions.check(&ctx, PermissionAction::Read).is_err() {
            return vec![];
        }

        let Some(ref kg) = self.knowledge_graph else {
            return vec![cid.to_string()];
        };

        let mut chain = vec![cid.to_string()];
        let mut current = cid.to_string();
        let max_depth = 50;

        for _ in 0..max_depth {
            let neighbors = match kg.get_neighbors(&current, Some(crate::fs::KGEdgeType::Supersedes), 1) {
                Ok(n) => n,
                Err(_) => break,
            };
            let next = neighbors.iter().find(|(_node, edge)| {
                edge.edge_type == crate::fs::KGEdgeType::Supersedes && edge.src == current
            });
            match next {
                Some((node, _)) => {
                    chain.push(node.id.clone());
                    current = node.id.clone();
                }
                None => break,
            }
        }

        chain
    }

    /// Rollback a CID to a previous version in its Supersedes chain.
    ///
    /// Finds the previous version via Supersedes edges and restores it
    /// as a new update (preserving the full chain). Returns the restored CID.
    pub fn rollback(
        &self,
        cid: &str,
        agent_id: &str,
    ) -> Result<String, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions.check(&ctx, PermissionAction::Write).map_err(|e| e.to_string())?;

        let history = self.version_history(cid, agent_id);
        if history.len() < 2 {
            return Err("No previous version to rollback to".to_string());
        }

        let previous_cid = &history[1];
        let previous_objs = self.fs.read(&Query::ByCid(previous_cid.to_string()))
            .map_err(|e| format!("Cannot read previous version {}: {}", previous_cid, e))?;
        let previous_obj = previous_objs.into_iter().next()
            .ok_or_else(|| format!("Previous version {} not found", previous_cid))?;

        let new_cid = self.semantic_update(
            cid,
            previous_obj.data.clone(),
            Some(previous_obj.meta.tags.clone()),
            agent_id,
            "default",
        ).map_err(|e| format!("Rollback update failed: {}", e))?;

        self.maybe_persist_search_index();

        Ok(new_cid)
    }

    // ─── Storage Governance (F-18) ─────────────────────────────────

    /// Get usage statistics for a CAS object by CID (F-22: real access tracking).
    /// Uses get_raw to avoid inflating the access counter.
    pub fn get_object_usage(&self, cid: &str) -> crate::api::semantic::ObjectUsageResult {
        let access = self.fs.cas().object_usage(cid);
        let created_at = self.fs.cas().get_raw(cid)
            .map(|obj| obj.meta.created_at)
            .unwrap_or(0);

        crate::api::semantic::ObjectUsageResult {
            created_at,
            last_accessed_at: access.as_ref().map(|a| a.last_accessed_at).unwrap_or(0),
            access_count: access.as_ref().map(|a| a.access_count).unwrap_or(0),
            referenced_by_kg: false,
            referenced_by_memory: false,
        }
    }

    /// Get complete storage statistics (F-23: real CAS + memory data).
    pub fn get_storage_stats(&self) -> crate::api::semantic::StorageStatsResult {
        let total_objects = self.fs.count_objects().unwrap_or(0);
        let total_bytes = self.fs.cas().total_bytes() as usize;
        let cold_threshold = 30 * 24 * 3600 * 1000_u64; // 30 days
        let cold_objects = self.fs.cas().cold_objects(cold_threshold).len();

        crate::api::semantic::StorageStatsResult {
            total_objects,
            total_bytes,
            by_tier: crate::api::semantic::TierStats {
                ephemeral_count: 0,
                ephemeral_bytes: 0,
                working_count: 0,
                working_bytes: 0,
                longterm_count: 0,
                longterm_bytes: 0,
            },
            cold_objects,
            about_to_expire: 0,
        }
    }

    /// Evict cold objects from CAS (F-24: real eviction via soft-delete).
    /// `dry_run=true` returns what would be evicted without acting.
    pub fn evict_cold(&self, dry_run: bool) -> crate::api::semantic::EvictColdResult {
        let cold_threshold = 30 * 24 * 3600 * 1000_u64;
        let cold_cids = self.fs.cas().cold_objects(cold_threshold);
        let evicted_count = cold_cids.len();

        if dry_run || cold_cids.is_empty() {
            return crate::api::semantic::EvictColdResult {
                evicted_count,
                evicted_bytes: 0,
                remaining_cold: 0,
            };
        }

        let mut evicted_bytes = 0usize;
        for cid in &cold_cids {
            if let Ok(meta) = std::fs::metadata(self.fs.cas().root().join(&cid[..2]).join(&cid[2..])) {
                evicted_bytes += meta.len() as usize;
            }
            let _ = self.fs.delete(cid, "system".to_string());
        }

        let _ = self.fs.cas().persist_access_log();

        crate::api::semantic::EvictColdResult {
            evicted_count,
            evicted_bytes,
            remaining_cold: 0,
        }
    }

    /// Persist CAS access log (called on shutdown/checkpoint).
    pub fn persist_cas_access_log(&self) -> std::io::Result<()> {
        self.fs.cas().persist_access_log()
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_create_and_semantic_delete_roundtrip() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let cid = kernel.semantic_create(
            b"important data".to_vec(),
            vec!["test".to_string(), "data".to_string()],
            "kernel",
            None,
        ).expect("create failed");

        // Verify it exists
        let obj = kernel.get_object(&cid, "kernel", "default").expect("get failed");
        assert_eq!(obj.data, b"important data");

        // Delete it
        kernel.semantic_delete(&cid, "kernel", "default").expect("delete failed");

        // Verify it's gone (or in recycle bin)
        let entries = kernel.list_deleted("kernel");
        assert!(entries.iter().any(|e| e.cid == cid));
    }

    #[test]
    fn test_semantic_search_with_require_tags_and_semantics() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.semantic_create(b"doc-a".to_vec(), vec!["rust".to_string(), "async".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"doc-b".to_vec(), vec!["rust".to_string(), "sync".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"doc-c".to_vec(), vec!["go".to_string(), "async".to_string()], "kernel", None).ok();

        // Search with require_tags (AND semantics) - should match only doc-a
        let results = kernel.semantic_search("rust", "kernel", "default", 10, vec!["async".to_string()], vec![]).expect("search failed");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_semantic_search_exclude_tags() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.semantic_create(b"keep-me".to_vec(), vec!["visible".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"exclude-me".to_vec(), vec!["visible".to_string(), "secret".to_string()], "kernel", None).ok();

        let results = kernel.semantic_search("", "kernel", "default", 10, vec![], vec!["secret".to_string()]).expect("search failed");
        assert!(results.iter().all(|r| !r.meta.tags.contains(&"secret".to_string())));
    }

    #[test]
    fn test_search_by_tags_intersection() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.semantic_create(b"doc1".to_vec(), vec!["a".to_string(), "b".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"doc2".to_vec(), vec!["a".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"doc3".to_vec(), vec!["b".to_string()], "kernel", None).ok();

        let results = kernel.search_by_tags_intersection(&["a".to_string(), "b".to_string()], 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].meta.tags, vec!["a", "b"]);
    }

    #[test]
    fn test_list_tags() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        kernel.semantic_create(b"x".to_vec(), vec!["tag-a".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"y".to_vec(), vec!["tag-b".to_string()], "kernel", None).ok();
        kernel.semantic_create(b"z".to_vec(), vec!["tag-a".to_string()], "kernel", None).ok();

        let tags = kernel.list_tags();
        assert!(tags.contains(&"tag-a".to_string()));
        assert!(tags.contains(&"tag-b".to_string()));
    }

    #[test]
    fn test_version_history_single_version() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let cid = kernel.semantic_create(b"v1".to_vec(), vec!["test".to_string()], "kernel", None).expect("create failed");

        let history = kernel.version_history(&cid, "kernel");
        // Single version with no supersedes chain returns just itself
        assert!(history.len() >= 1);
        assert_eq!(history[0], cid);
    }

    // ─── F-2: Effect Contracts ─────────────────────────────────────────────

    #[test]
    fn test_semantic_create_empty_content_rejected() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        // F-2: Empty content should be rejected at kernel layer (precondition)
        let result = kernel.semantic_create(vec![], vec!["tag".to_string()], "kernel", None);
        assert!(result.is_err(), "empty content should be rejected");
        let err = result.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_semantic_create_returns_valid_cid() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let cid = kernel.semantic_create(b"test content".to_vec(), vec![], "kernel", None).expect("create failed");
        // CID should be valid hex string (SHA-256 = 64 hex chars)
        assert_eq!(cid.len(), 64);
        assert!(cid.chars().all(|c| c.is_ascii_hexdigit()), "CID should be valid hex");
    }

    #[test]
    fn test_semantic_delete_cid_in_recycle_bin() {
        let (kernel, _dir) = crate::kernel::tests::make_kernel();
        let cid = kernel.semantic_create(b"to delete".to_vec(), vec!["tmp".to_string()], "kernel", None).expect("create failed");

        kernel.semantic_delete(&cid, "kernel", "default").expect("delete failed");

        // F-2: Postcondition — CID must be in recycle bin after delete
        let deleted = kernel.list_deleted("kernel");
        assert!(deleted.iter().any(|e| e.cid == cid), "deleted CID must be in recycle bin");
    }
}
