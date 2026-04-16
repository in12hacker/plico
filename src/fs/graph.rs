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

/// Type alias for the triple returned by `load_from_disk`.
type DiskGraph = (
    HashMap<String, KGNode>,
    HashMap<String, Vec<(String, KGEdge)>>,
    HashMap<String, Vec<(String, KGEdge)>>,
);

use serde::{Deserialize, Serialize};

/// Node type discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGNodeType {
    Entity,
    Fact,
    Document,
    Agent,
    Memory,
    // ── Project Self-Management (Dogfooding Plico) ────────────────────────
    /// An iteration/phase in the project lifecycle.
    Iteration,
    /// A plan item or milestone.
    Plan,
    /// A design document or specification.
    DesignDoc,
}

impl std::fmt::Display for KGNodeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KGNodeType::Entity => write!(f, "entity"),
            KGNodeType::Fact => write!(f, "fact"),
            KGNodeType::Document => write!(f, "document"),
            KGNodeType::Agent => write!(f, "agent"),
            KGNodeType::Memory => write!(f, "memory"),
            KGNodeType::Iteration => write!(f, "iteration"),
            KGNodeType::Plan => write!(f, "plan"),
            KGNodeType::DesignDoc => write!(f, "design_doc"),
        }
    }
}

/// Edge type discriminator.
///
/// # Serialization
/// All variants serialize to their snake_case string name via serde's default
/// derive. When loading persisted KG files, unknown variants cause a deserialize
/// error — acceptable during the prototype phase (no compatibility requirement).
/// Adding a new variant here requires updating this enum, the Display impl, and
/// any `match` arms that cover all variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGEdgeType {
    // ── General / existing ────────────────────────────────────────────────
    AssociatesWith,
    Follows,
    Mentions,
    Causes,
    Reminds,
    PartOf,
    SimilarTo,
    RelatedTo,
    // ── Event-specific relations ──────────────────────────────────────────
    /// Event → Person (attendee of the event).
    HasAttendee,
    /// Event → Document (content associated with the event).
    HasDocument,
    /// Event → Media (photo, recording, etc. from the event).
    HasMedia,
    /// Event → ActionItem (decision, task, or resolution from the event).
    HasDecision,
    // ── Reasoning / Action Suggestion edges ───────────────────────────────
    /// Person → UserFact (inferred preference from behavioral patterns).
    HasPreference,
    /// Preference node → suggested action (cross-event inference).
    SuggestsAction,
    /// Action suggestion → event that triggered it.
    MotivatedBy,
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
            KGEdgeType::HasAttendee => write!(f, "has_attendee"),
            KGEdgeType::HasDocument => write!(f, "has_document"),
            KGEdgeType::HasMedia => write!(f, "has_media"),
            KGEdgeType::HasDecision => write!(f, "has_decision"),
            KGEdgeType::SuggestsAction => write!(f, "suggests_action"),
            KGEdgeType::MotivatedBy => write!(f, "motivated_by"),
            KGEdgeType::HasPreference => write!(f, "has_preference"),
        }
    }
}

/// A node in the knowledge graph.
///
/// # Temporal Validity (Bi-temporal Model)
///
/// Per Graphiti's bi-temporal model:
/// - `created_at`: when this node was first ingested into the system
/// - `valid_at`: when this node became true/existent in the real world
/// - `invalid_at`: when this node was invalidated/merged (None = still valid)
/// - `expired_at`: when this node was soft-deleted (admin/user delete, None = active)
///
/// For example, after entity resolution merges "Wang Zong" and "王总" into one node,
/// the old node gets invalid_at set. Soft-deleted nodes are excluded from normal
/// queries but retained for audit.
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
    /// When this node became valid (Unix ms). None = unknown.
    #[serde(default)]
    pub valid_at: Option<u64>,
    /// When this node was invalidated (Unix ms). None = still valid.
    #[serde(default)]
    pub invalid_at: Option<u64>,
    /// Soft deletion marker (admin/user delete). Expired nodes are excluded from
    /// normal queries but retained for audit. None = active.
    #[serde(default)]
    pub expired_at: Option<u64>,
}

