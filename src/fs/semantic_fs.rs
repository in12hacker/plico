//! Semantic Filesystem Implementation
//!
//! Provides AI-friendly CRUD operations. No paths — only semantic descriptions.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::{EmbeddingProvider, EmbedError};
use crate::fs::search::{SemanticSearch, SearchFilter, SearchIndexMeta};
use crate::fs::summarizer::Summarizer;
use crate::fs::graph::{KnowledgeGraph, KGNode, KGNodeType, KGEdge, KGEdgeType};
use crate::temporal::TemporalResolver;

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
        filter: Option<SearchFilter>,
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
    pub meta: AIObjectMeta,
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
/// This is the "EventContainer" concept from plico-kg-entity-design.md §2.1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    pub label: String,
    pub event_type: EventType,
    /// Start time as Unix milliseconds. None = unknown.
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub location: Option<String>,
    /// Person KG node IDs of attendees.
    pub attendee_ids: Vec<String>,
    /// CAS CIDs of related content (documents, media, etc.).
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
    fn edge_type(self) -> KGEdgeType {
        match self {
            EventRelation::Attendee => KGEdgeType::HasAttendee,
            EventRelation::Document => KGEdgeType::HasDocument,
            EventRelation::Media => KGEdgeType::HasMedia,
            EventRelation::Decision => KGEdgeType::HasDecision,
        }
    }
}

// ── Reasoning / Action Suggestion types ────────────────────────────────────────

/// Confidence and status of an action suggestion.
///
/// Low confidence → show reasoning chain to user for confirmation.
/// High confidence → auto-act (proactive scheduler).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    /// Awaiting user confirmation or more evidence.
    Pending,
    /// User confirmed; ready to act.
    Confirmed,
    /// User dismissed or confidence dropped below threshold.
    Dismissed,
}

/// The minimum evidence count before a preference is considered actionable.
/// Per plico-multi-hop-reasoning.md §6.3.
pub const PREFERENCE_MIN_CONFIDENCE: f32 = 0.4;
/// Confidence level above which suggestions auto-fire (no confirmation needed).
pub const PREFERENCE_HIGH_CONFIDENCE: f32 = 0.8;

// ── BehavioralObservation Pipeline (Phase C) ───────────────────────────────────

/// A single behavioral observation — an externally-injected event record.
///
/// The system does not observe behavior autonomously; external data sources
/// (order logs, calendar events, explicit user input) inject these records.
///
/// Per plico-multi-hop-reasoning.md §5.2: BehavioralObservation drives
/// pattern extraction, which may promote repeated patterns to UserFact KG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BehavioralObservation {
    /// Unique observation ID.
    pub id: String,
    /// When this behavior occurred (Unix ms).
    pub timestamp: u64,
    /// Who performed the behavior (e.g. person ID or "user").
    pub subject_id: String,
    /// Scene / situation of the behavior, e.g. "at_dinner", "when_drunk", "at_work".
    pub context: String,
    /// Category of action: "order_food", "preference_explicit", "consumption", etc.
    pub action_type: String,
    /// What was observed (free text), e.g. "ordered white congee", "said prefers red wine".
    pub outcome: String,
    /// If the user stated an explicit preference, store it here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explicit_preference: Option<String>,
}

impl BehavioralObservation {
    /// Create a new behavioral observation.
    pub fn new(
        subject_id: String,
        context: String,
        action_type: String,
        outcome: String,
        explicit_preference: Option<String>,
    ) -> Self {
        Self {
            id: format!("obs:{}", uuid::Uuid::new_v4()),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            subject_id,
            context,
            action_type,
            outcome,
            explicit_preference,
        }
    }
}

/// Time decay constant for preference confidence (per multi-hop-reasoning.md §6.3).
/// Half-life = 30 days in milliseconds.
const PREFERENCE_DECAY_HALF_LIFE_MS: u64 = 30 * 24 * 3600 * 1000;

/// A promoted user fact — a repeated behavioral pattern persisted as a KG node.
///
/// Created when a pattern is observed repeatedly (≥ MIN_PROMOTE_OBSERVATIONS times).
/// Stored in the knowledge graph as a KGNode with Fact type and this struct as metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserFact {
    pub id: String,
    /// Who this fact pertains to.
    pub subject_id: String,
    /// The inferred predicate: "prefers" | "dislikes" | "needs" | "allergic_to".
    pub predicate: String,
    /// The object of the preference, e.g. "wine", "white_congee", "spicy_food".
    pub object: String,
    /// Context where this pattern applies: "at_dinner", "when_drunk", "at_home".
    pub context: String,
    /// Confidence score [0, 1], accounting for frequency and recency decay.
    pub confidence: f32,
    /// Evidence: IDs of BehavioralObservations that support this fact.
    pub evidence_ids: Vec<String>,
    /// When this fact was last updated (Unix ms).
    pub updated_at: u64,
}

impl UserFact {
    /// Compute confidence from observation frequency with time-decay.
    ///
    /// confidence = min(1.0, count / MIN_PROMOTE_OBSERVATIONS) × decay(now − last_obs)
    /// where decay(x) = 0.5 ^ (x / HALF_LIFE_MS)
    ///
    /// Per plico-multi-hop-reasoning.md §4.2, §6.3.
    pub fn from_observations(
        subject_id: &str,
        predicate: &str,
        object: &str,
        context: &str,
        observations: &[BehavioralObservation],
    ) -> Option<Self> {
        const MIN_PROMOTE: usize = 3;
        if observations.len() < MIN_PROMOTE {
            return None;
        }
        let count = observations.len() as f32;
        let frequency_confidence = (count / MIN_PROMOTE as f32).min(1.0);

        // Time decay: most recent observation age
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let latest_ts = observations.iter().map(|o| o.timestamp).max().unwrap_or(0);
        let age_ms = now_ms.saturating_sub(latest_ts);
        let decay = 0.5_f32.powf(age_ms as f32 / PREFERENCE_DECAY_HALF_LIFE_MS as f32);

        let confidence = frequency_confidence * decay;
        let evidence_ids: Vec<String> = observations.iter().map(|o| o.id.clone()).collect();

        Some(Self {
            id: format!("fact:{}", uuid::Uuid::new_v4()),
            subject_id: subject_id.to_string(),
            predicate: predicate.to_string(),
            object: object.to_string(),
            context: context.to_string(),
            confidence,
            evidence_ids,
            updated_at: now_ms,
        })
    }
}

/// Groups behavioral observations by (subject, context, object) and extracts patterns.
pub struct PatternExtractor;

