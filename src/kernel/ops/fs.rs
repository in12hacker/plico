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

    /// Get the version history of a CID by following Supersedes edges backwards.
    ///
    /// Returns a chain from newest to oldest: [current, previous, ...]
    pub fn version_history(&self, cid: &str, agent_id: &str) -> Vec<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
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
        let ctx = PermissionContext::new(agent_id.to_string());
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
        ).map_err(|e| format!("Rollback update failed: {}", e))?;

        self.maybe_persist_search_index();

        Ok(new_cid)
    }
}
