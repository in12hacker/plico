//! Knowledge Graph Module
//!
//! Provides a typed knowledge graph for AI agents. Nodes represent entities,
//! facts, and memory blocks extracted from CAS objects. Edges represent
//! typed relationships (RelatedTo, Follows, Mentions, etc.).
//!
//! # Node Types
//!
//! | Type | Created when | Content |
//! |------|-------------|---------|
//! | `Document` | SemanticFS.create() | CAS object reference |
//! | `Entity` | LLM extraction | Named entity (person/concept/project) |
//! | `Fact` | LLM extraction | Subject-predicate-object triple |
//!
//! # Edge Types
//!
//! | Type | Created when | Weight basis |
//! |------|-------------|-------------|
//! | `AssociatesWith` | Shared ≥2 tags | tag overlap count |
//! | `Mentions` | Entity extracted from document | 1.0 |
//! | `Follows` | Temporal/conversational sequence | recency decay |
//! | `SimilarTo` | Vector similarity > 0.85 | cosine similarity |

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};

/// Node type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGNodeType {
    Entity,
    Fact,
    Document,
    Agent,
    Memory,
}

impl std::fmt::Display for KGNodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KGNodeType::Entity => write!(f, "entity"),
            KGNodeType::Fact => write!(f, "fact"),
            KGNodeType::Document => write!(f, "document"),
            KGNodeType::Agent => write!(f, "agent"),
            KGNodeType::Memory => write!(f, "memory"),
        }
    }
}

/// Edge type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGEdgeType {
    AssociatesWith,
    Follows,
    Mentions,
    Causes,
    Reminds,
    PartOf,
    SimilarTo,
    RelatedTo,
}

impl std::fmt::Display for KGEdgeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KGEdgeType::AssociatesWith => write!(f, "associates_with"),
            KGEdgeType::Follows => write!(f, "follows"),
            KGEdgeType::Mentions => write!(f, "mentions"),
            KGEdgeType::Causes => write!(f, "causes"),
            KGEdgeType::Reminds => write!(f, "reminds"),
            KGEdgeType::PartOf => write!(f, "part_of"),
            KGEdgeType::SimilarTo => write!(f, "similar_to"),
            KGEdgeType::RelatedTo => write!(f, "related_to"),
        }
    }
}

/// A node in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGNode {
    pub id: String,
    pub label: String,
    pub node_type: KGNodeType,
    /// CID of the backing CAS object.
    pub content_cid: Option<String>,
    pub properties: serde_json::Value,
    pub agent_id: String,
    pub created_at: u64,
}

/// An edge in the knowledge graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGEdge {
    pub src: String,
    pub dst: String,
    pub edge_type: KGEdgeType,
    /// 0.0–1.0. Used for scoring.
    pub weight: f32,
    pub evidence_cid: Option<String>,
    pub created_at: u64,
}

/// A hybrid search result combining vector similarity and graph authority.
#[derive(Debug, Clone)]
pub struct KGSearchHit {
    pub node: KGNode,
    pub edge_type: Option<KGEdgeType>,
    pub vector_score: f32,
    pub authority_score: f32,
    pub combined_score: f32,
}

/// Errors from knowledge graph operations.
#[derive(Debug, thiserror::Error)]
pub enum KGError {
    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Edge already exists: {0} → {1} ({2:?})")]
    EdgeExists(String, String, KGEdgeType),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// The KnowledgeGraph trait — pluggable graph backends.
///
/// Implement this trait to provide different storage strategies:
/// - `PetgraphBackend`: in-memory + JSON persistence (MVP, fast)
/// - `SqliteGraphBackend`: persisted adjacency list (future)
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
    fn list_nodes(&self, agent_id: &str, node_type: Option<KGNodeType>) -> Result<Vec<KGNode>, KGError>;
    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError>;
    fn remove_node(&self, id: &str) -> Result<(), KGError>;
    /// Upsert a Document node and auto-create AssociatesWith edges for shared ≥2 tags.
    fn upsert_document(&self, cid: &str, tags: &[String], agent_id: &str) -> Result<(), KGError>;
    /// Compute authority score (log-scaled degree, normalized 0–1).
    fn authority_score(&self, node_id: &str) -> Result<f32, KGError>;
}