impl KGNode {
    /// Returns true if this node is currently valid at the given timestamp.
    ///
    /// An node is valid at time T if:
    /// - `valid_at <= T`, and
    /// - `invalid_at.is_none() || invalid_at > T`, and
    /// - `expired_at.is_none()` (soft-deleted nodes are excluded)
    pub fn is_valid_at(&self, t: u64) -> bool {
        self.valid_at.map_or(true, |v| v <= t)
            && self.invalid_at.map_or(true, |i| i > t)
            && self.expired_at.map_or(true, |e| e > t)
    }
}

/// An edge in the knowledge graph.
///
/// # Temporal Validity (Bi-temporal Model)
///
/// Per Graphiti's bi-temporal model:
/// - `created_at`: when this edge was first ingested into the system
/// - `valid_at`: when this edge became true in the real world
/// - `invalid_at`: when this edge was superseded in the real world (None = still valid)
/// - `expired_at`: when this edge was soft-deleted (admin/user delete, None = active)
///
/// Soft-deleted (`expired_at` set) edges are excluded from normal queries but retained for audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGEdge {
    pub src: String,
    pub dst: String,
    pub edge_type: KGEdgeType,
    /// 0.0–1.0. Used for scoring.
    pub weight: f32,
    pub evidence_cid: Option<String>,
    pub created_at: u64,
    /// When this edge became valid (Unix ms). None = unknown.
    #[serde(default)]
    pub valid_at: Option<u64>,
    /// When this edge was invalidated (Unix ms). None = still valid.
    #[serde(default)]
    pub invalid_at: Option<u64>,
    /// Soft deletion marker (admin/user delete). Expired edges are excluded from
    /// normal queries but retained for audit. None = active.
    #[serde(default)]
    pub expired_at: Option<u64>,
    /// Source episode IDs that produced or modified this fact.
    ///
    /// Enables provenance queries: "which conversation/event produced this fact?"
    /// Also enables credibility assessment — facts from official documents vs casual chat.
    ///
    /// Per Graphiti's provenance model: each fact links to the episodes that created it.
    #[serde(default)]
    pub episodes: Vec<String>,
}

impl KGEdge {
    /// Create a new edge with temporal validity initialized to now.
    ///
    /// `valid_at` is set to `now_ms()`, `invalid_at` is `None` (still valid).
    pub fn new(src: String, dst: String, edge_type: KGEdgeType, weight: f32) -> Self {
        let now = now_ms();
        Self {
            src,
            dst,
            edge_type,
            weight,
            evidence_cid: None,
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
            episodes: Vec::new(),
        }
    }

    /// Create a new edge with an initial episode ID.
    ///
    /// Use this when the edge is produced by a specific episode (e.g., document CID,
    /// conversation ID). The episode traces the provenance of this fact.
    ///
    /// Per Graphiti's provenance model: each fact links to the episodes that created it.
    pub fn new_with_episode(
        src: String,
        dst: String,
        edge_type: KGEdgeType,
        weight: f32,
        episode: impl Into<String>,
    ) -> Self {
        let now = now_ms();
        Self {
            src,
            dst,
            edge_type,
            weight,
            evidence_cid: None,
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
            episodes: vec![episode.into()],
        }
    }

    /// Returns true if this edge is currently valid at the given timestamp.
    ///
    /// An edge is valid at time T if:
    /// - `valid_at <= T`, and
    /// - `invalid_at.is_none() || invalid_at > T`, and
    /// - `expired_at.is_none()` (soft-deleted edges are excluded)
    pub fn is_valid_at(&self, t: u64) -> bool {
        self.valid_at.map_or(true, |v| v <= t)
            && self.invalid_at.map_or(true, |i| i > t)
            && self.expired_at.map_or(true, |e| e > t)
    }
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
    /// Return IDs of all nodes in the graph. Used for full-scan queries (e.g. event listing).
    fn all_node_ids(&self) -> Vec<String>;
    /// Upsert a Document node and auto-create AssociatesWith edges for shared ≥2 tags.
    fn upsert_document(&self, cid: &str, tags: &[String], agent_id: &str) -> Result<(), KGError>;
    /// Compute authority score (log-scaled degree, normalized 0–1).
    fn authority_score(&self, node_id: &str) -> Result<f32, KGError>;

    /// Number of nodes in the graph.
    fn node_count(&self) -> Result<usize, KGError>;