impl PatternExtractor {
    /// Extract `UserFact` instances from a list of behavioral observations.
    ///
    /// Groups observations by (subject_id, context, explicit_preference).
    /// Returns one UserFact per group if the group has ≥ 3 observations.
    ///
    /// Per plico-multi-hop-reasoning.md §4.2: frequency / MIN_OBSERVATIONS → confidence.
    pub fn extract(observations: &[BehavioralObservation]) -> Vec<UserFact> {
        use std::collections::HashMap;

        // Group by (subject_id, context, outcome) — outcome = inferred object
        let mut groups: HashMap<(String, String, String), Vec<&BehavioralObservation>> =
            HashMap::new();
        for obs in observations {
            let key = (obs.subject_id.clone(), obs.context.clone(), obs.outcome.clone());
            groups.entry(key).or_default().push(obs);
        }

        let mut facts = Vec::new();
        for ((subject_id, context, outcome), group) in groups {
            // Determine predicate: explicit preference → "prefers", otherwise infer
            let predicate = if group.iter().any(|o| o.explicit_preference.is_some()) {
                "prefers"
            } else {
                "prefers" // TODO: could infer "consumed" from action_type
            };

            let owned: Vec<BehavioralObservation> =
                group.iter().map(|o| (*o).clone()).collect();
            if let Some(fact) = UserFact::from_observations(
                &subject_id, predicate, &outcome, &context, &owned,
            ) {
                facts.push(fact);
            }
        }
        facts
    }
}

/// An AI-generated action suggestion inferred from a pattern.
///
/// Stored inline in the reasoning pipeline — not persisted as a KG node
/// unless the pattern repeats (cross-event UserFact promotion).
///
/// Per plico-multi-hop-reasoning.md §6.1: Preference is stored as an inline
/// field here, not as a separate KG node. Only patterns that repeat across
/// multiple events are promoted to persistent UserFact KG nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSuggestion {
    /// Unique suggestion ID.
    pub id: String,
    /// The event that triggered this suggestion.
    pub trigger_event_id: String,
    /// Person the suggestion targets.
    pub target_person_id: String,
    /// The suggested action text, e.g. "提醒带红酒".
    pub action: String,
    /// Step-by-step reasoning chain for explainability (Chain-of-Knowledge).
    /// Each entry is a natural-language step, e.g. "王总在8次商务晚餐中6次选择红酒".
    pub reasoning_chain: Vec<String>,
    /// Inline preference: person_id who has this preference.
    pub preference_person_id: String,
    /// What the person prefers (e.g. "wine", "white_congee").
    pub preference_object: String,
    /// Context where this preference applies (e.g. "at_dinner", "when_drunk").
    pub preference_context: String,
    /// Confidence score [0, 1], computed as frequency / MIN_OBSERVATIONS × time_decay.
    pub confidence: f32,
    /// Suggestion lifecycle status.
    pub status: SuggestionStatus,
}

impl ActionSuggestion {
    pub fn new(
        trigger_event_id: String,
        target_person_id: String,
        action: String,
        reasoning_chain: Vec<String>,
        preference_person_id: String,
        preference_object: String,
        preference_context: String,
        confidence: f32,
    ) -> Self {
        let id = format!("sug:{}", uuid::Uuid::new_v4());
        Self {
            id,
            trigger_event_id,
            target_person_id,
            action,
            reasoning_chain,
            preference_person_id,
            preference_object,
            preference_context,
            confidence,
            status: SuggestionStatus::Pending,
        }
    }

    /// Suggestion is actionable: high enough confidence to auto-fire.
    pub fn is_actionable(&self) -> bool {
        self.confidence >= PREFERENCE_HIGH_CONFIDENCE
    }

    /// Suggestion needs user confirmation: moderate confidence.
    pub fn needs_confirmation(&self) -> bool {
        self.confidence >= PREFERENCE_MIN_CONFIDENCE
            && self.confidence < PREFERENCE_HIGH_CONFIDENCE
    }

