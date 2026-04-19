//! Knowledge graph operations — explore, node/edge CRUD.

use crate::fs::{KGNodeType, KGNode, KGEdgeType, KGEdge, KGSearchHit};

impl crate::kernel::AIKernel {
    /// Explore graph neighbors of a CID at a given depth.
    pub fn graph_explore(&self, cid: &str, edge_type: Option<KGEdgeType>, depth: u8) -> Vec<KGSearchHit> {
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        let neighbors = match kg.get_neighbors(cid, edge_type, depth) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("graph_explore failed for {}: {}", cid, e);
                return Vec::new();
            }
        };
        neighbors
            .into_iter()
            .map(|(node, edge)| KGSearchHit {
                node,
                edge_type: Some(edge.edge_type),
                vector_score: 0.0,
                authority_score: kg.authority_score(cid).unwrap_or(0.0),
                combined_score: 0.0,
            })
            .collect()
    }

    /// Explore graph neighbors, returning plain strings (no fs types) for API layer.
    ///
    /// Returns `Vec<(node_id, label, node_type, edge_type, authority_score)>`.
    pub fn graph_explore_raw(
        &self,
        cid: &str,
        edge_type_str: Option<&str>,
        depth: u8,
    ) -> Vec<(String, String, String, String, f32)> {
        let edge_filter = edge_type_str.and_then(|s| match s {
            "associates_with" => Some(KGEdgeType::AssociatesWith),
            "mentions"        => Some(KGEdgeType::Mentions),
            "follows"         => Some(KGEdgeType::Follows),
            "part_of"         => Some(KGEdgeType::PartOf),
            "related_to"      => Some(KGEdgeType::RelatedTo),
            "similar_to"      => Some(KGEdgeType::SimilarTo),
            "causes"          => Some(KGEdgeType::Causes),
            "reminds"         => Some(KGEdgeType::Reminds),
            _ => None,
        });
        self.graph_explore(cid, edge_filter, depth)
            .into_iter()
            .map(|hit| {
                let node_type = format!("{:?}", hit.node.node_type).to_lowercase();
                let edge_type = hit.edge_type
                    .map(|et| format!("{:?}", et).to_lowercase())
                    .unwrap_or_default();
                (hit.node.id, hit.node.label, node_type, edge_type, hit.authority_score)
            })
            .collect()
    }

    /// Create an arbitrary KG node (Entity, Fact, Document, Agent, Memory).
    pub fn kg_add_node(
        &self,
        label: &str,
        node_type: KGNodeType,
        properties: serde_json::Value,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<String> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let node = KGNode {
            id: id.clone(),
            label: label.to_string(),
            node_type,
            content_cid: None,
            properties,
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
        };
        kg.add_node(node)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(id)
    }

    /// Create an edge between two KG nodes.
    pub fn kg_add_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: KGEdgeType,
        weight: Option<f32>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Verify tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let edge = KGEdge {
            src: src.to_string(),
            dst: dst.to_string(),
            edge_type,
            weight: weight.unwrap_or(1.0),
            evidence_cid: None,
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
            episode: None,
        };
        kg.add_edge(edge)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// List KG nodes, optionally filtered by type.
    /// Returns only nodes belonging to the caller's tenant.
    pub fn kg_list_nodes(
        &self,
        node_type: Option<KGNodeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<Vec<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        kg.list_nodes(agent_id, node_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .map(|mut nodes| {
                // Filter by tenant isolation
                nodes.retain(|n| n.tenant_id == tenant_id);
                nodes
            })
    }

    /// Find all paths between two KG nodes up to a given depth.
    pub fn kg_find_paths(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Vec<Vec<KGNode>> {
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        kg.find_paths(src, dst, max_depth).unwrap_or_default()
    }

    /// Find the highest-weighted path between two KG nodes using best-first search.
    /// Returns `None` if no path exists within max_depth hops.
    pub fn kg_find_weighted_path(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Option<Vec<KGNode>> {
        let kg = self.knowledge_graph.as_ref()?;
        kg.find_weighted_path(src, dst, max_depth).unwrap_or_default()
    }

    /// Query nodes valid at a specific point in time.
    ///
    /// Returns nodes where:
    /// - `valid_at <= t`
    /// - `invalid_at.is_none() || invalid_at > t`
    /// - `expired_at.is_none()` (soft-deleted nodes excluded)
    /// - tenant_id matches caller's tenant
    pub fn kg_get_valid_nodes_at(
        &self,
        agent_id: &str,
        tenant_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> std::io::Result<Vec<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        kg.get_valid_nodes_at(agent_id, node_type, t)
            .map_err(|e| std::io::Error::other(e.to_string()))
            .map(|mut nodes| {
                // Filter by tenant isolation
                nodes.retain(|n| n.tenant_id == tenant_id);
                nodes
            })
    }

    /// Get a single KG node by ID.
    pub fn kg_get_node(&self, node_id: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<Option<KGNode>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(None);
        };
        let node = kg.get_node(node_id).map_err(|e| std::io::Error::other(e.to_string()))?;
        // Check tenant isolation
        if let Some(ref n) = node {
            self.permissions.check_tenant_access(&ctx, &n.tenant_id)?;
        }
        Ok(node)
    }

    /// List edges, optionally filtered by a node they touch.
    /// Only returns edges where both source and destination nodes belong to the caller's tenant.
    pub fn kg_list_edges(&self, agent_id: &str, tenant_id: &str, node_id: Option<&str>) -> std::io::Result<Vec<KGEdge>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        let edges = kg.list_edges(agent_id)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        // Filter edges by tenant isolation (both src and dst nodes must be in same tenant)
        let filtered: Vec<KGEdge> = edges.into_iter().filter(|e| {
            if let (Ok(Some(src_node)), Ok(Some(dst_node))) = (kg.get_node(&e.src), kg.get_node(&e.dst)) {
                src_node.tenant_id == tenant_id && dst_node.tenant_id == tenant_id
            } else {
                false
            }
        }).collect();
        if let Some(nid) = node_id {
            Ok(filtered.into_iter().filter(|e| e.src == nid || e.dst == nid).collect())
        } else {
            Ok(filtered)
        }
    }

    /// Remove an edge between two KG nodes.
    pub fn kg_remove_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Delete)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Verify tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        kg.remove_edge(src, dst, edge_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Update an existing KG node's label and/or properties.
    pub fn kg_update_node(
        &self,
        node_id: &str,
        label: Option<&str>,
        properties: Option<serde_json::Value>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Check tenant isolation before update
        if let Ok(Some(node)) = kg.get_node(node_id) {
            self.permissions.check_tenant_access(&ctx, &node.tenant_id)?;
        }
        kg.update_node(node_id, label, properties)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Remove a KG node and all its edges.
    pub fn kg_remove_node(&self, node_id: &str, agent_id: &str, tenant_id: &str) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Delete)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::other("knowledge graph not available"));
        };
        // Check tenant isolation before delete
        if let Ok(Some(node)) = kg.get_node(node_id) {
            self.permissions.check_tenant_access(&ctx, &node.tenant_id)?;
        }
        kg.remove_node(node_id)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Get full edge history (including invalidated edges) between two nodes.
    pub fn kg_edge_history(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        agent_id: &str,
        tenant_id: &str,
    ) -> std::io::Result<Vec<KGEdge>> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), tenant_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Ok(Vec::new());
        };
        // Check tenant isolation for both nodes
        if let Ok(Some(src_node)) = kg.get_node(src) {
            self.permissions.check_tenant_access(&ctx, &src_node.tenant_id)?;
        }
        if let Ok(Some(dst_node)) = kg.get_node(dst) {
            self.permissions.check_tenant_access(&ctx, &dst_node.tenant_id)?;
        }
        kg.edge_history(src, dst, edge_type)
            .map_err(|e| std::io::Error::other(e.to_string()))
    }
}