    /// Number of edges in the graph.
    fn edge_count(&self) -> Result<usize, KGError>;

    // ── Temporal query methods ─────────────────────────────────────────────

    /// Return edges valid at the given timestamp (per `KGEdge::is_valid_at`).
    ///
    /// This enables historical queries: "what facts were true at time T?"
    fn get_valid_edges_at(&self, t: u64) -> Result<Vec<KGEdge>, KGError>;

    /// Return the currently valid edge between two nodes, if any.
    ///
    /// Returns the most recently valid edge (highest `valid_at`) that is currently valid.
    fn get_valid_edge_between(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        t: u64,
    ) -> Result<Option<KGEdge>, KGError>;

    /// Invalidate all conflicting edges before adding a new one.
    ///
    /// Per Graphiti's conflict resolution: when a new fact contradicts a prior one,
    /// the old edge is invalidated (not deleted) to preserve history.
    ///
    /// Conflicts are edges where:
    /// - Same `(src, dst, edge_type)` exists
    /// - AND the existing edge is valid at the new edge's `valid_at`
    fn invalidate_conflicts(&self, new_edge: &KGEdge) -> Result<usize, KGError>;

    /// Return nodes valid at the given timestamp (per `KGNode::is_valid_at`).
    ///
    /// This enables historical queries: "what entities existed at time T?"
    fn get_valid_nodes_at(
        &self,
        agent_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> Result<Vec<KGNode>, KGError>;
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
    ) -> Result<DiskGraph, KGError> {
        let nodes: HashMap<String, KGNode> = serde_json::from_str(&std::fs::read_to_string(nodes_path)?)
            .map_err(KGError::Json)?;
        let edges: Vec<EdgeRecord> = serde_json::from_str(&std::fs::read_to_string(edges_path)?)
            .map_err(KGError::Json)?;

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
            valid_at: None,
            invalid_at: None,
            expired_at: None,
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
            // Both directions carry the new document's CID as the episode source.
            let e1 = KGEdge::new_with_episode(
                cid.to_string(),
                other_id.clone(),
                KGEdgeType::AssociatesWith,
                w,
                cid,
            );
            let e2 = KGEdge::new_with_episode(
                other_id.clone(),
                cid.to_string(),
                KGEdgeType::AssociatesWith,
                w,
                cid,
            );
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
        ((degree as f32).ln() / ((max_degree.max(1)) as f32).ln()).clamp(0.0, 1.0)
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
                .is_some_and(|v| v.iter().any(|(dst, _)| dst == &edge.dst))
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
                        if edge_type.is_none_or(|et| edge.edge_type == et) {
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
                        if edge_type.is_none_or(|et| edge.edge_type == et) {
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
            .filter(|n| n.agent_id == agent_id && node_type.is_none_or(|t| n.node_type == t))
            .cloned()
            .collect())
    }

    fn list_edges(&self, agent_id: &str) -> Result<Vec<KGEdge>, KGError> {
        let nodes = self.nodes.read().unwrap();
        let out = self.out_edges.read().unwrap();
        let mut edges = Vec::new();

        for (src, out_list) in out.iter() {
            if nodes.get(src).map(|n| n.agent_id == agent_id).unwrap_or(false) {
                for (_, edge) in out_list {
                    edges.push(edge.clone());
                }
            }
        }

        Ok(edges)
    }

    fn all_node_ids(&self) -> Vec<String> {
        let nodes = self.nodes.read().unwrap();
        nodes.keys().cloned().collect()
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

    fn node_count(&self) -> Result<usize, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes.len())
    }

    fn edge_count(&self) -> Result<usize, KGError> {
        let out = self.out_edges.read().unwrap();
        let total: usize = out.values().map(|v| v.len()).sum();
        Ok(total)
    }

    fn get_valid_edges_at(&self, t: u64) -> Result<Vec<KGEdge>, KGError> {
        let out = self.out_edges.read().unwrap();
        let mut valid = Vec::new();
        for (_, out_list) in out.iter() {
            for (_, edge) in out_list {
                if edge.is_valid_at(t) {
                    valid.push(edge.clone());
                }
            }
        }
        Ok(valid)
    }