    /// Suggestion confidence is too low to surface.
    pub fn is_too_uncertain(&self) -> bool {
        self.confidence < PREFERENCE_MIN_CONFIDENCE
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

/// The semantic filesystem — a CAS-backed filesystem with AI-friendly operations.
pub struct SemanticFS {
    /// CAS storage backend.
    cas: Arc<CASStorage>,
    /// Tag index: tag → CIDs.
    tag_index: RwLock<HashMap<String, Vec<String>>>,
    /// Path to persist the tag index.
    tag_index_path: std::path::PathBuf,
    /// Path to persist the recycle bin index.
    recycle_bin_path: std::path::PathBuf,
    /// Context loader for L0/L1/L2 layers.
    ctx_loader: Arc<ContextLoader>,
    /// Recycle bin (logical deletes).
    recycle_bin: RwLock<HashMap<String, RecycleEntry>>,
    /// Update audit log.
    audit_log: RwLock<Vec<AuditEntry>>,
    /// Embedding provider (e.g. Ollama).
    embedding: Arc<dyn EmbeddingProvider>,
    /// Vector search index.
    search_index: Arc<dyn SemanticSearch>,
    /// LLM summarizer for L0/L1 context generation.
    summarizer: Option<Arc<dyn Summarizer>>,
    /// Knowledge graph for entity/relationship tracking.
    knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecycleEntry {
    pub cid: String,
    pub deleted_at: u64,
    pub original_meta: AIObjectMeta,
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

#[derive(Debug, thiserror::Error)]
pub enum FSError {
    #[error("Object not found: {0}")]
    NotFound(String),

    #[error("CAS error: {0}")]
    CAS(#[from] crate::cas::CASError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Embedding error: {0}")]
    Embedding(#[from] EmbedError),
}

impl SemanticFS {
    /// Create a new semantic filesystem.
    ///
    /// `embedding` — provider for text → vector embeddings (e.g. OllamaBackend).
    /// `search_index` — backend for vector similarity search (e.g. InMemoryBackend).
    /// `summarizer` — optional LLM summarizer for L0/L1 context (e.g. OllamaSummarizer).
    pub fn new(
        root_path: std::path::PathBuf,
        embedding: Arc<dyn EmbeddingProvider>,
        search_index: Arc<dyn SemanticSearch>,
        summarizer: Option<Arc<dyn Summarizer>>,
        knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    ) -> std::io::Result<Self> {
        let tag_index_path = root_path.join("tag_index.json");
        let recycle_bin_path = root_path.join("recycle_bin.json");
        let cas = Arc::new(CASStorage::new(root_path.join("objects"))?);

        // Rebuild in-memory tag index from existing CAS objects on startup
        let tag_index = if tag_index_path.exists() {
            Self::load_tag_index(&tag_index_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load tag index, rebuilding from CAS: {}", e);
                Self::rebuild_tag_index(&cas)
            })
        } else {
            Self::rebuild_tag_index(&cas)
        };

        // Load recycle bin from disk (if it exists)
        let recycle_bin = if recycle_bin_path.exists() {
            Self::load_recycle_bin(&recycle_bin_path).unwrap_or_else(|e| {
                tracing::warn!("Failed to load recycle bin: {}", e);
                HashMap::new()
            })
        } else {
            HashMap::new()
        };

        let fs = Self {
            cas: Arc::clone(&cas),
            tag_index: RwLock::new(tag_index),
            tag_index_path,
            recycle_bin_path,
            ctx_loader: Arc::new(ContextLoader::new(root_path.join("context"), summarizer.clone(), cas)?),
            recycle_bin: RwLock::new(recycle_bin),
            audit_log: RwLock::new(Vec::new()),
            embedding,
            search_index,
            summarizer,
            knowledge_graph,
        };

        // Rebuild vector index from persisted CAS objects.
        // The in-memory SemanticSearch index is lost on every restart; re-embed
        // all stored text objects so semantic search works after a cold start.
        fs.rebuild_vector_index();

        Ok(fs)
    }

    /// Rebuild the in-memory vector search index from all CAS objects.
    ///
    /// Called once at startup. Skipped (with a warning) if the embedding
    /// provider is unavailable — the tag-based fallback remains functional.
    fn rebuild_vector_index(&self) {
        let cids = match self.cas.list_cids() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("rebuild_vector_index: failed to list CIDs: {e}");
                return;
            }
        };

        if cids.is_empty() {
            return;
        }

        tracing::debug!("rebuild_vector_index: found {} CIDs", cids.len());

        tracing::info!("Rebuilding vector index for {} objects…", cids.len());
        let mut indexed = 0usize;

        for cid in &cids {
            let obj = match self.cas.get(cid) {
                Ok(o) => o,
                Err(_) => continue,
            };

            // Skip known binary blobs (images, audio, video).
            // Include Unknown — legacy objects stored without type detection.
            if obj.meta.content_type.is_multimedia() {
                continue;
            }

            let text = match std::str::from_utf8(&obj.data) {
                Ok(s) => s.trim().to_string(),
                Err(_) => continue,
            };

            if text.is_empty() {
                continue;
            }

            match self.embedding.embed(&text) {
                Ok(emb) => {
                    self.search_index.upsert(
                        cid,
                        &emb,
                        SearchIndexMeta {
                            cid: cid.clone(),
                            tags: obj.meta.tags.clone(),
                            content_type: obj.meta.content_type.to_string(),
                            snippet: text.chars().take(256).collect(),
                            created_at: obj.meta.created_at,
                        },
                    );
                    indexed += 1;
                }
                Err(e) => {
                    tracing::warn!("rebuild_vector_index: embed failed for {}: {e}", &cid[..8]);
                    // Stop trying — embedding provider unavailable; tag-based fallback remains.
                    break;
                }
            }
        }

        if indexed > 0 {
            tracing::info!("Vector index rebuilt: {}/{} objects indexed", indexed, cids.len());
        }
    }

    /// **Create**: Store content with semantic metadata. Returns CID.
    ///
    /// Side effects:
    /// 1. Content is stored in CAS
    /// 2. Tags are indexed
    /// 3. Text is embedded and upserted to the vector search index
    /// 4. Audit log entry is created
    pub fn create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        created_by: String,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        // Auto-detect content type: if the bytes are valid UTF-8, treat as text.
        let content_type = if std::str::from_utf8(&content).is_ok() {
            crate::cas::ContentType::Text
        } else {
            crate::cas::ContentType::Unknown
        };

        let meta = AIObjectMeta {
            content_type,
            tags: tags.clone(),
            created_by,
            created_at: now_ms(),
            intent,
        };

        let obj = AIObject::new(content.clone(), meta.clone());
        let cid = self.cas.put(&obj)?;

        // Update tag index
        self.update_tag_index(&tags, &cid);

        // Embed and index for semantic search
        self.upsert_semantic_index(&cid, &content, &meta);

        // Upsert to knowledge graph: creates Document node + AssociatesWith edges
        if let Some(ref kg) = self.knowledge_graph {
            if let Err(e) = kg.upsert_document(&cid, &tags, &meta.created_by) {
                tracing::warn!("Failed to upsert document to knowledge graph: {}", e);
            }
        }

        // Auto-generate L0 summary if a summarizer is configured.
        // Failure is non-fatal — L2 content is always available as fallback.
        if let Some(ref summarizer) = self.summarizer {
            let text = match std::str::from_utf8(&content) {
                Ok(s) if !s.trim().is_empty() => s.to_string(),
                _ => String::new(),
            };
            if !text.is_empty() {
                match summarizer.summarize(&text, crate::fs::summarizer::SummaryLayer::L0) {
                    Ok(summary) => {
                        if let Err(e) = self.ctx_loader.store_l0(&cid, summary) {
                            tracing::warn!("Failed to store L0 summary for {}: {}", &cid[..8], e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("L0 summarization failed for {}: {}", &cid[..8], e);
                    }
                }
            }
        }

        // Audit log
        self.audit_log
            .write()
            .unwrap()
            .push(AuditEntry {
                timestamp: now_ms(),
                action: AuditAction::Create,
                cid: cid.clone(),
                agent_id: String::new(),
            });

        Ok(cid)
    }

    /// **Read**: Retrieve object by CID or query. Optionally at specific context layer.
    pub fn read(&self, query: &Query) -> std::io::Result<Vec<AIObject>> {
        match query {
            Query::ByCid(cid) => {
                let obj = self.cas.get(cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
                Ok(vec![obj])
            }
            Query::ByTags(tags) => {
                let cids = self.resolve_tags(tags);
                let mut objects = Vec::new();
                for cid in cids {
                    if let Ok(obj) = self.cas.get(&cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::Semantic { text, filter } => {
                // Vector semantic search
                let filter = filter.clone().unwrap_or_default();
                let query_emb = match self.embedding.embed(text) {
                    Ok(emb) => emb,
                    Err(e) => {
                        tracing::warn!("Embedding failed for query '{text}': {e}. Falling back to tag search.");
                        // Fallback: tag-based keyword matching
                        let tags = text.split_whitespace().map(String::from).collect();
                        return self.read(&Query::ByTags(tags));
                    }
                };
                let hits = self.search_index.search(&query_emb, 10, &filter);
                let mut objects = Vec::new();
                for hit in hits {
                    if let Ok(obj) = self.cas.get(&hit.cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::ByType(content_type) => {
                // Scan the search index for all entries with the matching content_type.
                let filter = crate::fs::search::SearchFilter {
                    content_type: Some(content_type.clone()),
                    ..Default::default()
                };
                let cids = self.search_index.list_by_filter(&filter);
                let mut objects = Vec::new();
                for cid in cids {
                    if let Ok(obj) = self.cas.get(&cid) {
                        objects.push(obj);
                    }
                }
                Ok(objects)
            }
            Query::Hybrid { tags, semantic, content_type } => {
                // Build a filter from tags + content_type.
                let filter = crate::fs::search::SearchFilter {
                    require_tags: tags.clone(),
                    content_type: content_type.clone(),
                    ..Default::default()
                };

                if let Some(text) = semantic {
                    // Semantic vector search with tag + type filter applied.
                    let query_emb = match self.embedding.embed(text) {
                        Ok(emb) => emb,
                        Err(e) => {
                            tracing::warn!("Embedding failed in Hybrid query: {e}. Falling back to filter scan.");
                            let cids = self.search_index.list_by_filter(&filter);
                            let mut objects = Vec::new();
                            for cid in cids {
                                if let Ok(obj) = self.cas.get(&cid) {
                                    objects.push(obj);
                                }
                            }
                            return Ok(objects);
                        }
                    };
                    let hits = self.search_index.search(&query_emb, 10, &filter);
                    let mut objects = Vec::new();
                    for hit in hits {
                        if let Ok(obj) = self.cas.get(&hit.cid) {
                            objects.push(obj);
                        }
                    }
                    Ok(objects)
                } else {
                    // No semantic text — pure tag+type filter scan.
                    let cids = self.search_index.list_by_filter(&filter);
                    let mut objects = Vec::new();
                    for cid in cids {
                        if let Ok(obj) = self.cas.get(&cid) {
                            objects.push(obj);
                        }
                    }
                    Ok(objects)
                }
            }
        }
    }

    /// **Update**: Replace object content, preserving CID history for rollback.
    /// Returns the new CID (old CID is preserved in audit log).
    pub fn update(
        &self,
        old_cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: String,
    ) -> std::io::Result<String> {
        // Read old object
        let old_obj = self.cas.get(old_cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;

        // Decide on new tags: use new_tags if provided, otherwise keep old ones
        let final_tags = new_tags.unwrap_or_else(|| old_obj.meta.tags.clone());

        let new_meta = AIObjectMeta {
            content_type: old_obj.meta.content_type,
            tags: final_tags.clone(),
            created_by: old_obj.meta.created_by.clone(),
            created_at: now_ms(),
            intent: old_obj.meta.intent.clone(),
        };

        let new_obj = AIObject::new(new_content.clone(), new_meta.clone());
        let new_cid = self.cas.put(&new_obj)?;

        // Update tag index: old CID is gone regardless of whether tags changed,
        // because the content hash changed and index keys on (tag, cid) pairs.
        self.remove_from_tag_index(&old_obj.meta.tags, old_cid);
        self.update_tag_index(&final_tags, &new_cid);

        // Update search index: remove old, add new
        self.search_index.delete(old_cid);
        self.upsert_semantic_index(&new_cid, &new_content, &new_meta);

        // Audit log
        self.audit_log
            .write()
            .unwrap()
            .push(AuditEntry {
                timestamp: now_ms(),
                action: AuditAction::Update {
                    previous_cid: old_cid.to_string(),
                },
                cid: new_cid.clone(),
                agent_id,
            });

        Ok(new_cid)
    }

    /// **Delete**: Logical delete — move to recycle bin (no physical deletion).
    pub fn delete(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
        if let Ok(obj) = self.cas.get(cid) {
            self.recycle_bin
                .write()
                .unwrap()
                .insert(cid.to_string(), RecycleEntry {
                    cid: cid.to_string(),
                    deleted_at: now_ms(),
                    original_meta: obj.meta.clone(),
                });

            // Remove from search index
            self.search_index.delete(cid);

            // Remove from knowledge graph
            if let Some(ref kg) = self.knowledge_graph {
                let _ = kg.remove_node(cid);
            }

            // Remove from tag index
            self.remove_from_tag_index(&obj.meta.tags, cid);

            self.audit_log
                .write()
                .unwrap()
                .push(AuditEntry {
                    timestamp: now_ms(),
                    action: AuditAction::Delete,
                    cid: cid.to_string(),
                    agent_id,
                });

            // Persist recycle bin to disk so deleted entries survive restart
            let _ = self.persist_recycle_bin();
        }
        Ok(())
    }

    /// **List deleted**: Returns all entries in the recycle bin.
    pub fn list_deleted(&self) -> Vec<RecycleEntry> {
        let bin = self.recycle_bin.read().unwrap();
        let mut entries: Vec<_> = bin.values().cloned().collect();
        // Stable ordering: most recently deleted first
        entries.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
        entries
    }

    /// **Restore**: Move an entry from recycle bin back to the active tag index
    /// and search index. The object data is still in CAS — only the index entries
    /// are restored. Returns FSError::NotFound if the CID is not in the recycle bin.
    pub fn restore(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
        let entry = {
            let mut bin = self.recycle_bin.write().unwrap();
            bin.remove(cid).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID not in recycle bin: {cid}"))
            })?
        };

        // Re-add to tag index
        self.update_tag_index(&entry.original_meta.tags, cid);

        // Re-add to search index (re-embed content from CAS)
        if let Ok(obj) = self.cas.get(cid) {
            self.upsert_semantic_index(cid, &obj.data, &obj.meta);
        }

        // Re-add to knowledge graph
        if let Some(ref kg) = self.knowledge_graph {
            let _ = kg.upsert_document(cid, &entry.original_meta.tags, &entry.original_meta.created_by);
        }

        // Persist updated (smaller) recycle bin
        let _ = self.persist_recycle_bin();

        self.audit_log.write().unwrap().push(AuditEntry {
            timestamp: now_ms(),
            action: AuditAction::Create, // restore is semantically a re-create
            cid: cid.to_string(),
            agent_id,
        });

        Ok(())
    }

    /// **Search**: Semantic search across all stored objects.
    /// Uses vector embeddings for semantic similarity.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        self.search_with_filter(query, limit, SearchFilter::default())
    }

    /// Semantic search with optional tag/content-type filtering.
    pub fn search_with_filter(&self, query: &str, limit: usize, filter: SearchFilter) -> Vec<SearchResult> {
        let query_emb = match self.embedding.embed(query) {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!("Embedding failed for query '{query}': {e}. Falling back to tag search.");
                return self.search_by_tags_with_filter(query, &filter);
            }
        };

        let hits = self.search_index.search(&query_emb, limit, &filter);
        hits
            .into_iter()
            .filter_map(|hit| {
                self.cas.get(&hit.cid).ok().map(|obj| SearchResult {
                    cid: hit.cid,
                    relevance: hit.score,
                    meta: obj.meta,
                })
            })
            .collect()
    }

    /// Tag-based keyword search with filter applied (fallback when embeddings unavailable).
    fn search_by_tags_with_filter(&self, query: &str, filter: &SearchFilter) -> Vec<SearchResult> {
        let query_lower = query.to_lowercase();
        let index = self.tag_index.read().unwrap();
        let mut results = Vec::new();

        for (tag, cids) in index.iter() {
            if tag.to_lowercase().contains(&query_lower) {
                for cid in cids {
                    if let Ok(obj) = self.cas.get(cid) {
                        if filter.matches(&SearchIndexMeta {
                            cid: cid.clone(),
                            tags: obj.meta.tags.clone(),
                            snippet: String::new(),
                            content_type: format!("{}", obj.meta.content_type),
                            created_at: obj.meta.created_at,
                        }) {
                            results.push(SearchResult {
                                cid: cid.clone(),
                                relevance: 0.8,
                                meta: obj.meta,
                            });
                        }
                    }
                }
            }
        }
        results
    }

    /// List all tags in the filesystem.
    pub fn list_tags(&self) -> Vec<String> {
        let index = self.tag_index.read().unwrap();
        let mut tags: Vec<_> = index.keys().cloned().collect();
        tags.sort();
        tags
    }

    /// Get audit log.
    pub fn audit_log(&self) -> Vec<AuditEntry> {
        self.audit_log.read().unwrap().clone()
    }

    // ─── Internal helpers ────────────────────────────────────────────────

    fn upsert_semantic_index(&self, cid: &str, content: &[u8], meta: &AIObjectMeta) {
        let text = String::from_utf8_lossy(content);

        // Build snippet (first 200 chars of UTF-8 text; empty for binary).
        let snippet = if text.trim().is_empty() {
            String::new()
        } else if text.len() > 200 {
            format!("{}...", &text[..200])
        } else {
            text.to_string()
        };

        // Attempt to embed for semantic search. On failure, use a zero vector so
        // that filter-based queries (ByType, Hybrid tags) still work — only
        // cosine similarity ranking is disabled.
        let embedding = if text.trim().is_empty() {
            vec![0.0f32; self.embedding.dimension()]
        } else {
            match self.embedding.embed(&text) {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!("Failed to embed CID={}: {e}. Indexing with zero vector.", cid);
                    vec![0.0f32; self.embedding.dimension()]
                }
            }
        };

        self.search_index.upsert(cid, &embedding, SearchIndexMeta {
            cid: cid.to_string(),
            tags: meta.tags.clone(),
            snippet,
            content_type: format!("{:?}", meta.content_type).to_lowercase(),
            created_at: meta.created_at,
        });
    }

    fn update_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags {
            index.entry(tag.clone()).or_default().push(cid.to_string());
        }
        drop(index);
        let _ = self.persist_tag_index();
    }

    /// Persist recycle bin to disk.
    fn persist_recycle_bin(&self) -> std::io::Result<()> {
        let bin = self.recycle_bin.read().unwrap();
        let json = serde_json::to_vec(&*bin)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&self.recycle_bin_path, json)
    }

    /// Load recycle bin from disk.
    fn load_recycle_bin(path: &std::path::Path) -> std::io::Result<HashMap<String, RecycleEntry>> {
        let json = std::fs::read(path)?;
        serde_json::from_slice::<HashMap<String, RecycleEntry>>(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// Persist tag index to disk.
    fn persist_tag_index(&self) -> std::io::Result<()> {
        let index = self.tag_index.read().unwrap();
        let json = serde_json::to_vec(&*index)?;
        std::fs::write(&self.tag_index_path, json)
    }

    /// Load tag index from disk.
    fn load_tag_index(path: &std::path::Path) -> std::io::Result<HashMap<String, Vec<String>>> {
        let json = std::fs::read(path)?;
        let index = serde_json::from_slice::<HashMap<String, Vec<String>>>(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(index)
    }

    /// Rebuild tag index by scanning all CAS objects.
    fn rebuild_tag_index(cas: &CASStorage) -> HashMap<String, Vec<String>> {
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        if let Ok(cids) = cas.list_cids() {
            for cid in cids {
                if let Ok(obj) = cas.get(&cid) {
                    for tag in &obj.meta.tags {
                        index.entry(tag.clone()).or_default().push(cid.clone());
                    }
                }
            }
        }
        index
    }


    fn remove_from_tag_index(&self, tags: &[String], cid: &str) {
        let mut index = self.tag_index.write().unwrap();
        for tag in tags {
            if let Some(cids) = index.get_mut(tag) {
                cids.retain(|c| c != cid);
            }
        }
        drop(index);
        let _ = self.persist_tag_index();
    }

    fn resolve_tags(&self, tags: &[String]) -> Vec<String> {
        let index = self.tag_index.read().unwrap();
        let mut cids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for tag in tags {
            if let Some(tag_cids) = index.get(tag) {
                for cid in tag_cids {
                    if seen.insert(cid.clone()) {
                        cids.push(cid.clone());
                    }
                }
            }
        }

        cids
    }

    // ── Event operations ─────────────────────────────────────────────────────

    /// Create an event container — stored as a KG node with EventMeta.
    ///
    /// The node is created with `node_type = Entity` and `EventMeta` serialized
    /// into the `properties` JSON field. Returns the node ID.
    pub fn create_event(
        &self,
        label: &str,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<&str>,
        tags: Vec<String>,
        agent_id: &str,
    ) -> Result<String, FSError> {
        let node_id = format!("evt:{}", uuid::Uuid::new_v4().to_string());

        // Store as KG node if knowledge_graph is available
        if let Some(ref kg) = self.knowledge_graph {
            let meta = EventMeta {
                label: label.to_string(),
                event_type,
                start_time,
                end_time,
                location: location.map(String::from),
                attendee_ids: Vec::new(),
                related_cids: Vec::new(),
            };
            let node = KGNode {
                id: node_id.clone(),
                label: label.to_string(),
                node_type: KGNodeType::Entity,
                content_cid: None,
                properties: serde_json::to_value(&meta)
                    .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?,
                agent_id: agent_id.to_string(),
                created_at: now_ms(),
            };
            kg.add_node(node)
                .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;
        }

        // Index event by tags so it can be found via list_events
        {
            let mut tag_index = self.tag_index.write().unwrap();
            for tag in &tags {
                tag_index.entry(tag.clone()).or_default().push(node_id.clone());
            }
            drop(tag_index);
            self.persist_tag_index()
                .map_err(|e| FSError::Io(e))?;
        }

        Ok(node_id)
    }

    /// List events matching the given filters.
    ///
    /// Time filtering: events are included if their `start_time` falls within [since, until].
    /// If `tags` is non-empty, only events indexed under all those tags are candidates.
    /// If `event_type` is provided, only events of that type are returned.
    /// Returns an empty vec if `knowledge_graph` is not initialized.
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> Result<Vec<EventSummary>, FSError> {
        let kg = match self.knowledge_graph.as_ref() {
            Some(g) => g,
            None => return Ok(Vec::new()),
        };

        // Build candidate set from tag index (or all KG nodes if no tags)
        let candidates: Vec<String> = if tags.is_empty() {
            kg.all_node_ids()
        } else {
            let tag_index = self.tag_index.read().unwrap();
            let mut intersection: Option<std::collections::HashSet<String>> = None;
            for tag in tags {
                if let Some(ids) = tag_index.get(tag) {
                    let set: std::collections::HashSet<String> = ids.iter().cloned().collect();
                    match intersection.take() {
                        Some(existing) => intersection = Some(existing.intersection(&set).cloned().collect()),
                        None => intersection = Some(set),
                    }
                }
            }
            intersection.unwrap_or_default().into_iter().collect()
        };

        let mut results = Vec::new();
        for node_id in candidates {
            let node = match kg.get_node(&node_id) {
                Ok(Some(n)) => n,
                _ => continue,
            };
            // Only consider entity-type nodes that have valid EventMeta
            if node.node_type != KGNodeType::Entity { continue; }
            let meta: EventMeta = match serde_json::from_value(node.properties.clone()) {
                Ok(m) => m,
                Err(_) => continue, // Not an event node
            };
            if !meta.in_range(since, until) { continue; }
            if let Some(et) = event_type {
                if meta.event_type != et { continue; }
            }
            results.push(EventSummary {
                id: node.id,
                label: meta.label,
                event_type: meta.event_type,
                start_time: meta.start_time,
                attendee_count: meta.attendee_ids.len(),
                related_count: meta.related_cids.len(),
            });
        }

        results.sort_by_key(|e| e.start_time);
        Ok(results)
    }

    /// Resolve a natural-language time expression and list matching events.
    ///
    /// This is a convenience wrapper: it calls `resolver.resolve(expr, reference_ms)` to get
    /// a `TemporalRange`, then delegates to `list_events(range.since, range.until, tags, event_type)`.
    ///
    /// Returns `Err` if the expression cannot be resolved. Use this when the caller
    /// already has a `TemporalResolver` (e.g. `RULE_BASED_RESOLVER` or `OllamaTemporalResolver`).
    ///
    /// # Example
    ///
    /// ```ignore
    /// use plico::temporal::RULE_BASED_RESOLVER;
    /// let events = fs.list_events_by_time("上周", &[], None, &RULE_BASED_RESOLVER)?;
    /// ```
    pub fn list_events_by_time(
        &self,
        time_expression: &str,
        tags: &[String],
        event_type: Option<EventType>,
        resolver: &dyn TemporalResolver,
    ) -> Result<Vec<EventSummary>, FSError> {
        let range = resolver.resolve(time_expression, None)
            .ok_or_else(|| {
                FSError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("Cannot resolve time expression: {time_expression}"),
                ))
            })?;
        // TemporalRange uses i64 (signed); EventMeta.start_time uses u64 (unsigned).
        // Unix timestamps are always non-negative, so cast is safe.
        let since = if range.since >= 0 { Some(range.since as u64) } else { None };
        let until = Some(range.until as u64);
        self.list_events(since, until, tags, event_type)
    }

    /// Attach a target (Person, Document, Media, etc.) to an event via a typed edge.
    ///
    /// Updates both the KG edge and the EventMeta.attendee_ids / related_cids field.
    /// Returns `FSError::Io(NotFound)` if the KG is not available or event not found.
    pub fn event_attach(
        &self,
        event_id: &str,
        target_id: &str,
        relation: EventRelation,
        _agent_id: &str,
    ) -> Result<(), FSError> {
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not initialized")))?;

        // Add the KG edge
        let edge = KGEdge {
            src: event_id.to_string(),
            dst: target_id.to_string(),
            edge_type: relation.edge_type(),
            weight: 1.0,
            evidence_cid: None,
            created_at: now_ms(),
        };
        kg.add_edge(edge)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        // Update EventMeta on the KG node
        let mut node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;
        let mut meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        match relation {
            EventRelation::Attendee => {
                if !meta.attendee_ids.contains(&target_id.to_string()) {
                    meta.attendee_ids.push(target_id.to_string());
                }
            }
            EventRelation::Document | EventRelation::Media | EventRelation::Decision => {
                if !meta.related_cids.contains(&target_id.to_string()) {
                    meta.related_cids.push(target_id.to_string());
                }
            }
        }

        node.properties = serde_json::to_value(&meta)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        // Replace the node in the KG (HashMap::insert upserts)
        kg.add_node(node)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        Ok(())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::embedding::StubEmbeddingProvider;
    use crate::fs::graph::PetgraphBackend;
    use crate::fs::search::InMemoryBackend;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn make_fs(dir: &TempDir) -> SemanticFS {
        SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            None,
        )
        .unwrap()
    }

    fn make_fs_with_kg(dir: &TempDir) -> SemanticFS {
        SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,                                          // summarizer
            Some(Arc::new(PetgraphBackend::new())),        // knowledge_graph
        )
        .unwrap()
    }

    // ── EventMeta tests ────────────────────────────────────────────────────────

    #[test]
    fn event_meta_in_range_filters_correctly() {
        let meta = |start, end| EventMeta {
            label: "test".into(),
            event_type: EventType::Meeting,
            start_time: start,
            end_time: end,
            location: None,
            attendee_ids: vec![],
            related_cids: vec![],
        };

        // No bounds → always in range
        assert!(meta(Some(1000), Some(2000)).in_range(None, None));

        // Within range
        assert!(meta(Some(1000), Some(2000)).in_range(Some(500), Some(1500)));
        // Start before range
        assert!(!meta(Some(100), Some(500)).in_range(Some(500), Some(1500)));
        // Start after range
        assert!(!meta(Some(2000), Some(3000)).in_range(Some(500), Some(1500)));
        // Open lower bound
        assert!(meta(Some(1000), Some(2000)).in_range(None, Some(1500)));
        // Open upper bound
        assert!(meta(Some(1000), Some(2000)).in_range(Some(500), None));
        // None start_time treated as 0
        assert!(meta(None, None).in_range(Some(0), Some(100)));
    }

    #[test]
    fn event_type_serialize_roundtrip() {
        use serde_json;
        for et in [
            EventType::Meeting,
            EventType::Presentation,
            EventType::Travel,
            EventType::Social,
            EventType::Work,
        ] {
            let json = serde_json::to_string(&et).unwrap();
            let back: EventType = serde_json::from_str(&json).unwrap();
            assert_eq!(et, back);
        }
    }

    // ── SemanticFS event tests ─────────────────────────────────────────────────

    #[test]
    fn create_event_without_kg_returns_id() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir); // No KG
        let id = fs.create_event(
            "Q2规划会议",
            EventType::Meeting,
            Some(1000),
            None,
            Some("国贸大厦"),
            vec!["规划".to_string(), "Q2".to_string()],
            "agent1",
        ).unwrap();
        assert!(id.starts_with("evt:"));
    }

    #[test]
    fn create_and_list_event_with_kg() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let _id = fs.create_event(
            "Q2规划会议",
            EventType::Meeting,
            Some(1_000_000),
            None,
            Some("国贸大厦"),
            vec!["规划".to_string(), "Q2".to_string()],
            "agent1",
        ).unwrap();

        let events = fs.list_events(None, None, &[], None).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].label, "Q2规划会议");
        assert_eq!(events[0].event_type, EventType::Meeting);
        assert_eq!(events[0].attendee_count, 0);
    }