/// Flattened edge record for JSON serialization (restores bidirectional edges on load).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct EdgeRecord {
    src: String,
    dst: String,
    edge: KGEdge,
}

// ─── In-memory implementation ───────────────────────────────────────────────────────

/// In-memory knowledge graph backed by HashMap.
/// Thread-safe via RwLock. Persisted to disk as JSON.
pub struct PetgraphBackend {
    nodes: RwLock<HashMap<String, KGNode>>,
    /// Outbound edges: src → [(dst, edge_data)]
    out_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    /// Inbound edges: dst → [(src, edge_data)]
    in_edges: RwLock<HashMap<String, Vec<(String, KGEdge)>>>,
    /// Path prefix for persistence (e.g. root/kg_nodes.json, root/kg_edges.json).
    path: Option<std::path::PathBuf>,
}

impl PetgraphBackend {
    /// Create a new in-memory graph with no persistence path.
    pub fn new() -> Self {
        Self {
            nodes: RwLock::new(HashMap::new()),
            out_edges: RwLock::new(HashMap::new()),
            in_edges: RwLock::new(HashMap::new()),
            path: None,
        }
    }

    /// Create or open a persisted graph at the given root path.
    pub fn open(root: std::path::PathBuf) -> Self {
        let nodes_path = root.join("kg_nodes.json");
        let edges_path = root.join("kg_edges.json");

        let (nodes, out_e, in_e) = if nodes_path.exists() {
            match Self::load_from_disk(&nodes_path, &edges_path) {
                Ok(triple) => triple,
                Err(e) => {
                    tracing::warn!("Failed to load knowledge graph, starting fresh: {}", e);
                    (HashMap::new(), HashMap::new(), HashMap::new())
                }
            }
        } else {
            (HashMap::new(), HashMap::new(), HashMap::new())
        };

        Self {
            nodes: RwLock::new(nodes),
            out_edges: RwLock::new(out_e),
            in_edges: RwLock::new(in_e),
            path: Some(root),
        }
    }

    fn load_from_disk(
        nodes_path: &std::path::Path,
        edges_path: &std::path::Path,
    ) -> Result<(HashMap<String, KGNode>, HashMap<String, Vec<(String, KGEdge)>>, HashMap<String, Vec<(String, KGEdge)>>), KGError> {
        let nodes: HashMap<String, KGNode> = serde_json::from_str(&std::fs::read_to_string(nodes_path)?)
            .map_err(|e| KGError::Json(e))?;
        let edges: Vec<EdgeRecord> = serde_json::from_str(&std::fs::read_to_string(edges_path)?)
            .map_err(|e| KGError::Json(e))?;

        let mut out_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        let mut in_edges: HashMap<String, Vec<(String, KGEdge)>> = HashMap::new();
        for rec in edges {
            out_edges.entry(rec.src.clone()).or_default().push((rec.dst.clone(), rec.edge.clone()));
            in_edges.entry(rec.dst.clone()).or_default().push((rec.src.clone(), rec.edge));
        }
        Ok((nodes, out_edges, in_edges))
    }

    fn persist(&self) {
        let Some(ref path) = self.path else { return };
        let nodes_path = path.join("kg_nodes.json");
        let edges_path = path.join("kg_edges.json");

        let nodes = self.nodes.read().unwrap();
        if let Ok(json) = serde_json::to_string(&*nodes) {
            let _ = std::fs::write(&nodes_path, json);
        }
        drop(nodes);

        let out = self.out_edges.read().unwrap();
        let records: Vec<EdgeRecord> = out
            .iter()
            .flat_map(|(src, list)| {
                list.iter().map(move |(dst, edge)| EdgeRecord {
                    src: src.clone(),
                    dst: dst.clone(),
                    edge: edge.clone(),
                })
            })
            .collect();
        if let Ok(json) = serde_json::to_string(&records) {
            let _ = std::fs::write(&edges_path, json);
        }
    }

