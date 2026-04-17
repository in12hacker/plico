//! Semantic Filesystem Types
//!
//! Core types for the AI-friendly semantic filesystem.
//! No paths, no directories — only semantic descriptions.

use serde::{Deserialize, Serialize};

// ── Query & Search ─────────────────────────────────────────────────────────────

/// Search query — can be tag-based, semantic, or mixed.
#[derive(Debug, Clone)]
pub enum Query {
    /// Find by exact CID (direct address).
    ByCid(String),
    /// Find by semantic tag(s).
    ByTags(Vec<String>),
    /// Find by natural language query (semantic search).
    /// Uses vector embeddings for semantic similarity.
    Semantic {
        text: String,
        filter: Option<crate::fs::search::SearchFilter>,
    },
    /// Find by content type.
    ByType(String),
    /// Mixed: tags + semantic query.
    Hybrid {
        tags: Vec<String>,
        semantic: Option<String>,
        content_type: Option<String>,
    },
}

/// A search result with relevance score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub cid: String,
    pub relevance: f32,
    pub meta: crate::cas::AIObjectMeta,
}

// ── Event types ───────────────────────────────────────────────────────────────

/// Event classification — stored as KGNode metadata for events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    Meeting,
    Presentation,
    Review,
    Interview,
    Travel,
    Entertainment,
    Social,
    Work,
    Personal,
    Other,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::Meeting => write!(f, "meeting"),
            EventType::Presentation => write!(f, "presentation"),
            EventType::Review => write!(f, "review"),
            EventType::Interview => write!(f, "interview"),
            EventType::Travel => write!(f, "travel"),
            EventType::Entertainment => write!(f, "entertainment"),
            EventType::Social => write!(f, "social"),
            EventType::Work => write!(f, "work"),
            EventType::Personal => write!(f, "personal"),
            EventType::Other => write!(f, "other"),
        }
    }
}

/// Event metadata — serialized into KGNode.metadata JSON field.
/// Avoids adding a new KGNodeType; reuses Entity nodes with this metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    pub label: String,
    pub event_type: EventType,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub location: Option<String>,
    pub attendee_ids: Vec<String>,
    pub related_cids: Vec<String>,
}

impl EventMeta {
    /// Returns true if this event's start_time falls within [since, until].
    /// If both bounds are None, returns true (no time constraint).
    pub fn in_range(&self, since: Option<u64>, until: Option<u64>) -> bool {
        let start = self.start_time.unwrap_or(0);
        if let Some(s) = since {
            if start < s { return false; }
        }
        if let Some(u) = until {
            if start > u { return false; }
        }
        true
    }
}

/// Relation type when attaching a target to an event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventRelation {
    /// Target is a Person (attendee of the event).
    Attendee,
    /// Target is a Document (content from the event).
    Document,
    /// Target is Media (photo, recording, etc. from the event).
    Media,
    /// Target is an ActionItem (decision, task, resolution from the event).
    Decision,
}

impl EventRelation {
    /// Maps relation type to corresponding KGEdgeType variant.
    pub fn to_edge_type(self) -> super::graph::KGEdgeType {
        use super::graph::KGEdgeType;
        match self {
            EventRelation::Attendee => KGEdgeType::HasAttendee,
            EventRelation::Document => KGEdgeType::HasDocument,
            EventRelation::Media => KGEdgeType::HasMedia,
            EventRelation::Decision => KGEdgeType::HasDecision,
        }
    }
}

/// A lightweight event summary returned by list_events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSummary {
    pub id: String,
    pub label: String,
    pub event_type: EventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
    pub attendee_count: usize,
    pub related_count: usize,
}

// ── Audit & Recycle ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecycleEntry {
    pub cid: String,
    pub deleted_at: u64,
    pub original_meta: crate::cas::AIObjectMeta,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: u64,
    pub action: AuditAction,
    pub cid: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuditAction {
    Create,
    Update { previous_cid: String },
    Delete,
}

// ── Errors ──────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FSError {
    #[error("Object not found: {0}")]
    NotFound(String),

    #[error("CAS error: {0}")]
    CAS(#[from] crate::cas::CASError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Embedding error: {0}")]
    Embedding(#[from] crate::fs::embedding::EmbedError),
}
