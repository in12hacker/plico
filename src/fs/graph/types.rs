//! Knowledge Graph Types
//!
//! Core types for entity and relationship tracking in the knowledge graph.
//! Nodes represent entities, facts, and memory blocks from CAS objects.
//! Edges represent typed relationships (RelatedTo, Follows, Mentions, etc.).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Node types ────────────────────────────────────────────────────────────────

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
    /// Event → Participant (agent or user involved in the event).
    HasParticipant,
    /// Event → Artifact (AI-generated content from the event).
    HasArtifact,
    /// Event → Recording (log, data output from the event).
    HasRecording,
    /// Event → Resolution (decision, conclusion from the event).
    HasResolution,
    // ── Reasoning edges (AI-native) ─────────────────────────────────────
    /// Agent → Fact (knowledge graph assertion).
    HasFact,
    // ── Version tracking ─────────────────────────────────────────────────
    /// New CID → Old CID (version chain for rollback).
    Supersedes,
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
            KGEdgeType::HasParticipant => write!(f, "has_participant"),
            KGEdgeType::HasArtifact => write!(f, "has_artifact"),
            KGEdgeType::HasRecording => write!(f, "has_recording"),
            KGEdgeType::HasResolution => write!(f, "has_resolution"),
            KGEdgeType::HasFact => write!(f, "has_fact"),
            KGEdgeType::Supersedes => write!(f, "supersedes"),
        }
    }
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

/// A hybrid search result combining vector similarity and graph authority.
#[derive(Debug, Clone)]
pub struct KGSearchHit {
    pub node: KGNode,
    pub edge_type: Option<KGEdgeType>,
    pub vector_score: f32,
    pub authority_score: f32,
    pub combined_score: f32,
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
    /// Tenant ID for multi-tenant isolation.
    #[serde(default)]
    pub tenant_id: String,
    pub created_at: u64,
    /// When this node became valid (Unix ms). None = unknown.
    #[serde(default)]
    pub valid_at: Option<u64>,
    /// When this node was invalidated (Unix ms). None = still valid.
    #[serde(default)]
    pub invalid_at: Option<u64>,
    /// When this node was soft-deleted (Unix ms). None = active.
    #[serde(default)]
    pub expired_at: Option<u64>,
}

impl KGNode {
    /// Create a new node with a UUID id.
    pub fn new(label: String, node_type: KGNodeType, agent_id: String, tenant_id: String) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            node_type,
            content_cid: None,
            properties: serde_json::Value::Null,
            agent_id,
            tenant_id,
            created_at: now_ms(),
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        }
    }

    /// Create a node with a content CID reference.
    pub fn with_content(
        label: String,
        node_type: KGNodeType,
        content_cid: String,
        agent_id: String,
        tenant_id: String,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label,
            node_type,
            content_cid: Some(content_cid),
            properties: serde_json::Value::Null,
            agent_id,
            tenant_id,
            created_at: now_ms(),
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        }
    }

    /// Returns true if the node is currently valid (not invalidated and not expired).
    pub fn is_active(&self) -> bool {
        self.invalid_at.is_none() && self.expired_at.is_none()
    }

    /// Returns true if this node is currently valid at the given timestamp.
    ///
    /// A node is valid at time T if:
    /// - `valid_at <= T`, and
    /// - `invalid_at.is_none() || invalid_at > T`, and
    /// - `expired_at.is_none()` (soft-deleted nodes are excluded)
    pub fn is_valid_at(&self, t: u64) -> bool {
        self.valid_at.is_none_or(|v| v <= t)
            && self.invalid_at.is_none_or(|i| i > t)
            && self.expired_at.is_none_or(|e| e > t)
    }
}

