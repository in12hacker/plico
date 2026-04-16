//! Semantic Filesystem Implementation
//!
//! Provides AI-friendly CRUD operations. No paths — only semantic descriptions.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::{EmbeddingProvider, EmbedError};
use crate::fs::search::{SemanticSearch, SearchFilter, SearchIndexMeta, Bm25Index};
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
    /// IDs of behavioral observations associated with this event.
    pub observation_ids: Vec<String>,
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

    /// Convert extracted `UserFact` patterns into `ActionSuggestion` instances.
    ///
    /// Only UserFacts with confidence ≥ `PREFERENCE_MIN_CONFIDENCE` are promoted
    /// to suggestions. Each suggestion includes a reasoning chain for explainability.
    ///
    /// Per plico-multi-hop-reasoning.md §4.3, §6.1: Preference is stored inline
    /// in ActionSuggestion, not as a separate KG node.
    ///
    /// # Arguments
    /// * `facts` — UserFacts extracted from behavioral observations
    /// * `event_id` — The event that triggered this extraction (used as trigger_event_id)
    /// * `event_label` — Human-readable event label for reasoning chain
    pub fn extract_and_suggest(
        facts: &[UserFact],
        event_id: &str,
        event_label: &str,
    ) -> Vec<ActionSuggestion> {
        let mut suggestions = Vec::new();

        for fact in facts {
            // Only surface suggestions that meet minimum confidence threshold
            if fact.confidence < PREFERENCE_MIN_CONFIDENCE {
                continue;
            }

            let action = Self::action_for_fact(fact);
            let reasoning_chain = Self::build_reasoning_chain(fact, event_label);

            suggestions.push(ActionSuggestion::new(
                event_id.to_string(),
                fact.subject_id.clone(),
                action,
                reasoning_chain,
                fact.subject_id.clone(),
                fact.object.clone(),
                fact.context.clone(),
                fact.confidence,
            ));
        }

        suggestions
    }

    /// Map a UserFact to a human-readable action string.
    ///
    /// Per plico-multi-hop-reasoning.md §4.3 `action_for_preference()`.
    fn action_for_fact(fact: &UserFact) -> String {
        match (fact.predicate.as_str(), fact.object.as_str()) {
            ("prefers", "wine") => "提醒带红酒".to_string(),
            ("prefers", "white_congee") => "准备白粥".to_string(),
            ("prefers", "beer") => "准备啤酒".to_string(),
            ("prefers", "whisky") | ("prefers", "whiskey") => "准备威士忌".to_string(),
            ("dislikes", obj) => format!("避免准备{}", obj),
            ("allergic_to", obj) => format!("绝对不要提供{}", obj),
            ("needs", obj) => format!("准备{}", obj),
            _ => format!("考虑准备{}", fact.object),
        }
    }

    /// Build a Chain-of-Knowledge reasoning chain for a UserFact.
    ///
    /// Per plico-multi-hop-reasoning.md §3.2: each step links to evidence.
    fn build_reasoning_chain(fact: &UserFact, event_label: &str) -> Vec<String> {
        let count = fact.evidence_ids.len();
        let context = &fact.context;
        let obj = &fact.object;
        let subject = &fact.subject_id;

        vec![
            format!(
                "{}在{}场景下共出现{}次偏好{}",
                subject, count, context, obj
            ),
            format!(
                "根据历史行为模式，推断{}偏好{}",
                subject, obj
            ),
            format!(
                "建议在{}时采取行动：{}",
                event_label,
                Self::action_for_fact(fact)
            ),
        ]
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

    /// Mark this suggestion as confirmed by the user.
    pub fn confirm(&mut self) {
        self.status = SuggestionStatus::Confirmed;
    }

    /// Mark this suggestion as dismissed by the user.
    pub fn dismiss(&mut self) {
        self.status = SuggestionStatus::Dismissed;
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
    /// Root path of the semantic FS (passed to `new`).
    root: std::path::PathBuf,
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
    /// In-memory store of UserFacts (inferred preferences from behavioral patterns).
    /// Keyed by subject_id (person node ID).
    /// Per plico-multi-hop-reasoning.md §6.1: UserFacts are promoted from observations.
    user_facts: RwLock<HashMap<String, Vec<UserFact>>>,
    /// BM25 keyword search index for exact-term matching.
    ///
    /// Per Hindsight (91.4%) vs Zep (63.8%) research: BM25 fills the gap where
    /// vector similarity fails on exact terms (SKU codes, names, error strings).
    bm25_index: Arc<Bm25Index>,
    /// Persistent store of generated ActionSuggestions.
    ///
    /// Suggestions are created by `infer_suggestions_for_event()` and stored here
    /// so they can be queried (pending), confirmed, or dismissed later.
    ///
    /// Key: event_id that triggered the suggestion. Value: list of suggestions.
    suggestion_store: RwLock<HashMap<String, Vec<ActionSuggestion>>>,
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
    /// Root path of this semantic filesystem.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

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
            root: root_path.clone(),
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
            user_facts: RwLock::new(HashMap::new()),
            bm25_index: Arc::new(Bm25Index::new()),
            suggestion_store: RwLock::new(HashMap::new()),
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

            // Also upsert to BM25 for keyword search — done for every text object
            // regardless of embedding outcome (BM25 is independent of the vector index).
            if !text.is_empty() {
                self.bm25_index.upsert(cid, &text);
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

            // Remove from BM25 keyword index
            self.bm25_index.remove(cid);

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
        // Try vector semantic search.
        let query_emb = match self.embedding.embed(query) {
            Ok(emb) => emb,
            Err(e) => {
                tracing::warn!("Embedding failed for query '{query}': {e}. Falling back to tag search.");
                return self.search_by_tags_with_filter(query, &filter);
            }
        };

        // Run vector search — filter is applied inside search_index.search().
        let vector_hits: HashMap<String, f32> = self
            .search_index
            .search(&query_emb, limit * 2, &filter)
            .into_iter()
            .map(|hit| (hit.cid.clone(), hit.score))
            .collect();

        // Run BM25 keyword search — we get CIDs + scores, filter is applied post-hoc.
        let bm25_hits: Vec<(String, f32)> = self.bm25_index.search(query, limit * 2);

        // RRF (Reciprocal Rank Fusion) to combine vector + BM25 rankings.
        // RRF formula: score = Σ 1 / (k + rank), k=60 (standard constant).
        // This is robust to different score scales (vector cosine vs BM25).
        const RRF_K: usize = 60;
        let mut rrf_scores: HashMap<String, f32> = HashMap::new();

        for (cid, score) in &vector_hits {
            rrf_scores.insert(cid.clone(), *score);
        }

        // Collect BM25 CIDs for O(1) membership check after we consume bm25_hits.
        let bm25_cids: std::collections::HashSet<String> =
            bm25_hits.iter().map(|(c, _)| c.clone()).collect();

        // First pass: add RRF contribution from BM25 results that pass the filter.
        for (rank, (cid, _bm25_score)) in bm25_hits.iter().enumerate() {
            if let Ok(obj) = self.cas.get(cid) {
                let meta_for_filter = SearchIndexMeta {
                    cid: cid.clone(),
                    tags: obj.meta.tags.clone(),
                    snippet: String::new(),
                    content_type: format!("{:?}", obj.meta.content_type).to_lowercase(),
                    created_at: obj.meta.created_at,
                };
                if !filter.matches(&meta_for_filter) {
                    continue;
                }
                let entry = rrf_scores.entry(cid.clone()).or_insert(0.0f32);
                *entry += 1.0f32 / (RRF_K as f32 + rank as f32);
            }
        }

        // Second pass: add RRF contribution from vector-only results (not in BM25).
        let vector_cids: Vec<String> = vector_hits.keys().cloned().collect();
        for (rank, cid) in vector_cids.iter().enumerate() {
            if !bm25_cids.contains(cid) {
                if let Some(score) = rrf_scores.get_mut(cid) {
                    *score += 1.0f32 / (RRF_K as f32 + rank as f32);
                }
            }
        }

        // Sort by RRF score descending, take top `limit`.
        let mut sorted: Vec<(String, f32)> = rrf_scores.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(limit);

        // Fetch final objects from CAS.
        sorted
            .into_iter()
            .filter_map(|(cid, relevance)| {
                self.cas.get(&cid).ok().map(|obj| SearchResult {
                    cid,
                    relevance,
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

        // Also index full text in BM25 for keyword search.
        // Use full text (not snippet) — BM25 needs sufficient context to rank well.
        if !text.trim().is_empty() {
            self.bm25_index.upsert(cid, &text);
        }
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
                observation_ids: Vec::new(),
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
                valid_at: None,
                invalid_at: None,
                expired_at: None,
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

        // Add the KG edge with temporal validity
        let edge = KGEdge::new(
            event_id.to_string(),
            target_id.to_string(),
            relation.edge_type(),
            1.0,
        );
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

    /// Associate a behavioral observation with an event.
    ///
    /// Unlike `event_attach` which creates KG edges for documents/attendees,
    /// this method only adds the observation ID to `EventMeta.observation_ids`.
    /// Behavioral observations are not stored as KG nodes — they are managed
    /// by the behavioral pipeline and linked to events for pattern extraction.
    pub fn event_add_observation(
        &self,
        event_id: &str,
        observation_id: &str,
    ) -> Result<(), FSError> {
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not initialized")))?;

        // Update EventMeta on the KG node
        let mut node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;
        let mut meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        if !meta.observation_ids.contains(&observation_id.to_string()) {
            meta.observation_ids.push(observation_id.to_string());
        }

        node.properties = serde_json::to_value(&meta)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        kg.add_node(node)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        Ok(())
    }

    /// Get all behavioral observation IDs associated with an event.
    pub fn event_get_observations(
        &self,
        event_id: &str,
    ) -> Result<Vec<String>, FSError> {
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not initialized")))?;

        let node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;
        let meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        Ok(meta.observation_ids)
    }

    /// Add a UserFact (promoted from behavioral observations) to the preference store.
    ///
    /// Per plico-multi-hop-reasoning.md §6.1: UserFacts are promoted when patterns
    /// repeat across multiple events and stored in the preference store.
    pub fn add_user_fact(&self, fact: UserFact) {
        let mut facts = self.user_facts.write().unwrap();
        facts.entry(fact.subject_id.clone()).or_default().push(fact);
    }

    /// Get all UserFacts for a given subject (person).
    pub fn get_user_facts_for_subject(&self, subject_id: &str) -> Vec<UserFact> {
        let facts = self.user_facts.read().unwrap();
        facts.get(subject_id).cloned().unwrap_or_default()
    }

    /// Infer action suggestions for an event by traversing:
    /// Event → HasAttendee → Person → UserFact → ActionSuggestion
    ///
    /// Returns ActionSuggestions for all attendees with known preferences.
    /// Uses `PatternExtractor::extract_and_suggest` to convert UserFacts to suggestions.
    ///
    /// Per plico-multi-hop-reasoning.md §4.3, §5.1.
    pub fn infer_suggestions_for_event(
        &self,
        event_id: &str,
    ) -> Result<Vec<ActionSuggestion>, FSError> {
        // Step 1: Get event to find attendees
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not initialized")))?;

        let node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;

        let meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        // Step 2: For each attendee, get their UserFacts and generate suggestions
        let mut all_suggestions = Vec::new();
        let event_label = &meta.label;
        let event_type = meta.event_type;

        for attendee_id in &meta.attendee_ids {
            let user_facts = self.get_user_facts_for_subject(attendee_id);
            if user_facts.is_empty() {
                continue;
            }

            // Filter UserFacts by context compatibility with event type
            let relevant_facts: Vec<&UserFact> = user_facts
                .iter()
                .filter(|f| context_matches_event_type(&f.context, event_type))
                .collect();

            if relevant_facts.is_empty() {
                continue;
            }

            // Convert UserFacts to ActionSuggestions (convert refs to owned)
            let owned_facts: Vec<UserFact> = relevant_facts.into_iter().cloned().collect();
            let suggestions = PatternExtractor::extract_and_suggest(
                &owned_facts,
                event_id,
                event_label,
            );
            all_suggestions.extend(suggestions);
        }

        // M16: Cross-person conflict detection
        // After generating per-attendee suggestions, check for preference conflicts
        // among attendees and generate compromise suggestions if needed.
        if let Ok(conflicts) = self.detect_preference_conflicts(event_id) {
            for conflict_group in conflicts {
                if conflict_group.len() >= 2 {
                    // Multiple attendees have different preferences for same context
                    let compromise = self.generate_compromise_suggestions(
                        event_id,
                        &conflict_group,
                        event_label,
                        false, // individual suggestions already generated by extract_and_suggest
                    );
                    all_suggestions.extend(compromise);
                }
            }
        }

        // Store generated suggestions for later query/confirm/dismiss
        if !all_suggestions.is_empty() {
            let mut store = self.suggestion_store.write().unwrap();
            store.insert(event_id.to_string(), all_suggestions.clone());
        }

        Ok(all_suggestions)
    }

    /// Get all pending (unconfirmed/undismissed) suggestions across all events.
    ///
    /// Used by the notification system to surface actionable suggestions to users.
    ///
    /// Per plico-multi-hop-reasoning.md §4.1: "4. Proactive（主动提议）"
    pub fn get_pending_suggestions(&self) -> Vec<ActionSuggestion> {
        let store = self.suggestion_store.read().unwrap();
        store
            .values()
            .flatten()
            .filter(|s| s.status == SuggestionStatus::Pending)
            .filter(|s| !s.is_too_uncertain()) // filter out very uncertain ones
            .cloned()
            .collect()
    }

    /// Get all suggestions for a specific event.
    pub fn get_suggestions_for_event(&self, event_id: &str) -> Vec<ActionSuggestion> {
        let store = self.suggestion_store.read().unwrap();
        store.get(event_id).cloned().unwrap_or_default()
    }

    /// Confirm a suggestion by ID, marking it as accepted by the user.
    pub fn confirm_suggestion(&self, suggestion_id: &str) -> Result<(), FSError> {
        let mut store = self.suggestion_store.write().unwrap();
        for suggestions in store.values_mut() {
            if let Some(sug) = suggestions.iter_mut().find(|s| s.id == suggestion_id) {
                sug.confirm();
                return Ok(());
            }
        }
        Err(FSError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("suggestion {} not found", suggestion_id),
        )))
    }

    /// Dismiss a suggestion by ID, marking it as rejected by the user.
    pub fn dismiss_suggestion(&self, suggestion_id: &str) -> Result<(), FSError> {
        let mut store = self.suggestion_store.write().unwrap();
        for suggestions in store.values_mut() {
            if let Some(sug) = suggestions.iter_mut().find(|s| s.id == suggestion_id) {
                sug.dismiss();
                return Ok(());
            }
        }
        Err(FSError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("suggestion {} not found", suggestion_id),
        )))
    }

    /// Count of pending suggestions (for dashboard status).
    pub fn pending_suggestion_count(&self) -> usize {
        self.get_pending_suggestions().len()
    }

    /// Detect preference conflicts among event attendees.
    ///
    /// When multiple attendees have different preferences for the same context,
    /// this generates conflict groups for resolution.
    ///
    /// Per plico-multi-hop-reasoning.md §5.3: "跨人推理链"
    ///
    /// Returns a list of conflict groups, each containing UserFacts with different
    /// preferred objects for the same context.
    pub fn detect_preference_conflicts(
        &self,
        event_id: &str,
    ) -> Result<Vec<Vec<UserFact>>, FSError> {
        let kg = self.knowledge_graph.as_ref()
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not initialized")))?;

        let node = kg.get_node(event_id)
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?
            .ok_or_else(|| FSError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "event not found")))?;

        let meta: EventMeta = serde_json::from_value(node.properties.clone())
            .map_err(|e| FSError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

        let event_type = meta.event_type;

        // Collect all relevant preferences for all attendees (owned values)
        let mut all_prefs: Vec<UserFact> = Vec::new();
        for attendee_id in &meta.attendee_ids {
            let user_facts = self.get_user_facts_for_subject(attendee_id);
            for fact in user_facts {
                if context_matches_event_type(&fact.context, event_type) {
                    all_prefs.push(fact);
                }
            }
        }

        // Group by (context) to find conflicts
        let mut conflicts: Vec<Vec<UserFact>> = Vec::new();
        let mut by_context: std::collections::HashMap<String, Vec<&UserFact>> =
            std::collections::HashMap::new();

        for pref in &all_prefs {
            by_context.entry(pref.context.clone()).or_default().push(pref);
        }

        // A conflict exists when same context has multiple different objects
        for (_context, prefs) in by_context {
            let mut objects: std::collections::HashSet<String> = std::collections::HashSet::new();
            for pref in &prefs {
                objects.insert(pref.object.clone());
            }
            if objects.len() > 1 {
                // Conflict detected: multiple different preferences for same context
                let conflict: Vec<UserFact> = prefs.iter().map(|p| (*p).clone()).collect();
                conflicts.push(conflict);
            }
        }

        Ok(conflicts)
    }

    /// Generate compromise suggestions when attendees have conflicting preferences.
    ///
    /// When `include_individual` is true, returns both per-person suggestions and a
    /// compromise option. When false, returns only the compromise suggestion (used when
    /// individual suggestions are already generated by `infer_suggestions_for_event`).
    ///
    /// Per plico-multi-hop-reasoning.md §5.3 Step 4.
    pub fn generate_compromise_suggestions(
        &self,
        event_id: &str,
        conflict_group: &[UserFact],
        event_label: &str,
        include_individual: bool,
    ) -> Vec<ActionSuggestion> {
        let mut suggestions = Vec::new();

        // Individual suggestions (only when not already generated)
        if include_individual {
            for fact in conflict_group {
                let action = PatternExtractor::action_for_fact(fact);
                let reasoning = format!(
                    "{} 偏好 {}（基于历史行为）",
                    fact.subject_id, fact.object
                );
                suggestions.push(ActionSuggestion::new(
                    event_id.to_string(),
                    fact.subject_id.clone(),
                    action,
                    vec![reasoning],
                    fact.subject_id.clone(),
                    fact.object.clone(),
                    fact.context.clone(),
                    fact.confidence,
                ));
            }
        }

        // Compromise suggestion (香槟 as neutral option)
        let objects: Vec<&str> = conflict_group.iter().map(|f| f.object.as_str()).collect();
        let compromise_reasoning = vec![
            format!("参会人员偏好不同：{}", objects.join(" vs ")),
            "建议选择香槟作为折中方案".to_string(),
            format!("适用于 {} 场合", event_label),
        ];

        suggestions.push(ActionSuggestion::new(
            event_id.to_string(),
            "compromise".to_string(),
            "准备香槟（折中方案）".to_string(),
            compromise_reasoning,
            "compromise".to_string(),
            "香槟".to_string(),
            "compromise".to_string(),
            0.5, // Lower confidence for compromise
        ));

        suggestions
    }
}

/// Check if a preference context is relevant for a given event type.
///
/// Per plico-multi-hop-reasoning.md §5.1: "Preference.context = 'at_business_dinner'
/// EventContainer.event_type = Meal → 匹配"
fn context_matches_event_type(context: &str, event_type: EventType) -> bool {
    match event_type {
        EventType::Meeting => {
            // Meeting events match dining-related contexts
            context.contains("dinner")
                || context.contains("lunch")
                || context.contains("breakfast")
                || context.contains("meal")
                || context.contains("business")
        }
        EventType::Entertainment => {
            // Entertainment events match drinking/social contexts
            context.contains("drunk")
                || context.contains("party")
                || context.contains("social")
                || context.contains("entertainment")
        }
        EventType::Travel => {
            // Travel matches when on trip contexts
            context.contains("travel")
                || context.contains("trip")
                || context.contains("journey")
        }
        EventType::Social => {
            // Social events match any social context
            context.contains("social")
                || context.contains("party")
                || context.contains("gathering")
                || context.contains("dinner")
                || context.contains("lunch")
        }
        // For other event types, accept if context is generic or empty
        _ => {
            context.is_empty()
                || context == "general"
                || context == "default"
        }
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
            observation_ids: vec![],
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
    fn event_add_and_get_observation() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let event_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        let obs_id = "obs:dining:2026-04-15:001";

        // Initially no observations
        let obs_ids = fs.event_get_observations(&event_id).unwrap();
        assert!(obs_ids.is_empty());

        // Add observation
        fs.event_add_observation(&event_id, obs_id).unwrap();

        // Should now contain the observation
        let obs_ids = fs.event_get_observations(&event_id).unwrap();
        assert_eq!(obs_ids.len(), 1);
        assert_eq!(obs_ids[0], obs_id);

        // Adding same observation again should be idempotent
        fs.event_add_observation(&event_id, obs_id).unwrap();
        let obs_ids = fs.event_get_observations(&event_id).unwrap();
        assert_eq!(obs_ids.len(), 1); // Still 1, not 2
    }

    #[test]
    fn event_add_multiple_observations() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let event_id = fs.create_event("会议", EventType::Meeting, None, None, None, vec![], "a").unwrap();

        fs.event_add_observation(&event_id, "obs:1").unwrap();
        fs.event_add_observation(&event_id, "obs:2").unwrap();
        fs.event_add_observation(&event_id, "obs:3").unwrap();

        let obs_ids = fs.event_get_observations(&event_id).unwrap();
        assert_eq!(obs_ids.len(), 3);
    }

    #[test]
    fn event_add_observation_not_found() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let result = fs.event_add_observation("nonexistent-event", "obs:1");
        assert!(result.is_err());
    }

    #[test]
    fn event_get_observations_not_found() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs_with_kg(&dir);

        let result = fs.event_get_observations("nonexistent-event");
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

    #[test]
    fn extract_and_suggest_generates_suggestion_for_high_confidence_fact() {
        // 3 identical observations → UserFact with confidence = 1.0 → ActionSuggestion
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
        assert_eq!(facts.len(), 1);

        let suggestions = PatternExtractor::extract_and_suggest(&facts, "evt:dinner1", "商务晚餐");
        assert_eq!(suggestions.len(), 1);
        let sug = &suggestions[0];
        assert_eq!(sug.target_person_id, "person:wang");
        assert_eq!(sug.preference_object, "红酒");
        assert_eq!(sug.preference_context, "at_dinner");
        assert_eq!(sug.trigger_event_id, "evt:dinner1");
        assert!(sug.action.contains("红酒"), "action should mention wine");
        assert_eq!(sug.status, SuggestionStatus::Pending);
        assert!(!sug.is_too_uncertain());
    }

    #[test]
    fn extract_and_suggest_filters_low_confidence_facts() {
        // 2 observations → below threshold → no UserFact → no suggestion
        let obs = vec![
            BehavioralObservation::new(
                "person:li".to_string(),
                "at_dinner".to_string(),
                "order_food".to_string(),
                "威士忌".to_string(),
                None,
            ),
            BehavioralObservation::new(
                "person:li".to_string(),
                "at_dinner".to_string(),
                "order_food".to_string(),
                "威士忌".to_string(),
                None,
            ),
        ];
        let facts = PatternExtractor::extract(&obs);
        assert!(facts.is_empty());

        let suggestions = PatternExtractor::extract_and_suggest(&facts, "evt:dinner2", "晚餐");
        assert!(suggestions.is_empty());
    }

    #[test]
    fn extract_and_suggest_wine_action_mapping() {
        // wine → "提醒带红酒"
        let obs: Vec<BehavioralObservation> = (0..3)
            .map(|_| {
                BehavioralObservation::new(
                    "person:zhang".to_string(),
                    "at_business_dinner".to_string(),
                    "order_food".to_string(),
                    "wine".to_string(),
                    None,
                )
            })
            .collect();
        let facts = PatternExtractor::extract(&obs);
        let suggestions = PatternExtractor::extract_and_suggest(&facts, "evt:1", "商务宴请");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].action, "提醒带红酒");
    }

    #[test]
    fn extract_and_suggest_congee_action_mapping() {
        // white_congee → "准备白粥"
        let obs: Vec<BehavioralObservation> = (0..3)
            .map(|_| {
                BehavioralObservation::new(
                    "person:wang".to_string(),
                    "when_drunk".to_string(),
                    "order_food".to_string(),
                    "white_congee".to_string(),
                    None,
                )
            })
            .collect();
        let facts = PatternExtractor::extract(&obs);
        let suggestions = PatternExtractor::extract_and_suggest(&facts, "evt:2", "宿醉后");
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].action, "准备白粥");
    }

    #[test]
    fn extract_and_suggest_reasoning_chain_length() {
        let obs: Vec<BehavioralObservation> = (0..3)
            .map(|_i| {
                BehavioralObservation::new(
                    "person:test".to_string(),
                    "at_dinner".to_string(),
                    "order_food".to_string(),
                    "beer".to_string(),
                    None,
                )
            })
            .collect();
        let facts = PatternExtractor::extract(&obs);
        let suggestions = PatternExtractor::extract_and_suggest(&facts, "evt:1", "晚餐");
        assert_eq!(suggestions.len(), 1);
        // reasoning chain should have 3 steps
        assert_eq!(suggestions[0].reasoning_chain.len(), 3);
    }

    #[test]
    fn add_and_get_user_facts() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);

        let fact = UserFact {
            id: "fact:1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string()],
            updated_at: 0,
        };

        fs.add_user_fact(fact.clone());
        let facts = fs.get_user_facts_for_subject("person:wang");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].object, "wine");

        // Non-existent subject returns empty
        let facts = fs.get_user_facts_for_subject("person:unknown");
        assert!(facts.is_empty());
    }

    #[test]
    fn infer_suggestions_for_event_with_attendee_preference() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Create event with attendee
        let event_id = fs.create_event(
            "商务晚餐",
            EventType::Meeting,
            None,
            None,
            None,
            vec![],
            "a",
        ).unwrap();

        // Attach attendee
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();

        // Add UserFact for the attendee
        let fact = UserFact {
            id: "fact:1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string(), "obs:3".to_string()],
            updated_at: 0,
        };
        fs.add_user_fact(fact);

        // Infer suggestions
        let suggestions = fs.infer_suggestions_for_event(&event_id).unwrap();
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].target_person_id, "person:wang");
        assert_eq!(suggestions[0].preference_object, "wine");
        assert!(suggestions[0].action.contains("红酒"));
    }

    #[test]
    fn infer_suggestions_returns_empty_when_no_preferences() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        let event_id = fs.create_event("会议", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:unknown", EventRelation::Attendee, "a").unwrap();

        // No UserFacts for person:unknown → empty suggestions
        let suggestions = fs.infer_suggestions_for_event(&event_id).unwrap();
        assert!(suggestions.is_empty());
    }

    #[test]
    fn infer_suggestions_filters_by_context() {
        // A UserFact with context "at_dinner" should match EventType::Meeting
        // but not EventType::Travel
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Add UserFact with "at_dinner" context
        let fact = UserFact {
            id: "fact:dinner".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string(), "obs:3".to_string()],
            updated_at: 0,
        };
        fs.add_user_fact(fact);

        // Meeting event should match "at_dinner" context
        let meeting_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&meeting_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        let suggestions = fs.infer_suggestions_for_event(&meeting_id).unwrap();
        assert_eq!(suggestions.len(), 1, "Meeting event should match at_dinner context");

        // Travel event should NOT match "at_dinner" context
        let travel_id = fs.create_event("出差", EventType::Travel, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&travel_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        let suggestions = fs.infer_suggestions_for_event(&travel_id).unwrap();
        assert!(suggestions.is_empty(), "Travel event should not match at_dinner context");
    }

    #[test]
    fn infer_suggestions_matches_drunk_context_with_entertainment() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Add UserFact with "when_drunk" context
        let fact = UserFact {
            id: "fact:drunk".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "white_congee".to_string(),
            context: "when_drunk".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string(), "obs:3".to_string()],
            updated_at: 0,
        };
        fs.add_user_fact(fact);

        // Entertainment event should match "when_drunk" context
        let party_id = fs.create_event("聚会", EventType::Entertainment, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&party_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        let suggestions = fs.infer_suggestions_for_event(&party_id).unwrap();
        assert_eq!(suggestions.len(), 1, "Entertainment event should match when_drunk context");
    }

    #[test]
    fn infer_suggestions_for_event_with_cross_person_conflicts() {
        // M16: Two attendees with conflicting preferences should generate
        // individual suggestions PLUS a compromise suggestion.
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Two conflicting preferences for same context
        fs.add_user_fact(UserFact {
            id: "fact:w1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });
        fs.add_user_fact(UserFact {
            id: "fact:l1".to_string(),
            subject_id: "person:li".to_string(),
            predicate: "prefers".to_string(),
            object: "whisky".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });

        let event_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        fs.event_attach(&event_id, "person:li", EventRelation::Attendee, "a").unwrap();

        let suggestions = fs.infer_suggestions_for_event(&event_id).unwrap();
        // 2 individual suggestions + 1 compromise = 3
        assert_eq!(suggestions.len(), 3, "Should have 2 individual + 1 compromise suggestion");

        // Verify compromise suggestion exists
        let compromise = suggestions.iter().find(|s| s.preference_object == "香槟");
        assert!(compromise.is_some(), "Compromise suggestion (champagne) should be present");

        // Verify individual suggestions for each person exist
        let wine_sug = suggestions.iter().find(|s| s.preference_object == "wine");
        let whisky_sug = suggestions.iter().find(|s| s.preference_object == "whisky");
        assert!(wine_sug.is_some(), "Wine suggestion for wang should be present");
        assert!(whisky_sug.is_some(), "Whisky suggestion for li should be present");
    }

    #[test]
    fn action_suggestion_confirm_and_dismiss() {
        let mut sug = ActionSuggestion::new(
            "evt:x".to_string(),
            "person:y".to_string(),
            "提醒带红酒".to_string(),
            vec!["王总偏好红酒".to_string()],
            "person:wang".to_string(),
            "wine".to_string(),
            "at_dinner".to_string(),
            0.75,
        );
        assert_eq!(sug.status, SuggestionStatus::Pending);

        sug.confirm();
        assert_eq!(sug.status, SuggestionStatus::Confirmed);

        sug.dismiss();
        assert_eq!(sug.status, SuggestionStatus::Dismissed);
    }

    #[test]
    fn detect_conflicts_when_preferences_differ() {
        // Two people with different preferences for the same context
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Wang prefers wine
        fs.add_user_fact(UserFact {
            id: "fact:1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });

        // Li prefers whisky
        fs.add_user_fact(UserFact {
            id: "fact:2".to_string(),
            subject_id: "person:li".to_string(),
            predicate: "prefers".to_string(),
            object: "whisky".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });

        let event_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        fs.event_attach(&event_id, "person:li", EventRelation::Attendee, "a").unwrap();

        let conflicts = fs.detect_preference_conflicts(&event_id).unwrap();
        assert_eq!(conflicts.len(), 1, "Should detect one conflict group");
        assert_eq!(conflicts[0].len(), 2, "Conflict group should have 2 attendees");
    }

    #[test]
    fn no_conflicts_when_preferences_same() {
        // Two people with the same preference
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Both prefer wine
        fs.add_user_fact(UserFact {
            id: "fact:1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });

        fs.add_user_fact(UserFact {
            id: "fact:2".to_string(),
            subject_id: "person:li".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec![],
            updated_at: 0,
        });

        let event_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        fs.event_attach(&event_id, "person:li", EventRelation::Attendee, "a").unwrap();

        let conflicts = fs.detect_preference_conflicts(&event_id).unwrap();
        assert!(conflicts.is_empty(), "No conflicts when preferences are the same");
    }

    #[test]
    fn generate_compromise_suggestions() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        let conflict_group = vec![
            UserFact {
                id: "fact:1".to_string(),
                subject_id: "person:wang".to_string(),
                predicate: "prefers".to_string(),
                object: "wine".to_string(),
                context: "at_dinner".to_string(),
                confidence: 0.85,
                evidence_ids: vec![],
                updated_at: 0,
            },
            UserFact {
                id: "fact:2".to_string(),
                subject_id: "person:li".to_string(),
                predicate: "prefers".to_string(),
                object: "whisky".to_string(),
                context: "at_dinner".to_string(),
                confidence: 0.85,
                evidence_ids: vec![],
                updated_at: 0,
            },
        ];

        let suggestions = fs.generate_compromise_suggestions("evt:1", &conflict_group, "商务晚餐", true);
        assert_eq!(suggestions.len(), 3, "2 individual + 1 compromise");

        // Check that compromise has lower confidence
        let compromise = suggestions.iter().find(|s| s.preference_object == "香槟").unwrap();
        assert_eq!(compromise.confidence, 0.5);
    }

    #[test]
    fn test_suggestion_store_infer_and_query() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        // Create event with attendee
        let event_id = fs.create_event("商务晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();

        // Add UserFact
        let fact = UserFact {
            id: "fact:1".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string(), "obs:3".to_string()],
            updated_at: 0,
        };
        fs.add_user_fact(fact);

        // Infer suggestions (should store them)
        let suggestions = fs.infer_suggestions_for_event(&event_id).unwrap();
        assert_eq!(suggestions.len(), 1);
        let sug_id = suggestions[0].id.clone();

        // Query suggestions for this event
        let event_sugs = fs.get_suggestions_for_event(&event_id);
        assert_eq!(event_sugs.len(), 1);

        // Query pending suggestions
        let pending = fs.get_pending_suggestions();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].status, SuggestionStatus::Pending);

        // Dismiss the suggestion
        fs.dismiss_suggestion(&sug_id).unwrap();

        // Pending should now be empty
        let pending_after = fs.get_pending_suggestions();
        assert!(pending_after.is_empty());
    }

    #[test]
    fn test_suggestion_store_confirm() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        let event_id = fs.create_event("晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        let fact = UserFact {
            id: "fact:x".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.9,
            evidence_ids: vec![],
            updated_at: 0,
        };
        fs.add_user_fact(fact);

        let suggestions = fs.infer_suggestions_for_event(&event_id).unwrap();
        assert_eq!(suggestions.len(), 1);
        let sug_id = suggestions[0].id.clone();

        fs.confirm_suggestion(&sug_id).unwrap();

        let after = fs.get_pending_suggestions();
        assert!(after.is_empty()); // confirmed = no longer pending
    }

    #[test]
    fn test_pending_suggestion_count() {
        let dir = TempDir::new().unwrap();
        let kg = Arc::new(PetgraphBackend::new());
        let fs = SemanticFS::new(
            dir.path().to_path_buf(),
            Arc::new(StubEmbeddingProvider::new()),
            Arc::new(InMemoryBackend::new()),
            None,
            Some(kg.clone()),
        )
        .unwrap();

        assert_eq!(fs.pending_suggestion_count(), 0);

        let event_id = fs.create_event("晚餐", EventType::Meeting, None, None, None, vec![], "a").unwrap();
        fs.event_attach(&event_id, "person:wang", EventRelation::Attendee, "a").unwrap();
        let fact = UserFact {
            id: "fact:y".to_string(),
            subject_id: "person:wang".to_string(),
            predicate: "prefers".to_string(),
            object: "wine".to_string(),
            context: "at_dinner".to_string(),
            confidence: 0.85,
            evidence_ids: vec!["obs:1".to_string(), "obs:2".to_string(), "obs:3".to_string()],
            updated_at: 0,
        };
        fs.add_user_fact(fact);
        fs.infer_suggestions_for_event(&event_id).unwrap();

        assert_eq!(fs.pending_suggestion_count(), 1);
    }
}