    #[test]
    fn list_events_by_time_range() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        // Event in March 2026 (timestamps in ms)
        let march = 1_742_781_600_000u64; // 2026-03-01 approx
        let apr = 1_745_875_200_000u64;   // 2026-04-01 approx

        fs.create_event("三月会议", EventType::Meeting, Some(march), None, None, vec![], "a").unwrap();
        fs.create_event("四月会议", EventType::Meeting, Some(apr), None, None, vec![], "a").unwrap();

        // Query only March (before April)
        let march_events = fs.list_events(Some(march), Some(apr - 1), &[], None).unwrap();
        assert_eq!(march_events.len(), 1);
        assert_eq!(march_events[0].label, "三月会议");

        // Query April onward
        let apr_events = fs.list_events(Some(apr), None, &[], None).unwrap();
        assert_eq!(apr_events.len(), 1);
        assert_eq!(apr_events[0].label, "四月会议");
    }

    #[test]
    fn list_events_by_tag_intersection() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        fs.create_event("会议A", EventType::Meeting, None, None, None, vec!["规划".to_string()], "a").unwrap();
        fs.create_event("会议B", EventType::Meeting, None, None, None, vec!["Q2".to_string()], "a").unwrap();
        fs.create_event("会议C", EventType::Meeting, None, None, None, vec!["规划".to_string(), "Q2".to_string()], "a").unwrap();

        // Both tags must match
        let both = fs.list_events(None, None, &["规划".to_string(), "Q2".to_string()], None).unwrap();
        assert_eq!(both.len(), 1);
        assert_eq!(both[0].label, "会议C");
    }

    #[test]
    fn event_attach_updates_meta_and_edge() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let event_id = fs.create_event("Q2规划", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        let person_id = "person:wang"; // Simulated person node

        // Attach attendee
        fs.event_attach(&event_id, person_id, EventRelation::Attendee, "a").unwrap();

        let events = fs.list_events(None, None, &[], None).unwrap();
        assert_eq!(events[0].attendee_count, 1);

        // Attach document
        let doc_cid = "QmAABBCC";
        fs.event_attach(&event_id, doc_cid, EventRelation::Document, "a").unwrap();

        let events = fs.list_events(None, None, &[], None).unwrap();
        assert_eq!(events[0].related_count, 1);
    }

    #[test]
    fn list_events_returns_empty_without_kg() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir); // No KG
        fs.create_event("test", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        // list_events should return empty vec when KG is None
        let events = fs.list_events(None, None, &[], None).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn event_attach_fails_without_kg() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir); // No KG
        let id = fs.create_event("test", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        let result = fs.event_attach(&id, "target", EventRelation::Attendee, "a");
        assert!(result.is_err());
    }

    #[test]
    fn list_events_by_time_resolves_expression() {
        use crate::temporal::RULE_BASED_RESOLVER;
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);
        let resolver: &dyn crate::temporal::TemporalResolver = &RULE_BASED_RESOLVER;
        // Use Local time (same as HeuristicTemporalResolver) to avoid timezone mismatch.
        // resolve("几天前") → (now-7days, now) in local time.
        let three_days_ago = (chrono::Local::now() - chrono::Duration::days(3))
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_millis() as u64;
        fs.create_event(
            "三天前的会议",
            EventType::Meeting,
            Some(three_days_ago),
            None,
            None,
            vec!["工作".to_string()],
            "a",
        )
        .unwrap();
        let events = fs
            .list_events_by_time("几天前", &["工作".to_string()], None, resolver)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].label, "三天前的会议");
    }

    #[test]
    fn list_events_by_time_unknown_expression_returns_error() {
        use crate::temporal::StubTemporalResolver;
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let resolver = StubTemporalResolver;
        let result = fs.list_events_by_time("当我还是个孩子的时候", &[], None, &resolver);
        assert!(result.is_err());
    }

    #[test]
    fn context_loader_l2_returns_actual_content() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let expected = b"The quick brown fox";
        let cid = fs
            .create(expected.to_vec(), vec!["test".to_string()], "agent".to_string(), None)
            .unwrap();

        // Load via context loader
        let ctx = fs.ctx_loader.load(&cid, crate::fs::context_loader::ContextLayer::L2).unwrap();
        assert_eq!(ctx.layer, crate::fs::context_loader::ContextLayer::L2);
        assert_eq!(ctx.content.as_bytes(), expected);
        assert!(ctx.tokens_estimate > 0);
    }

    #[test]
    fn by_type_returns_matching_objects() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        // Create a text object and a binary object
        let cid_text = fs
            .create(b"hello text".to_vec(), vec!["doc".to_string()], "a".to_string(), None)
            .unwrap();
        let cid_bin = fs
            .create(vec![0x89, 0x50, 0x4E, 0x47], vec!["img".to_string()], "a".to_string(), None)
            .unwrap();

        // Query by type "text"
        let results = fs.read(&Query::ByType("text".to_string())).unwrap();
        let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
        assert!(cids.contains(&cid_text.as_str()), "text object must appear in ByType(text)");
        // Binary (PNG magic bytes) should not appear as text
        assert!(!cids.contains(&cid_bin.as_str()), "binary object must not appear in ByType(text)");
    }

    #[test]
    fn hybrid_query_with_tags_filters_correctly() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let cid_a = fs
            .create(b"Rust programming notes".to_vec(), vec!["rust".to_string(), "notes".to_string()], "a".to_string(), None)
            .unwrap();
        let _cid_b = fs
            .create(b"Python tutorial".to_vec(), vec!["python".to_string(), "notes".to_string()], "a".to_string(), None)
            .unwrap();

        // Hybrid with only tags — should return only rust-tagged object
        let results = fs
            .read(&Query::Hybrid {
                tags: vec!["rust".to_string()],
                semantic: None,
                content_type: None,
            })
            .unwrap();

        let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
        assert!(cids.contains(&cid_a.as_str()), "rust-tagged object must appear");
        assert_eq!(cids.len(), 1, "only rust-tagged object expected");
    }

    /// Regression test: after update() with unchanged tags, the NEW cid must be
    /// reachable via ByTags and the OLD cid must not appear.
    #[test]
    fn update_tag_index_reflects_new_cid() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let cid1 = fs
            .create(
                b"version one".to_vec(),
                vec!["rust".to_string(), "plico".to_string()],
                "agent-test".to_string(),
                None,
            )
            .unwrap();

        // Update content only (tags unchanged — this was the bug trigger)
        let cid2 = fs
            .update(&cid1, b"version two".to_vec(), None, "agent-test".to_string())
            .unwrap();

        // The two versions must have different CIDs (different content).
        assert_ne!(cid1, cid2, "updated content must produce a new CID");

        // ByTags must return the NEW cid, not the old one.
        let results = fs.read(&Query::ByTags(vec!["rust".to_string()])).unwrap();
        let cids: Vec<_> = results.iter().map(|r| r.cid.as_str()).collect();

        assert!(
            cids.contains(&cid2.as_str()),
            "new CID must be in tag index after update; got {:?}",
            cids
        );
        assert!(
            !cids.contains(&cid1.as_str()),
            "old CID must be removed from tag index after update; got {:?}",
            cids
        );
    }

    #[test]
    fn action_suggestion_is_actionable_threshold() {
        let sug = ActionSuggestion::new(
            "evt:1".to_string(),
            "person:wang".to_string(),
            "提醒带红酒".to_string(),
            vec!["王总偏好红酒".to_string()],
            "person:wang".to_string(),
            "wine".to_string(),
            "at_dinner".to_string(),
            0.85,
        );
        assert!(sug.is_actionable());
        assert!(!sug.needs_confirmation());
        assert!(!sug.is_too_uncertain());
    }

    #[test]
    fn action_suggestion_needs_confirmation_mid_range() {
        let sug = ActionSuggestion::new(
            "evt:2".to_string(),
            "person:li".to_string(),
            "准备白粥".to_string(),
            vec!["醉酒后偏好白粥".to_string()],
            "person:li".to_string(),
            "white_congee".to_string(),
            "when_drunk".to_string(),
            0.5,
        );
        assert!(!sug.is_actionable());
        assert!(sug.needs_confirmation());
        assert!(!sug.is_too_uncertain());
    }

    #[test]
    fn action_suggestion_too_uncertain_below_min() {
        let sug = ActionSuggestion::new(
            "evt:3".to_string(),
            "person:zhang".to_string(),
            "准备威士忌".to_string(),
            vec!["仅观察到1次".to_string()],
            "person:zhang".to_string(),
            "whisky".to_string(),
            "at_dinner".to_string(),
            0.2,
        );
        assert!(!sug.is_actionable());
        assert!(!sug.needs_confirmation());
        assert!(sug.is_too_uncertain());
    }

    #[test]
    fn suggestion_status_serialize_roundtrip() {
        use serde_json;
        for status in &[SuggestionStatus::Pending, SuggestionStatus::Confirmed, SuggestionStatus::Dismissed] {
            let json = serde_json::to_string(status).unwrap();
            let decoded: SuggestionStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(*status, decoded);
        }
    }

    #[test]
    fn action_suggestion_serialize_roundtrip() {
        use serde_json;
        let sug = ActionSuggestion::new(
            "evt:x".to_string(),
            "person:y".to_string(),
            "带酒".to_string(),
            vec!["王总偏好红酒(6/8次)".to_string()],
            "person:wang".to_string(),
            "wine".to_string(),
            "at_dinner".to_string(),
            0.75,
        );
        let json = serde_json::to_string(&sug).unwrap();
        let decoded: ActionSuggestion = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, sug.id);
        assert_eq!(decoded.action, sug.action);
        assert_eq!(decoded.confidence, sug.confidence);
        assert_eq!(decoded.reasoning_chain.len(), 1);
    }

    #[test]
    fn pattern_extractor_requires_min_observations() {
        // 2 observations → below threshold → no UserFact
        let obs = vec![
            BehavioralObservation::new(
                "person:wang".to_string(),
                "at_dinner".to_string(),
                "order_food".to_string(),
                "红酒".to_string(),
                None,
            ),
            BehavioralObservation::new(
                "person:wang".to_string(),
                "at_dinner".to_string(),
                "order_food".to_string(),
                "红酒".to_string(),
                None,
            ),
        ];
        let facts = PatternExtractor::extract(&obs);
        assert!(facts.is_empty(), "should need ≥ 3 observations to promote");
    }

    #[test]
    fn pattern_extractor_promotes_repeated_pattern() {
        // 3 identical observations → should create one UserFact
        let obs: Vec<BehavioralObservation> = (0..3)
            .map(|_| {
                BehavioralObservation::new(
                    "person:wang".to_string(),
                    "at_dinner".to_string(),
                    "order_food".to_string(),
                    "红酒".to_string(),
                    None,
                )
            })
            .collect();
        let facts = PatternExtractor::extract(&obs);
        assert_eq!(facts.len(), 1, "3 identical observations → 1 UserFact");
        let fact = &facts[0];
        assert_eq!(fact.subject_id, "person:wang");
        assert_eq!(fact.object, "红酒");
        assert_eq!(fact.context, "at_dinner");
        assert!(fact.confidence >= 0.5, "3 obs / 3 min = 1.0 before decay");
        assert!(fact.confidence <= 1.0);
        assert_eq!(fact.evidence_ids.len(), 3);
    }

    #[test]
    fn pattern_extractor_groups_by_context_separately() {
        // Wang + dinner + wine  (3x) vs Wang + drunk + congee (3x) → 2 separate facts
        let mut obs = Vec::new();
        for _ in 0..3 {
            obs.push(BehavioralObservation::new(
                "person:wang".to_string(),
                "at_dinner".to_string(),
                "order_food".to_string(),
                "红酒".to_string(),
                None,
            ));
            obs.push(BehavioralObservation::new(
                "person:wang".to_string(),
                "when_drunk".to_string(),
                "order_food".to_string(),
                "白粥".to_string(),
                None,
            ));
        }
        let facts = PatternExtractor::extract(&obs);
        assert_eq!(facts.len(), 2, "two distinct contexts → two UserFacts");
    }

    #[test]
    fn user_fact_from_observations_rejects_low_count() {
        let obs = vec![BehavioralObservation::new(
            "person:li".to_string(),
            "at_dinner".to_string(),
            "order_food".to_string(),
            "威士忌".to_string(),
            None,
        )];
        let fact = UserFact::from_observations(
            "person:li", "prefers", "威士忌", "at_dinner", &obs,
        );
        assert!(fact.is_none(), "single observation should not promote");
    }

    #[test]
    fn behavioral_observation_has_timestamp() {
        let obs = BehavioralObservation::new(
            "user".to_string(),
            "at_home".to_string(),
            "consumption".to_string(),
            "牛奶".to_string(),
            None,
        );
        assert!(obs.timestamp > 0, "timestamp should be set to current time");
        assert!(!obs.id.is_empty());
    }
}
