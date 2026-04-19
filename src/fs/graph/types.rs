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
