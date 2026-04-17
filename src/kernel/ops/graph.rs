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
    ) -> std::io::Result<String> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not available"));
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
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
        };
        kg.add_node(node)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
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
    ) -> std::io::Result<()> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, crate::api::permission::PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not available"));
        };
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
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// List KG nodes, optionally filtered by type.
    pub fn kg_list_nodes(
        &self,
        node_type: Option<KGNodeType>,
        agent_id: &str,
    ) -> Vec<KGNode> {
        let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, crate::api::permission::PermissionAction::Read);
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        kg.list_nodes(agent_id, node_type).unwrap_or_default()
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
}