    /// Upsert a Document node and automatically create AssociatesWith edges
    /// to all existing Document nodes that share ≥2 tags.
    ///
    /// Call this from SemanticFS.create() to automatically build the knowledge graph.
    pub fn upsert_document(
        &self,
        cid: &str,
        tags: &[String],
        agent_id: &str,
    ) -> Result<(), KGError> {
        let node = KGNode {
            id: cid.to_string(),
            label: format!("doc:{}", &cid[..8.min(cid.len())]),
            node_type: KGNodeType::Document,
            content_cid: Some(cid.to_string()),
            properties: serde_json::json!({ "tags": tags }),
            agent_id: agent_id.to_string(),
            created_at: now_ms(),
        };

        // Find docs sharing ≥2 tags
        let candidates: Vec<_> = {
            let nodes = self.nodes.read().unwrap();
            nodes
                .values()
                .filter(|n| {
                    n.agent_id == agent_id
                        && n.node_type == KGNodeType::Document
                        && n.id != cid
                        && shared_tag_count(&n.properties, tags) >= 2
                })
                .map(|n| (n.id.clone(), shared_tag_count(&n.properties, tags)))
                .collect()
        };

        self.add_node(node)?;

        for (other_id, shared) in candidates {
            let w = (shared as f32).min(1.0);
            let ts = now_ms();
            let e1 = KGEdge {
                src: cid.to_string(),
                dst: other_id.clone(),
                edge_type: KGEdgeType::AssociatesWith,
                weight: w,
                evidence_cid: None,
                created_at: ts,
            };
            let e2 = KGEdge {
                src: other_id.clone(),
                dst: cid.to_string(),
                edge_type: KGEdgeType::AssociatesWith,
                weight: w,
                evidence_cid: None,
                created_at: ts,
            };
            let _ = self.add_edge(e1);
            let _ = self.add_edge(e2);
        }

        Ok(())
    }

    /// Degree: count of unique neighbor nodes (in/out combined, duplicates merged).
    /// For KG: a bidirectional edge = 1 connection, not 2.
    pub fn degree(&self, node_id: &str) -> usize {
        let out = self.out_edges.read().unwrap();
        let inc = self.in_edges.read().unwrap();
        let mut neighbors: HashSet<String> = out
            .get(node_id)
            .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
            .unwrap_or_default();
        if let Some(inc_list) = inc.get(node_id) {
            for (n, _) in inc_list {
                neighbors.insert(n.clone());
            }
        }
        neighbors.len()
    }

    /// Authority score = log-scaled degree (unique neighbors), normalized 0-1.
    fn authority_score(&self, node_id: &str) -> f32 {
        let degree = self.degree(node_id);
        if degree == 0 {
            return 0.0;
        }
        let max_degree = {
            let out = self.out_edges.read().unwrap();
            let inc = self.in_edges.read().unwrap();
            let all_ids: HashSet<_> = out.keys().chain(inc.keys()).cloned().collect();
            all_ids
                .into_iter()
                .map(|id| {
                    let mut nbrs: HashSet<String> = out
                        .get(&id)
                        .map(|v| v.iter().map(|(n, _)| n.clone()).collect())
                        .unwrap_or_default();
                    if let Some(inc_list) = inc.get(&id) {
                        for (n, _) in inc_list {
                            nbrs.insert(n.clone());
                        }
                    }
                    nbrs.len()
                })
                .max()
                .unwrap_or(1)
        };
        ((degree as f32).ln() / ((max_degree.max(1)) as f32).ln()).max(0.0).min(1.0)
    }
}