/// An edge in the knowledge graph.
///
/// `episode` field enables Graphiti-style provenance: track which event/context
/// introduced each fact, enabling fact-level time-travel and conflict detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KGEdge {
    /// Source node ID.
    pub src: String,
    /// Destination node ID.
    pub dst: String,
    /// Edge type.
    pub edge_type: KGEdgeType,
    /// Confidence/weight in [0, 1].
    #[serde(default)]
    pub weight: f32,
    /// Optional CID of a CAS object providing evidence for this edge.
    #[serde(default)]
    pub evidence_cid: Option<String>,
    pub created_at: u64,
    /// When this edge became valid (Unix ms). None = unknown.
    #[serde(default)]
    pub valid_at: Option<u64>,
    /// When this edge was invalidated (Unix ms). None = still valid.
    #[serde(default)]
    pub invalid_at: Option<u64>,
    /// When this edge was soft-deleted (Unix ms). None = active.
    #[serde(default)]
    pub expired_at: Option<u64>,
    /// The episode (event/context) that produced this edge.
    /// Per Graphiti's provenance model: each fact links to the episodes that created it.
    #[serde(default)]
    pub episode: Option<String>,
}

impl KGEdge {
    /// Create a new edge.
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
            episode: None,
        }
    }

    /// Create an edge with episode provenance.
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
            episode: Some(episode.into()),
        }
    }

    /// Returns true if the edge is currently valid (not invalidated and not expired).
    pub fn is_active(&self) -> bool {
        self.invalid_at.is_none() && self.expired_at.is_none()
    }

    /// Returns true if this edge is currently valid at the given timestamp.
    ///
    /// An edge is valid at time T if:
    /// - `valid_at <= T`, and
    /// - `invalid_at.is_none() || invalid_at > T`, and
    /// - `expired_at.is_none()` (soft-deleted edges are excluded)
    pub fn is_valid_at(&self, t: u64) -> bool {
        self.valid_at.is_none_or(|v| v <= t)
            && self.invalid_at.is_none_or(|i| i > t)
            && self.expired_at.is_none_or(|e| e > t)
    }
}

// ── Disk format ────────────────────────────────────────────────────────────────

/// Type alias for the triple returned by `load_from_disk`.
pub type DiskGraph = (
    HashMap<String, KGNode>,
    HashMap<String, Vec<(String, KGEdge)>>,
    HashMap<String, Vec<(String, KGEdge)>>,
);