    fn get_valid_edge_between(
        &self,
        src: &str,
        dst: &str,
        edge_type: Option<KGEdgeType>,
        t: u64,
    ) -> Result<Option<KGEdge>, KGError> {
        let out = self.out_edges.read().unwrap();
        let candidates = out.get(src).map(|v| {
            v.iter()
                .filter(|(d, e)| *d == dst && edge_type.is_none_or(|et| e.edge_type == et))
                .filter(|(_, e)| e.is_valid_at(t))
                .collect::<Vec<_>>()
        });
        Ok(candidates
            .and_then(|c| {
                c.iter()
                    .filter(|(_, e)| e.is_valid_at(t))
                    .max_by_key(|(_, e)| e.valid_at)
                    .map(|(_, e)| e.clone())
            }))
    }

    fn invalidate_conflicts(&self, new_edge: &KGEdge) -> Result<usize, KGError> {
        let now = now_ms();
        let t = new_edge.valid_at.unwrap_or(now);

        // Phase 1: collect conflicts (read-only)
        let conflicts: Vec<(String, String)> = {
            let out = self.out_edges.read().unwrap();
            out.get(&new_edge.src)
                .map(|edges| {
                    edges
                        .iter()
                        .filter(|(_, e)| {
                            e.dst == new_edge.dst
                                && e.edge_type == new_edge.edge_type
                                && e.is_valid_at(t)
                        })
                        .map(|(d, _)| (new_edge.src.clone(), d.clone()))
                        .collect()
                })
                .unwrap_or_default()
        };

        // Phase 2: invalidate conflicts (write)
        if conflicts.is_empty() {
            return Ok(0);
        }

        let mut out = self.out_edges.write().unwrap();
        let mut in_edges = self.in_edges.write().unwrap();
        let mut count = 0;

        for (src, dst) in conflicts {
            if let Some(list) = out.get_mut(&src) {
                for e in list.iter_mut() {
                    if e.1.dst == dst && e.1.edge_type == new_edge.edge_type {
                        e.1.invalid_at = Some(now);
                        count += 1;
                    }
                }
            }
            if let Some(list) = in_edges.get_mut(&dst) {
                for e in list.iter_mut() {
                    if e.1.src == src && e.1.edge_type == new_edge.edge_type {
                        e.1.invalid_at = Some(now);
                    }
                }
            }
        }
        drop(out);
        drop(in_edges);
        self.persist();
        Ok(count)
    }