fn shared_tag_count(props: &serde_json::Value, tags: &[String]) -> usize {
    let existing: HashSet<String> = props
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    tags.iter().filter(|t| existing.contains(t.as_str())).count()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for PetgraphBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl KnowledgeGraph for PetgraphBackend {
    fn add_node(&self, node: KGNode) -> Result<(), KGError> {
        let mut nodes = self.nodes.write().unwrap();
        nodes.insert(node.id.clone(), node);
        drop(nodes);
        self.persist();
        Ok(())
    }

    fn add_edge(&self, edge: KGEdge) -> Result<(), KGError> {
        {
            let out = self.out_edges.read().unwrap();
            if out
                .get(&edge.src)
                .map_or(false, |v| v.iter().any(|(dst, _)| dst == &edge.dst))
            {
                return Err(KGError::EdgeExists(edge.src.clone(), edge.dst.clone(), edge.edge_type));
            }
        }
        self.out_edges
            .write()
            .unwrap()
            .entry(edge.src.clone())
            .or_default()
            .push((edge.dst.clone(), edge.clone()));
        self.in_edges
            .write()
            .unwrap()
            .entry(edge.dst.clone())
            .or_default()
            .push((edge.src.clone(), edge));
        self.persist();
        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes.get(id).cloned())
    }

    fn get_neighbors(
        &self,
        id: &str,
        edge_type: Option<KGEdgeType>,
        depth: u8,
    ) -> Result<Vec<(KGNode, KGEdge)>, KGError> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let inc = self.in_edges.read().unwrap();

        let mut results = Vec::new();
        let mut frontier: HashSet<_> = [id.to_string()].into_iter().collect();
        let mut visited: HashSet<_> = frontier.clone();

        for _d in 0..depth {
            let mut next = HashSet::new();
            for current in frontier.iter() {
                // outbound neighbors
                if let Some(out_list) = out.get(current) {
                    for (neighbor, edge) in out_list {
                        if visited.contains(neighbor) {
                            continue;
                        }
                        if edge_type.map_or(true, |et| edge.edge_type == et) {
                            if let Some(node) = nodes.get(neighbor) {
                                results.push((node.clone(), edge.clone()));
                                visited.insert(neighbor.clone());
                                next.insert(neighbor.clone());
                            }
                        }
                    }
                }
                // inbound neighbors
                if let Some(inc_list) = inc.get(current) {
                    for (neighbor, edge) in inc_list {
                        if visited.contains(neighbor) {
                            continue;
                        }
                        if edge_type.map_or(true, |et| edge.edge_type == et) {
                            if let Some(node) = nodes.get(neighbor) {
                                results.push((node.clone(), edge.clone()));
                                visited.insert(neighbor.clone());
                                next.insert(neighbor.clone());
                            }
                        }
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }

        Ok(results)
    }

    fn find_paths(&self, src: &str, dst: &str, max_depth: u8) -> Result<Vec<Vec<KGNode>>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();

        let mut results = Vec::new();
        let mut stack: Vec<(String, Vec<String>)> = vec![(src.to_string(), vec![src.to_string()])];
        let mut visited: HashSet<String> = [src.to_string()].into_iter().collect();

        while let Some((current, path)) = stack.pop() {
            if current == dst {
                let node_path: Vec<_> = path
                    .iter()
                    .filter_map(|id| nodes.get(id).cloned())
                    .collect();
                results.push(node_path);
                continue;
            }
            if path.len() >= max_depth as usize {
                continue;
            }
            if let Some(out_list) = out.get(&current) {
                for (neighbor, _) in out_list {
                    if !visited.contains(neighbor) {
                        let mut new_path = path.clone();
                        new_path.push(neighbor.clone());
                        visited.insert(neighbor.clone());
                        stack.push((neighbor.clone(), new_path));
                    }
                }
            }
        }

        Ok(results)
    }

    fn list_nodes(&self, agent_id: &str, node_type: Option<KGNodeType>) -> Result<Vec<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes
            .values()
            .filter(|n| n.agent_id == agent_id && node_type.map_or(true, |t| n.node_type == t))
            .cloned()
            .collect())
    }

    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let mut edges = Vec::new();

        for (src, out_list) in out.iter() {
            if nodes.get(src).map(|n| &n.agent_id == agent_id).unwrap_or(false) {
                for (_, edge) in out_list {
                    edges.push(edge.clone());
                }
            }
        }

        Ok(edges)
    }

    fn remove_node(&self, id: &str) -> Result<(), KGError> {
        {
            let mut nodes = self.nodes.write().unwrap();
            nodes.remove(id);
        }
        {
            let mut out = self.out_edges.write().unwrap();
            if let Some(removed) = out.remove(id) {
                for (dst, _) in removed {
                    if let Some(inc_list) = self.in_edges.write().unwrap().get_mut(&dst) {
                        inc_list.retain(|(src, _)| src != id);
                    }
                }
            }
        }
        {
            let mut inc = self.in_edges.write().unwrap();
            if let Some(removed) = inc.remove(id) {
                for (src, _) in removed {
                    if let Some(out_list) = self.out_edges.write().unwrap().get_mut(&src) {
                        out_list.retain(|(dst, _)| dst != id);
                    }
                }
            }
        }
        self.persist();
        Ok(())
    }

    fn upsert_document(&self, cid: &str, tags: &[String], agent_id: &str) -> Result<(), KGError> {
        PetgraphBackend::upsert_document(self, cid, tags, agent_id)
    }

    fn authority_score(&self, node_id: &str) -> Result<f32, KGError> {
        Ok(PetgraphBackend::authority_score(self, node_id))
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, node_type: KGNodeType, tags: Vec<String>, agent: &str) -> KGNode {
        KGNode {
            id: id.to_string(),
            label: id.to_string(),
            node_type,
            content_cid: Some(id.to_string()),
            properties: serde_json::json!({ "tags": tags }),
            agent_id: agent.to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn test_add_node_and_get() {
        let kg = PetgraphBackend::new();
        let node = make_node("node1", KGNodeType::Document, vec![], "agent1");
        kg.add_node(node.clone()).unwrap();
        let found = kg.get_node("node1").unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, "node1");
    }

    #[test]
    fn test_add_edge() {
        let kg = PetgraphBackend::new();
        let n1 = make_node("n1", KGNodeType::Entity, vec![], "a");
        let n2 = make_node("n2", KGNodeType::Entity, vec![], "a");
        kg.add_node(n1).unwrap();
        kg.add_node(n2).unwrap();

        kg.add_edge(KGEdge {
            src: "n1".into(), dst: "n2".into(),
            edge_type: KGEdgeType::RelatedTo, weight: 0.9,
            evidence_cid: None, created_at: 0,
        }).unwrap();

        let neighbors = kg.get_neighbors("n1", None, 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0.id, "n2");
    }

    #[test]
    fn test_edge_already_exists_error() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "x")).unwrap();

        kg.add_edge(KGEdge {
            src: "a".into(), dst: "b".into(),
            edge_type: KGEdgeType::RelatedTo, weight: 0.5,
            evidence_cid: None, created_at: 0,
        }).unwrap();

        let err = kg.add_edge(KGEdge {
            src: "a".into(), dst: "b".into(),
            edge_type: KGEdgeType::Mentions, weight: 0.5,
            evidence_cid: None, created_at: 0,
        }).unwrap_err();

        assert!(matches!(err, KGError::EdgeExists(_, _, _)));
    }

    #[test]
    fn test_neighbors_bidirectional() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        // x → y
        kg.add_edge(KGEdge {
            src: "x".into(), dst: "y".into(),
            edge_type: KGEdgeType::Follows, weight: 1.0,
            evidence_cid: None, created_at: 0,
        }).unwrap();

        // y has inbound neighbor x
        let n = kg.get_neighbors("y", None, 1).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].0.id, "x");
    }

    #[test]
    fn test_neighbors_depth2_chain() {
        let kg = PetgraphBackend::new();
        // a → b → c
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("c", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_edge(KGEdge { src: "a".into(), dst: "b".into(), edge_type: KGEdgeType::Follows, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();
        kg.add_edge(KGEdge { src: "b".into(), dst: "c".into(), edge_type: KGEdgeType::Follows, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();

        let n1 = kg.get_neighbors("a", None, 1).unwrap();
        assert_eq!(n1.len(), 1);
        assert_eq!(n1[0].0.id, "b");

        let n2 = kg.get_neighbors("a", None, 2).unwrap();
        let ids: HashSet<_> = n2.iter().map(|(n, _)| n.id.clone()).collect();
        assert!(ids.contains(&"b".to_string()));
        assert!(ids.contains(&"c".to_string()));
    }

    #[test]
    fn test_upsert_document_auto_associates() {
        let kg = PetgraphBackend::new();

        // Doc1 with 3 tags
        kg.upsert_document("doc1", &["rust".into(), "async".into(), "concurrency".into()], "agent1")
            .unwrap();

        // Doc2 sharing 2 tags (≥2 → AssociatesWith)
        kg.upsert_document("doc2", &["rust".into(), "async".into(), "networking".into()], "agent1")
            .unwrap();

        // Doc1 should have AssociatesWith edge to doc2
        let n1_neighbors = kg.get_neighbors("doc1", Some(KGEdgeType::AssociatesWith), 1).unwrap();
        assert!(!n1_neighbors.is_empty(), "doc1 should have AssociatesWith edge to doc2");
    }

    #[test]
    fn test_no_associate_single_tag() {
        let kg = PetgraphBackend::new();

        kg.upsert_document("a", &["rust".into()], "ag").unwrap();
        kg.upsert_document("b", &["rust".into()], "ag").unwrap();

        // Only 1 shared tag → should NOT create AssociatesWith edge
        let n = kg.get_neighbors("a", Some(KGEdgeType::AssociatesWith), 1).unwrap();
        assert!(n.is_empty(), "single shared tag should not create AssociatesWith edge");
    }

    #[test]
    fn test_find_paths() {
        let kg = PetgraphBackend::new();
        // x → y → z
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(KGEdge { src: "x".into(), dst: "y".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();
        kg.add_edge(KGEdge { src: "y".into(), dst: "z".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();

        let paths = kg.find_paths("x", "z", 5).unwrap();
        assert!(!paths.is_empty());
        assert_eq!(paths[0].last().map(|n| n.id.as_str()), Some("z"));
    }

    #[test]
    fn test_list_nodes_by_type_and_agent() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("n1", KGNodeType::Document, vec![], "a")).unwrap();
        kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("n3", KGNodeType::Document, vec![], "a")).unwrap();
        kg.add_node(make_node("n4", KGNodeType::Document, vec![], "b")).unwrap(); // diff agent

        let docs = kg.list_nodes("a", Some(KGNodeType::Document)).unwrap();
        assert_eq!(docs.len(), 2);

        let all = kg.list_nodes("a", None).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_remove_node_cascades_edges() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(KGEdge { src: "x".into(), dst: "y".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();

        kg.remove_node("x").unwrap();
        assert!(kg.get_node("x").unwrap().is_none());
        let neighbors = kg.get_neighbors("y", None, 1).unwrap();
        assert!(neighbors.is_empty());
    }

    #[test]
    fn test_degree_centrality() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("center", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("leaf1", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("leaf2", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(KGEdge { src: "center".into(), dst: "leaf1".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();
        kg.add_edge(KGEdge { src: "center".into(), dst: "leaf2".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();
        kg.add_edge(KGEdge { src: "leaf1".into(), dst: "center".into(), edge_type: KGEdgeType::RelatedTo, weight: 1.0, evidence_cid: None, created_at: 0 }).unwrap();

        // center has 2 unique neighbors: {leaf1, leaf2}
        // (leaf1 is also bidirectional, but unique neighbors = set)
        assert_eq!(kg.degree("center"), 2);
        // leaf1 has 1 neighbor: {center} (outbound to center)
        assert_eq!(kg.degree("leaf1"), 1);
    }
}
