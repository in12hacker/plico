//! Knowledge Graph
//!
//! Re-exports types and provides the KnowledgeGraph trait.
//! Implementation is in the backend module.

pub mod backend;
pub mod types;
#[cfg(test)]
mod tests;

pub use types::{KGNode, KGEdge, KGNodeType, KGEdgeType, DiskGraph, KGError, KGSearchHit};
pub use backend::{PetgraphBackend, EdgeRecord};

/// Result of a temporal diff between two time points.
#[derive(Debug, Clone)]
pub struct TemporalDiff {
    pub added: Vec<KGEdge>,
    pub removed: Vec<KGEdge>,
    pub unchanged: Vec<KGEdge>,
}

/// Direction for graph traversal (used by explore).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExploreDirection {
    Outgoing,
    Incoming,
    Both,
}

/// KnowledgeGraph trait — graph operations for entity and relationship tracking.
///
/// Implementations:
/// - `PetgraphBackend`: in-memory + redb 4.0 ACID persistence (O(1) per write)
pub trait KnowledgeGraph: Send + Sync {
    fn add_node(&self, node: KGNode) -> Result<(), KGError>;
    fn add_edge(&self, edge: KGEdge) -> Result<(), KGError>;
    fn get_node(&self, id: &str) -> Result<Option<KGNode>, KGError>;
    fn get_neighbors(
        &self,
        id: &str,
        edge_type: Option<KGEdgeType>,
        depth: u8,
    ) -> Result<Vec<(KGNode, KGEdge)>, KGError>;
    fn find_paths(&self, src: &str, dst: &str, max_depth: u8) -> Result<Vec<Vec<KGNode>>, KGError>;
    fn find_weighted_path(&self, src: &str, dst: &str, max_depth: u8) -> Result<Option<Vec<KGNode>>, KGError>;
    fn list_nodes(&self, agent_id: &str, node_type: Option<KGNodeType>) -> Result<Vec<KGNode>, KGError>;
    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError>;
    fn remove_node(&self, id: &str) -> Result<(), KGError>;
    fn remove_edge(&self, src: &str, dst: &str, edge_type: Option<KGEdgeType>) -> Result<(), KGError>;
    /// Soft-invalidate a specific edge by setting `invalid_at` to now.
    fn invalidate_edge(&self, src: &str, dst: &str, edge_type: KGEdgeType) -> Result<bool, KGError>;
    fn update_node(&self, id: &str, label: Option<&str>, properties: Option<serde_json::Value>) -> Result<(), KGError>;
    fn all_node_ids(&self) -> Vec<String>;
    fn upsert_document(&self, cid: &str, tags: &[String], agent_id: &str) -> Result<(), KGError>;
    fn authority_score(&self, node_id: &str) -> Result<f32, KGError>;
    fn node_count(&self) -> Result<usize, KGError>;
    fn edge_count(&self) -> Result<usize, KGError>;
    fn get_valid_edges_at(&self, t: u64) -> Result<Vec<KGEdge>, KGError>;
    fn get_valid_edge_between(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        t: u64,
    ) -> Result<Option<KGEdge>, KGError>;
    fn invalidate_conflicts(&self, new_edge: &KGEdge) -> Result<usize, KGError>;
    fn edge_history(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
    ) -> Result<Vec<KGEdge>, KGError>;
    fn get_valid_nodes_at(
        &self,
        agent_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> Result<Vec<KGNode>, KGError>;
    fn save_to_disk(&self, path: &std::path::Path) -> Result<(), KGError>;
    fn load_from_disk(&self, path: &std::path::Path) -> Result<(), KGError>;

    /// Personalized PageRank: given seed node IDs, compute PPR scores for all
    /// reachable nodes. Returns top-K node IDs sorted by PPR score (descending).
    ///
    /// Default implementation returns an empty vec (no graph traversal).
    /// Check whether any node references the given CID via `content_cid`.
    ///
    /// Default: O(n) scan. Backends may override with an indexed implementation.
    fn has_node_with_cid(&self, cid: &str) -> Result<bool, KGError> {
        let nodes = self.list_nodes("", None)?;
        Ok(nodes.iter().any(|n| n.content_cid.as_deref() == Some(cid)))
    }

    fn personalized_pagerank(
        &self,
        _seed_nodes: &[String],
        _alpha: f32,
        _max_iter: usize,
        _top_k: usize,
    ) -> Result<Vec<(String, f32)>, KGError> {
        Ok(vec![])
    }

    /// Compute temporal diff: what edges were added/removed/unchanged between t1 and t2.
    fn temporal_diff(
        &self,
        agent_id: &str,
        t1: u64,
        t2: u64,
    ) -> Result<TemporalDiff, KGError> {
        let edges_at_t1 = self.get_valid_edges_at(t1)?;
        let edges_at_t2 = self.get_valid_edges_at(t2)?;

        // Filter to agent's edges
        let edges_t1: Vec<KGEdge> = edges_at_t1
            .into_iter()
            .filter(|e| {
                // Edge belongs to agent if either endpoint node is owned by agent
                // For simplicity, check episode field or use all edges
                true
            })
            .collect();
        let edges_t2: Vec<KGEdge> = edges_at_t2
            .into_iter()
            .filter(|_| true)
            .collect();

        let t1_keys: std::collections::HashSet<String> = edges_t1
            .iter()
            .map(|e| format!("{}|{}|{:?}", e.src, e.dst, e.edge_type))
            .collect();
        let t2_keys: std::collections::HashSet<String> = edges_t2
            .iter()
            .map(|e| format!("{}|{}|{:?}", e.src, e.dst, e.edge_type))
            .collect();

        let added = edges_t2
            .iter()
            .filter(|e| {
                let key = format!("{}|{}|{:?}", e.src, e.dst, e.edge_type);
                !t1_keys.contains(&key)
            })
            .cloned()
            .collect();

        let removed = edges_t1
            .iter()
            .filter(|e| {
                let key = format!("{}|{}|{:?}", e.src, e.dst, e.edge_type);
                !t2_keys.contains(&key)
            })
            .cloned()
            .collect();

        let unchanged = edges_t2
            .iter()
            .filter(|e| {
                let key = format!("{}|{}|{:?}", e.src, e.dst, e.edge_type);
                t1_keys.contains(&key)
            })
            .cloned()
            .collect();

        Ok(TemporalDiff {
            added,
            removed,
            unchanged,
        })
    }

    /// Consolidate redundant historical versions of edges.
    /// Keeps the `keep_last_n` most recent valid versions, marks older invalidated ones as expired.
    /// Returns count of newly expired edges.
    fn consolidate_versions(
        &self,
        src: &str,
        dst: &str,
        edge_type: KGEdgeType,
        keep_last_n: usize,
    ) -> Result<usize, KGError> {
        let history = self.edge_history(src, dst, Some(edge_type))?;

        // Separate valid and invalidated edges
        let mut invalidated: Vec<KGEdge> = history
            .into_iter()
            .filter(|e| e.invalid_at.is_some() && e.expired_at.is_none())
            .collect();

        if invalidated.len() <= keep_last_n {
            return Ok(0);
        }

        // Sort by invalid_at descending (most recently invalidated first)
        invalidated.sort_unstable_by(|a, b| {
            b.invalid_at.unwrap_or(0).cmp(&a.invalid_at.unwrap_or(0))
        });

        // Mark the excess as expired
        let to_expire = &invalidated[keep_last_n..];
        let now = crate::util::now_ms();
        let count = to_expire.len();

        for edge in to_expire {
            let mut expired_edge = edge.clone();
            expired_edge.expired_at = Some(now);
            let _ = self.add_edge(expired_edge);
        }

        Ok(count)
    }
}