    fn get_valid_nodes_at(
        &self,
        agent_id: &str,
        node_type: Option<KGNodeType>,
        t: u64,
    ) -> Result<Vec<KGNode>, KGError> {
        let nodes = self.nodes.read().unwrap();
        Ok(nodes
            .values()
            .filter(|n| {
                n.agent_id == agent_id
                    && node_type.is_none_or(|nt| n.node_type == nt)
                    && n.is_valid_at(t)
            })
            .cloned()
            .collect())
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
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        }
    }

    /// Create a test edge with temporal validity fields initialized.
    fn make_edge(src: &str, dst: &str, edge_type: KGEdgeType, weight: f32) -> KGEdge {
        KGEdge::new(src.to_string(), dst.to_string(), edge_type, weight)
    }

    // ── is_valid_at tests ─────────────────────────────────────────────────────

    #[test]
    fn test_is_valid_at_currently_valid() {
        let edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        let now = now_ms();
        assert!(edge.is_valid_at(now), "newly created edge should be valid at current time");
        assert!(edge.is_valid_at(now + 1000), "edge should be valid in the future");
    }

    #[test]
    fn test_is_valid_at_after_invalidation() {
        let mut edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        edge.invalid_at = Some(now_ms() + 1000);
        assert!(edge.is_valid_at(now_ms()), "edge should be valid before invalid_at");
        assert!(!edge.is_valid_at(now_ms() + 2000), "edge should be invalid after invalid_at");
    }

    #[test]
    fn test_is_valid_at_with_valid_at_in_future() {
        let mut edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        edge.valid_at = Some(now_ms() + 1000); // becomes valid in the future
        edge.invalid_at = None;
        assert!(!edge.is_valid_at(now_ms()), "edge should not be valid before valid_at");
        assert!(edge.is_valid_at(now_ms() + 2000), "edge should be valid after valid_at");
    }

    #[test]
    fn test_is_valid_at_edge_case_boundaries() {
        let mut edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        let t = 1000;
        edge.valid_at = Some(t);
        edge.invalid_at = Some(t + 100);
        assert!(edge.is_valid_at(t), "valid_at boundary: edge is valid at exact valid_at");
        assert!(!edge.is_valid_at(t - 1), "edge invalid before valid_at");
        assert!(!edge.is_valid_at(t + 100), "invalid_at boundary: edge invalid at exact invalid_at");
        assert!(edge.is_valid_at(t + 99), "edge valid at invalid_at - 1");
    }

    // ── get_valid_edges_at tests ──────────────────────────────────────────────

    #[test]
    fn test_get_valid_edges_at_filters_by_time() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "agent1")).unwrap();

        let mut edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        edge.valid_at = Some(1000);
        edge.invalid_at = Some(2000);
        kg.add_edge(edge).unwrap();

        assert!(kg.get_valid_edges_at(500).unwrap().is_empty(), "before valid_at");
        assert_eq!(kg.get_valid_edges_at(1000).unwrap().len(), 1, "at valid_at");
        assert_eq!(kg.get_valid_edges_at(1500).unwrap().len(), 1, "between valid_at and invalid_at");
        assert!(kg.get_valid_edges_at(2000).unwrap().is_empty(), "at invalid_at boundary");
        assert!(kg.get_valid_edges_at(3000).unwrap().is_empty(), "after invalid_at");
    }

    #[test]
    fn test_get_valid_edges_at_current_time() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();

        let now = now_ms();
        let valid = kg.get_valid_edges_at(now).unwrap();
        assert_eq!(valid.len(), 1);
    }

    #[test]
    fn test_get_valid_edges_at_no_edges() {
        let kg = PetgraphBackend::new();
        assert!(kg.get_valid_edges_at(now_ms()).unwrap().is_empty());
    }

    // ── get_valid_edge_between tests ─────────────────────────────────────────

    #[test]
    fn test_get_valid_edge_between_returns_most_recent() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("p", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("w1", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("w2", KGNodeType::Entity, vec![], "a")).unwrap();

        // Two different preference edges: p→w1 (valid_at=1000) and p→w2 (valid_at=2000)
        let e1 = {
            let mut e = make_edge("p", "w1", KGEdgeType::HasPreference, 0.8);
            e.valid_at = Some(1000);
            e
        };
        kg.add_edge(e1).unwrap();

        let e2 = {
            let mut e = make_edge("p", "w2", KGEdgeType::HasPreference, 0.9);
            e.valid_at = Some(2000);
            e
        };
        kg.add_edge(e2).unwrap();

        // At time 1500, only e1 is valid (prefers w1 at t=1000)
        let found = kg.get_valid_edge_between("p", "w1", None, 1500).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().valid_at, Some(1000));

        // At time 2500, e2 is valid (prefers w2 at t=2000)
        let found = kg.get_valid_edge_between("p", "w2", None, 2500).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().valid_at, Some(2000));
    }

    #[test]
    fn test_get_valid_edge_between_filters_by_edge_type() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y1", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y2", KGNodeType::Entity, vec![], "a")).unwrap();

        // Two edges: x→y1 (RelatedTo) and x→y2 (Mentions)
        kg.add_edge(make_edge("x", "y1", KGEdgeType::RelatedTo, 1.0)).unwrap();
        kg.add_edge(make_edge("x", "y2", KGEdgeType::Mentions, 0.5)).unwrap();

        let found = kg.get_valid_edge_between("x", "y2", Some(KGEdgeType::HasPreference), now_ms()).unwrap();
        assert!(found.is_none(), "HasPreference edge does not exist");
        let found = kg.get_valid_edge_between("x", "y1", Some(KGEdgeType::RelatedTo), now_ms()).unwrap();
        assert!(found.is_some());
    }

    // ── invalidate_conflicts tests ───────────────────────────────────────────

    #[test]
    fn test_invalidate_conflicts_replaces_prior_edge() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("person", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("wine", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("beer", KGNodeType::Entity, vec![], "a")).unwrap();

        // person prefers wine
        let old = KGEdge::new("person".into(), "wine".into(), KGEdgeType::HasPreference, 0.9);
        kg.add_edge(old).unwrap();

        // New preference: person prefers beer (different dst, same edge_type)
        let new = KGEdge::new("person".into(), "beer".into(), KGEdgeType::HasPreference, 0.85);
        let count = kg.invalidate_conflicts(&new).unwrap();
        // Different dst → no same-(src,dst,type) conflict → count = 0
        assert_eq!(count, 0);
        // wine preference still exists (not invalidated since different dst)
        let still_valid = kg.get_valid_edge_between("person", "wine", None, now_ms()).unwrap();
        assert!(still_valid.is_some());
    }

    #[test]
    fn test_invalidate_conflicts_none_found() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "x")).unwrap();

        let edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        kg.add_edge(edge).unwrap();

        let new = make_edge("c", "d", KGEdgeType::RelatedTo, 0.5);
        let count = kg.invalidate_conflicts(&new).unwrap();
        assert_eq!(count, 0, "no conflicts for unrelated edge");
    }

    #[test]
    fn test_invalidate_conflicts_preserves_history() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("s", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("t", KGNodeType::Entity, vec![], "a")).unwrap();

        let old_time = now_ms() - 1000;
        let mut old_edge = make_edge("s", "t", KGEdgeType::HasPreference, 0.9);
        old_edge.valid_at = Some(old_time);
        kg.add_edge(old_edge).unwrap();

        let new_edge = KGEdge::new("s".into(), "t".into(), KGEdgeType::HasPreference, 0.8);
        kg.invalidate_conflicts(&new_edge).unwrap();

        // History preserved: can still query at old_time
        let historical = kg.get_valid_edge_between("s", "t", None, old_time).unwrap();
        assert!(historical.is_some(), "historical edge should still be queryable");
    }

    // ── get_valid_nodes_at tests ──────────────────────────────────────────────

    #[test]
    fn test_get_valid_nodes_at_filters_by_time() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("entity1", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_node(make_node("entity2", KGNodeType::Entity, vec![], "agent1")).unwrap();

        // entity1 becomes valid at t=1000, invalid at t=2000
        let mut node1 = make_node("entity1", KGNodeType::Entity, vec![], "agent1");
        node1.valid_at = Some(1000);
        node1.invalid_at = Some(2000);
        kg.add_node(node1).unwrap();

        // entity2 is always valid (default)
        let node2 = make_node("entity2", KGNodeType::Entity, vec![], "agent1");
        kg.add_node(node2).unwrap();

        // entity2 is always valid (valid_at=None), so it's returned at all times
        let at_500 = kg.get_valid_nodes_at("agent1", None, 500).unwrap();
        assert_eq!(at_500.len(), 1, "entity2 always valid");
        assert_eq!(at_500[0].id, "entity2");
        let at_1000 = kg.get_valid_nodes_at("agent1", None, 1000).unwrap();
        assert_eq!(at_1000.len(), 2, "at valid_at both nodes valid");
        let at_1500 = kg.get_valid_nodes_at("agent1", None, 1500).unwrap();
        assert_eq!(at_1500.len(), 2, "both still valid at 1500");
        // At exact invalid_at boundary, entity1 is NOT valid
        let at_2000 = kg.get_valid_nodes_at("agent1", None, 2000).unwrap();
        assert_eq!(at_2000.len(), 1, "entity1 invalid at exact invalid_at boundary, only entity2");
        let at_2500 = kg.get_valid_nodes_at("agent1", None, 2500).unwrap();
        assert_eq!(at_2500.len(), 1, "only entity2 valid after 2000");
        assert!(at_2500[0].id == "entity2");
    }

    #[test]
    fn test_get_valid_nodes_at_filters_by_agent_and_type() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("e1", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_node(make_node("e2", KGNodeType::Entity, vec![], "agent2")).unwrap();
        kg.add_node(make_node("d1", KGNodeType::Document, vec![], "agent1")).unwrap();

        let now = now_ms();
        let all_agent1 = kg.get_valid_nodes_at("agent1", None, now).unwrap();
        assert_eq!(all_agent1.len(), 2);

        let entities_only = kg.get_valid_nodes_at("agent1", Some(KGNodeType::Entity), now).unwrap();
        assert_eq!(entities_only.len(), 1);
        assert_eq!(entities_only[0].id, "e1");

        let other_agent = kg.get_valid_nodes_at("agent2", None, now).unwrap();
        assert_eq!(other_agent.len(), 1);
        assert_eq!(other_agent[0].id, "e2");
    }

    #[test]
    fn test_get_valid_nodes_at_respects_expired() {
        let kg = PetgraphBackend::new();
        let mut node = make_node("expired_node", KGNodeType::Entity, vec![], "a");
        node.expired_at = Some(now_ms() - 1000); // soft-deleted in the past
        kg.add_node(node).unwrap();

        let now = now_ms();
        assert!(kg.get_valid_nodes_at("a", None, now).unwrap().is_empty(), "soft-deleted node excluded");
        // But it was valid before expiration
        let before = kg.get_valid_nodes_at("a", None, now_ms() - 2000).unwrap();
        assert_eq!(before.len(), 1, "node valid before expired_at");
    }

    // ── existing tests updated ────────────────────────────────────────────────

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

        kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.9)).unwrap();

        let neighbors = kg.get_neighbors("n1", None, 1).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].0.id, "n2");
    }

    #[test]
    fn test_edge_already_exists_error() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "x")).unwrap();

        kg.add_edge(make_edge("a", "b", KGEdgeType::RelatedTo, 0.5)).unwrap();

        let err = kg.add_edge(make_edge("a", "b", KGEdgeType::Mentions, 0.5)).unwrap_err();
        assert!(matches!(err, KGError::EdgeExists(_, _, _)));
    }

    #[test]
    fn test_neighbors_bidirectional() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(make_edge("x", "y", KGEdgeType::Follows, 1.0)).unwrap();

        let n = kg.get_neighbors("y", None, 1).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].0.id, "x");
    }

    #[test]
    fn test_neighbors_depth2_chain() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("a", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("b", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_node(make_node("c", KGNodeType::Entity, vec![], "x")).unwrap();
        kg.add_edge(make_edge("a", "b", KGEdgeType::Follows, 1.0)).unwrap();
        kg.add_edge(make_edge("b", "c", KGEdgeType::Follows, 1.0)).unwrap();

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

        kg.upsert_document("doc1", &["rust".into(), "async".into(), "concurrency".into()], "agent1")
            .unwrap();

        kg.upsert_document("doc2", &["rust".into(), "async".into(), "networking".into()], "agent1")
            .unwrap();

        let n1_neighbors = kg.get_neighbors("doc1", Some(KGEdgeType::AssociatesWith), 1).unwrap();
        assert!(!n1_neighbors.is_empty(), "doc1 should have AssociatesWith edge to doc2");
    }

    #[test]
    fn test_upsert_document_episodes_populated() {
        // Verifies that upsert_document creates edges whose episodes include the source document CID.
        // Per Graphiti provenance model: each fact links to the episode that produced it.
        let kg = PetgraphBackend::new();

        // Create two docs sharing ≥2 tags so an AssociatesWith edge is created.
        kg.upsert_document("doc_a", &["x".into(), "y".into(), "z".into()], "agent1")
            .unwrap();
        kg.upsert_document("doc_b", &["x".into(), "y".into(), "w".into()], "agent1")
            .unwrap();

        // The edge doc_b → doc_a (and vice versa) should have doc_b as an episode.
        let neighbors = kg
            .get_neighbors("doc_b", Some(KGEdgeType::AssociatesWith), 1)
            .unwrap();
        let edge_to_a = neighbors
            .iter()
            .find(|(n, _)| n.id == "doc_a")
            .map(|(_, e)| e);

        assert!(
            edge_to_a.is_some(),
            "doc_b should have an AssociatesWith edge to doc_a"
        );
        let edge = edge_to_a.unwrap();
        assert!(
            edge.episodes.contains(&"doc_b".to_string()),
            "edge episodes should contain source doc_b CID, got {:?}",
            edge.episodes
        );
    }

    #[test]
    fn test_no_associate_single_tag() {
        let kg = PetgraphBackend::new();

        kg.upsert_document("a", &["rust".into()], "ag").unwrap();
        kg.upsert_document("b", &["rust".into()], "ag").unwrap();

        let n = kg.get_neighbors("a", Some(KGEdgeType::AssociatesWith), 1).unwrap();
        assert!(n.is_empty(), "single shared tag should not create AssociatesWith edge");
    }

    #[test]
    fn test_find_paths() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
        kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();
        kg.add_edge(make_edge("y", "z", KGEdgeType::RelatedTo, 1.0)).unwrap();

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
        kg.add_node(make_node("n4", KGNodeType::Document, vec![], "b")).unwrap();

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
        kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();

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
        kg.add_edge(make_edge("center", "leaf1", KGEdgeType::RelatedTo, 1.0)).unwrap();
        kg.add_edge(make_edge("center", "leaf2", KGEdgeType::RelatedTo, 1.0)).unwrap();
        kg.add_edge(make_edge("leaf1", "center", KGEdgeType::RelatedTo, 1.0)).unwrap();

        assert_eq!(kg.degree("center"), 2);
        assert_eq!(kg.degree("leaf1"), 1);
    }

    // ── Event edge types ──────────────────────────────────────────────────────

    #[test]
    fn test_event_edge_types_serialize_roundtrip() {
        let variants = [
            KGEdgeType::HasAttendee,
            KGEdgeType::HasDocument,
            KGEdgeType::HasMedia,
            KGEdgeType::HasDecision,
            KGEdgeType::HasPreference,
        ];

        for edge_type in variants {
            let json = serde_json::to_string(&edge_type).unwrap();
            let roundtrip: KGEdgeType = serde_json::from_str(&json).unwrap();
            assert_eq!(edge_type, roundtrip, "serde roundtrip must preserve {:?}", edge_type);
        }
    }

    #[test]
    fn test_event_edge_types_display() {
        assert_eq!(KGEdgeType::HasAttendee.to_string(), "has_attendee");
        assert_eq!(KGEdgeType::HasDocument.to_string(), "has_document");
        assert_eq!(KGEdgeType::HasMedia.to_string(), "has_media");
        assert_eq!(KGEdgeType::HasDecision.to_string(), "has_decision");
        assert_eq!(KGEdgeType::SuggestsAction.to_string(), "suggests_action");
        assert_eq!(KGEdgeType::MotivatedBy.to_string(), "motivated_by");
        assert_eq!(KGEdgeType::HasPreference.to_string(), "has_preference");
    }

    #[test]
    fn test_event_edge_add_and_query() {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("evt1", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_node(make_node("person1", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_edge(make_edge("evt1", "person1", KGEdgeType::HasAttendee, 1.0)).unwrap();

        let attendees = kg.get_neighbors("evt1", Some(KGEdgeType::HasAttendee), 1).unwrap();
        assert_eq!(attendees.len(), 1, "event should have 1 attendee");
        assert_eq!(attendees[0].0.label, "person1");

        kg.add_node(make_node("doc1", KGNodeType::Document, vec![], "agent1")).unwrap();
        kg.add_edge(make_edge("evt1", "doc1", KGEdgeType::HasDocument, 1.0)).unwrap();

        let docs = kg.get_neighbors("evt1", Some(KGEdgeType::HasDocument), 1).unwrap();
        assert_eq!(docs.len(), 1, "event should have 1 document");
        assert_eq!(docs[0].0.label, "doc1");

        let attendees_only = kg.get_neighbors("evt1", Some(KGEdgeType::HasAttendee), 1).unwrap();
        assert!(attendees_only.iter().all(|(n, _)| n.id != "doc1"), "HasAttendee filter must exclude documents");
    }

    // ── Temporal validity edge tests ─────────────────────────────────────────

    #[test]
    fn test_temporal_edge_new_has_valid_at() {
        let edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
        assert!(edge.valid_at.is_some(), "new edge should have valid_at set");
        assert!(edge.invalid_at.is_none(), "new edge should have invalid_at = None");
    }

    #[test]
    fn test_edge_serialization_includes_temporal_fields() {
        let edge = make_edge("a", "b", KGEdgeType::RelatedTo, 0.8);
        let json = serde_json::to_string(&edge).unwrap();
        let roundtrip: KGEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(roundtrip.valid_at, edge.valid_at);
        assert_eq!(roundtrip.invalid_at, edge.invalid_at);
    }
}
