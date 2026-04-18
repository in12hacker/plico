//! FS operations — CAS storage and semantic filesystem.

use crate::fs::Query;
use crate::api::permission::{PermissionContext, PermissionAction};
use crate::cas::{AIObject, AIObjectMeta};

impl crate::kernel::AIKernel {
    // ─── CAS Operations ────────────────────────────────────────────────

    /// Store an object directly in CAS.
    pub fn store_object(
        &self,
        data: Vec<u8>,
        meta: AIObjectMeta,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let obj = AIObject::new(data, meta);
        self.cas.put(&obj)
    }

    /// Retrieve an object by CID.
    pub fn get_object(&self, cid: &str, agent_id: &str) -> std::io::Result<AIObject> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        let results = self.fs.read(&Query::ByCid(cid.to_string()))?;
        let obj = results.into_iter().next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID={}", cid))
        })?;
        self.permissions.check_ownership(agent_id, &obj.meta.created_by)?;
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
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.create(content, tags, agent_id.to_string(), intent)
    }

    /// Semantic search with optional tag filtering.
    pub fn semantic_search(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
    ) -> std::io::Result<Vec<crate::fs::SearchResult>> {
        self.semantic_search_with_time(query, agent_id, limit, require_tags, exclude_tags, None, None)
    }

    /// Semantic search with time-range bounds.
    #[allow(clippy::too_many_arguments)]
    pub fn semantic_search_with_time(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
        since: Option<i64>,
        until: Option<i64>,
    ) -> std::io::Result<Vec<crate::fs::SearchResult>> {
        let ctx = PermissionContext::new(agent_id.to_string());
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
        Ok(if can_read_any {
            results.into_iter().take(limit).collect()
        } else {
            results.into_iter()
                .filter(|r| r.meta.created_by == agent_id)
                .take(limit)
                .collect()
        })
    }

    /// Semantic read with ownership isolation.
    pub fn semantic_read(&self, query: &Query, agent_id: &str) -> std::io::Result<Vec<AIObject>> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        let results = self.fs.read(query)?;
        if self.permissions.can_read_any(agent_id) {
            Ok(results)
        } else {
            Ok(results.into_iter()
                .filter(|obj| obj.meta.created_by == agent_id)
                .collect())
        }
    }

    /// Semantic update — only owner or trusted can update.
    pub fn semantic_update(
        &self,
        cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        if let Ok(obj) = self.fs.read(&Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                self.permissions.check_ownership(agent_id, &existing.meta.created_by)?;
            }
        }
        self.fs.update(cid, new_content, new_tags, agent_id.to_string())
    }

    /// Semantic delete (soft delete) — only owner or trusted can delete.
    pub fn semantic_delete(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Delete)?;
        if let Ok(obj) = self.fs.read(&Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                self.permissions.check_ownership(agent_id, &existing.meta.created_by)?;
            }
        }
        self.fs.delete(cid, agent_id.to_string())
    }

    /// List all tags in the filesystem.
    pub fn list_tags(&self) -> Vec<String> {
        self.fs.list_tags()
    }

    /// List soft-deleted objects in the recycle bin.
    pub fn list_deleted(&self, agent_id: &str) -> Vec<crate::fs::RecycleEntry> {
        let _ctx = PermissionContext::new(agent_id.to_string());
        self.fs.list_deleted()
    }

    /// Restore a soft-deleted object.
    pub fn restore_deleted(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
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
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.fs.ctx_loader().load(cid, layer)
    }
}
