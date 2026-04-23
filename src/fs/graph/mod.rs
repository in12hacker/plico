//! Knowledge Graph
//!
//! Re-exports types and provides the KnowledgeGraph trait.
//! Implementation is in the backend module.

pub mod backend;
pub mod types;
pub mod tests;

pub use types::{KGNode, KGEdge, KGNodeType, KGEdgeType, DiskGraph, KGError, KGSearchHit};
pub use backend::{PetgraphBackend, EdgeRecord};

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
}