// ── Time utility ──────────────────────────────────────────────────────────────

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── KGNodeType ─────────────────────────────────────────────────────────

    #[test]
    fn test_kg_node_type_display() {
        assert_eq!(KGNodeType::Entity.to_string(), "entity");
        assert_eq!(KGNodeType::Fact.to_string(), "fact");
        assert_eq!(KGNodeType::Document.to_string(), "document");
        assert_eq!(KGNodeType::Agent.to_string(), "agent");
        assert_eq!(KGNodeType::Memory.to_string(), "memory");
    }

    #[test]
    fn test_kg_node_type_debug() {
        assert_eq!(format!("{:?}", KGNodeType::Entity), "Entity");
        assert_eq!(format!("{:?}", KGNodeType::Fact), "Fact");
    }

    // ─── KGEdgeType ─────────────────────────────────────────────────────────

    #[test]
    fn test_kg_edge_type_display() {
        assert_eq!(KGEdgeType::AssociatesWith.to_string(), "associates_with");
        assert_eq!(KGEdgeType::Follows.to_string(), "follows");
        assert_eq!(KGEdgeType::SimilarTo.to_string(), "similar_to");
        assert_eq!(KGEdgeType::HasParticipant.to_string(), "has_participant");
        assert_eq!(KGEdgeType::Supersedes.to_string(), "supersedes");
    }

    // ─── KGNode ─────────────────────────────────────────────────────────────

    #[test]
    fn test_kg_node_new() {
        let node = KGNode::new(
            "TestNode".to_string(),
            KGNodeType::Entity,
            "agent1".to_string(),
            "tenant1".to_string(),
        );
        assert_eq!(node.label, "TestNode");
        assert_eq!(node.node_type, KGNodeType::Entity);
        assert_eq!(node.agent_id, "agent1");
        assert_eq!(node.tenant_id, "tenant1");
        assert!(!node.id.is_empty());
        assert!(node.content_cid.is_none());
    }

    #[test]
    fn test_kg_node_with_content() {
        let node = KGNode::with_content(
            "DocNode".to_string(),
            KGNodeType::Document,
            "abc123".to_string(),
            "agent1".to_string(),
            "tenant1".to_string(),
        );
        assert_eq!(node.content_cid, Some("abc123".to_string()));
    }

    #[test]
    fn test_kg_node_is_active() {
        let node = KGNode::new("Test".to_string(), KGNodeType::Entity, "agent".to_string(), "default".to_string());
        assert!(node.is_active());
    }

    #[test]
    fn test_kg_node_is_valid_at() {
        let node = KGNode::new("Test".to_string(), KGNodeType::Entity, "agent".to_string(), "default".to_string());
        // Node created now, should be valid at current time
        let now = now_ms();
        assert!(node.is_valid_at(now));
        // Should also be valid at 0
        assert!(node.is_valid_at(0));
    }

    #[test]
    fn test_kg_node_json_serialization() {
        let node = KGNode::new("Test".to_string(), KGNodeType::Fact, "agent".to_string(), "tenant".to_string());
        let json = serde_json::to_string(&node).unwrap();
        let deserialized: KGNode = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.label, node.label);
        assert_eq!(deserialized.node_type, node.node_type);
    }

    // ─── KGEdge ─────────────────────────────────────────────────────────────

    #[test]
    fn test_kg_edge_new() {
        let edge = KGEdge::new(
            "node1".to_string(),
            "node2".to_string(),
            KGEdgeType::RelatedTo,
            0.8,
        );
        assert_eq!(edge.src, "node1");
        assert_eq!(edge.dst, "node2");
        assert_eq!(edge.edge_type, KGEdgeType::RelatedTo);
        assert!((edge.weight - 0.8).abs() < f32::EPSILON);
        assert!(edge.is_active());
    }

    #[test]
    fn test_kg_edge_new_with_episode() {
        let edge = KGEdge::new_with_episode(
            "node1".to_string(),
            "node2".to_string(),
            KGEdgeType::SimilarTo,
            0.5,
            "event-123",
        );
        assert_eq!(edge.episode, Some("event-123".to_string()));
    }

    #[test]
    fn test_kg_edge_is_active() {
        let edge = KGEdge::new("a".to_string(), "b".to_string(), KGEdgeType::Follows, 1.0);
        assert!(edge.is_active());
    }

    #[test]
    fn test_kg_edge_is_valid_at() {
        let edge = KGEdge::new("a".to_string(), "b".to_string(), KGEdgeType::RelatedTo, 1.0);
        let now = now_ms();
        assert!(edge.is_valid_at(now));
    }

    #[test]
    fn test_kg_edge_json_serialization() {
        let edge = KGEdge::new_with_episode(
            "src".to_string(),
            "dst".to_string(),
            KGEdgeType::HasFact,
            0.9,
            "ep1",
        );
        let json = serde_json::to_string(&edge).unwrap();
        let deserialized: KGEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.src, edge.src);
        assert_eq!(deserialized.dst, edge.dst);
        assert_eq!(deserialized.episode, edge.episode);
    }

    // ─── KGError ────────────────────────────────────────────────────────────

    #[test]
    fn test_kg_error_display() {
        let err = KGError::NodeNotFound("node-x".to_string());
        assert_eq!(err.to_string(), "Node not found: node-x");
    }

    #[test]
    fn test_kg_error_io_roundtrip() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let kg_err = KGError::Io(io_err);
        assert!(kg_err.to_string().contains("test"));
    }

    // ─── KGSearchHit ────────────────────────────────────────────────────────

    #[test]
    fn test_kg_search_hit_clone() {
        let node = KGNode::new("Test".to_string(), KGNodeType::Entity, "agent".to_string(), "tenant".to_string());
        let hit = KGSearchHit {
            node,
            edge_type: Some(KGEdgeType::RelatedTo),
            vector_score: 0.9,
            authority_score: 0.5,
            combined_score: 0.7,
        };
        let cloned = hit.clone();
        assert_eq!(cloned.node.label, "Test");
        assert_eq!(cloned.combined_score, 0.7);
    }

    // ─── DiskGraph ──────────────────────────────────────────────────────────

    #[test]
    fn test_disk_graph_type_alias() {
        let graph: DiskGraph = (
            HashMap::new(),
            HashMap::new(),
            HashMap::new(),
        );
        assert_eq!(graph.0.len(), 0);
        assert_eq!(graph.1.len(), 0);
        assert_eq!(graph.2.len(), 0);
    }
}
