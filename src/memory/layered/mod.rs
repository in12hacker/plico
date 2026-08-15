//! Layered Memory Implementation
//!
//! Implements the 4-tier memory hierarchy. Each tier has different
//! characteristics for capacity, latency, and persistence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

#[cfg(test)]
pub mod tests;

/// Memory visibility scope — controls cross-agent access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MemoryScope {
    /// Only the owning agent can read/write.
    #[default]
    Private,
    /// Any agent can read; only the owner can write.
    Shared,
    /// Agents in the named group can read; only the owner can write.
    Group(String),
}

/// Cognitive memory type — orthogonal to tier, classifies memory by nature.
///
/// Based on ENGRAM (ICLR 2026) and cognitive science:
/// - Episodic: events and experiences with temporal context ("what happened when")
/// - Semantic: stable facts and preferences ("user likes X")
/// - Procedural: reusable workflows and skills ("how to do Y")
/// - Untyped: legacy/unclassified entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum MemoryType {
    Episodic,
    Semantic,
    Procedural,
    #[default]
    Untyped,
}

impl MemoryType {
    pub fn name(&self) -> &'static str {
        match self {
            MemoryType::Episodic => "episodic",
            MemoryType::Semantic => "semantic",
            MemoryType::Procedural => "procedural",
            MemoryType::Untyped => "untyped",
        }
    }

    pub fn from_str_loose(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "episodic" | "event" | "episode" => MemoryType::Episodic,
            "semantic" | "fact" | "knowledge" => MemoryType::Semantic,
            "procedural" | "procedure" | "skill" | "workflow" => MemoryType::Procedural,
            _ => MemoryType::Untyped,
        }
    }
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Memory tier classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    /// Active conversation state — highest priority, lowest capacity
    Ephemeral,
    /// Mid-term project context — medium capacity
    Working,
    /// Long-term persistent knowledge — high capacity, vector-indexed
    LongTerm,
    /// Learned workflows and skills — persistent, procedural
    Procedural,
}

/// Stable identity of one logical memory across immutable revisions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct MemoryId(String);

/// Identity of one immutable canonical memory revision.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct MemoryRevisionId(String);

/// SHA-256 of canonical content only. Runtime counters and projections are excluded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct CanonicalContentHash(String);

macro_rules! string_identity {
    ($type:ty) => {
        impl $type {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl From<String> for $type {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $type {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl std::fmt::Display for $type {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl Deref for $type {
            type Target = str;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl AsRef<str> for $type {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl Borrow<str> for $type {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl PartialEq<str> for $type {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $type {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl PartialEq<String> for $type {
            fn eq(&self, other: &String) -> bool {
                self.0 == *other
            }
        }
    };
}

string_identity!(MemoryId);
string_identity!(MemoryRevisionId);
string_identity!(CanonicalContentHash);

impl MemoryTier {
    /// Relative priority (higher = more urgent eviction candidate).
    pub fn priority(&self) -> u8 {
        match self {
            MemoryTier::Ephemeral => 3,
            MemoryTier::Working => 2,
            MemoryTier::LongTerm => 1,
            MemoryTier::Procedural => 0, // Never evicted
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            MemoryTier::Ephemeral => "ephemeral",
            MemoryTier::Working => "working",
            MemoryTier::LongTerm => "long_term",
            MemoryTier::Procedural => "procedural",
        }
    }
}

impl std::fmt::Display for MemoryTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// A single memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEntry {
    /// Unique ID for this immutable revision. Public `entry_id` maps here.
    pub id: String,

    /// Stable identity shared by every revision of the same logical memory.
    pub memory_id: MemoryId,

    /// Previous immutable revision in this logical memory stream.
    pub parent_revision_id: Option<MemoryRevisionId>,

    /// Canonical content hash. Operational and derived fields never affect it.
    pub canonical_content_hash: CanonicalContentHash,

    /// Origin local AgentRole within this personal vault.
    pub agent_id: String,

    /// Legacy namespace retained only for personal-vault migration.
    #[serde(default)]
    pub tenant_id: String,

    /// The tier this entry lives in.
    pub tier: MemoryTier,

    /// Content of this memory entry.
    pub content: MemoryContent,

    /// Importance score (0-100). Higher = less likely to be evicted.
    pub importance: u8,

    /// Access count — more accessed = less likely to be evicted.
    pub access_count: u32,

    /// Last accessed timestamp (milliseconds).
    pub last_accessed: u64,

    /// Created timestamp (milliseconds).
    pub created_at: u64,

    /// Semantic tags for retrieval.
    pub tags: Vec<String>,

    /// Time-to-live in milliseconds. When set, the entry expires after
    /// `created_at + ttl_ms` and is evicted during the next cleanup pass.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,

    /// Original TTL set at creation time — used to compute TTL refresh on access.
    /// Stored separately so refresh doesn't compound exponentially.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_ttl_ms: Option<u64>,

    /// Visibility scope — Private (default), Shared, or Group.
    #[serde(default)]
    pub scope: MemoryScope,

    /// Cognitive memory type — episodic, semantic, procedural, or untyped.
    #[serde(default)]
    pub memory_type: MemoryType,

    /// Causal parent — ID of the memory that causally led to this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal_parent: Option<String>,

    /// Supersedes — ID of the memory this one replaces (contradiction resolution).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,

    /// Legacy/runtime compatibility relation. Canonical writers reject this
    /// field and represent updates only through immutable parent revisions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,

    /// Tombstone timestamp (milliseconds). Canonical deletion sets this only
    /// on the newly appended tombstone revision and never rewrites its parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemoryContent {
    /// Plain text content.
    Text(String),
    /// Reference to a CAS object (CID).
    ObjectRef(String),
    /// Structured data (JSON).
    Structured(serde_json::Value),
    /// A learned procedure/workflow.
    Procedure(Procedure),
    /// A piece of accumulated knowledge.
    Knowledge(KnowledgePiece),
}

impl MemoryContent {
    /// Extract text content, if available.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            MemoryContent::Text(s) => Some(s),
            _ => None,
        }
    }

    /// Get content as a displayable string.
    pub fn display(&self) -> String {
        match self {
            MemoryContent::Text(s) => s.clone(),
            MemoryContent::ObjectRef(cid) => format!("[ObjectRef: {}]", cid),
            MemoryContent::Structured(v) => serde_json::to_string(v).unwrap_or_default(),
            MemoryContent::Procedure(p) => p.description.clone(),
            MemoryContent::Knowledge(k) => k.statement.clone(),
        }
    }

    /// Hash the canonical payload without operational counters, timestamps,
    /// visibility, tier placement, or derived projections.
    pub fn canonical_content_hash(&self) -> Result<CanonicalContentHash, &'static str> {
        let mut hasher = Sha256::new();
        hasher.update(b"plico.memory.content.v1\0");
        match self {
            MemoryContent::Text(value) => hash_component(&mut hasher, b"text", value.as_bytes()),
            MemoryContent::ObjectRef(value) => hash_component(&mut hasher, b"object_ref", value.as_bytes()),
            MemoryContent::Structured(value) => {
                ensure_jcs_safe_numbers(value)?;
                let encoded = serde_json_canonicalizer::to_vec(value).map_err(|_| "jcs_canonicalization_failed")?;
                hash_component(&mut hasher, b"structured_json", &encoded);
            }
            MemoryContent::Procedure(procedure) => {
                hash_component(&mut hasher, b"procedure_name", procedure.name.as_bytes());
                hash_component(&mut hasher, b"procedure_description", procedure.description.as_bytes());
                hash_component(
                    &mut hasher,
                    b"procedure_learned_from",
                    procedure.learned_from.as_bytes(),
                );
                for step in &procedure.steps {
                    hash_component(&mut hasher, b"step_number", &step.step_number.to_be_bytes());
                    hash_component(&mut hasher, b"step_description", step.description.as_bytes());
                    hash_component(&mut hasher, b"step_action", step.action.as_bytes());
                    hash_component(&mut hasher, b"step_outcome", step.expected_outcome.as_bytes());
                }
            }
            MemoryContent::Knowledge(knowledge) => {
                if !knowledge.confidence.is_finite() {
                    return Err("non_finite_knowledge_confidence");
                }
                hash_component(&mut hasher, b"knowledge_subject", knowledge.subject.as_bytes());
                hash_component(&mut hasher, b"knowledge_statement", knowledge.statement.as_bytes());
                hash_component(
                    &mut hasher,
                    b"knowledge_confidence",
                    &knowledge.confidence.to_bits().to_be_bytes(),
                );
                hash_component(&mut hasher, b"knowledge_source", knowledge.source.as_bytes());
            }
        }
        Ok(CanonicalContentHash(format!("{:x}", hasher.finalize())))
    }
}

const MAX_JCS_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn ensure_jcs_safe_numbers(value: &serde_json::Value) -> Result<(), &'static str> {
    match value {
        serde_json::Value::Number(number) => {
            if number.as_u64().is_some_and(|value| value > MAX_JCS_SAFE_INTEGER)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() > MAX_JCS_SAFE_INTEGER)
            {
                return Err("jcs_unsafe_integer");
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                ensure_jcs_safe_numbers(value)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                ensure_jcs_safe_numbers(value)?;
            }
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::String(_) => {}
    }
    Ok(())
}

fn hash_component(hasher: &mut Sha256, kind: &[u8], value: &[u8]) {
    hasher.update((kind.len() as u64).to_be_bytes());
    hasher.update(kind);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

/// A learned procedure — persisted workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Procedure {
    pub name: String,
    pub description: String,
    /// Steps in the procedure
    pub steps: Vec<ProcedureStep>,
    /// When this procedure was learned/learned_from
    pub learned_from: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureStep {
    pub step_number: u32,
    pub description: String,
    pub action: String,
    pub expected_outcome: String,
}

/// A piece of accumulated knowledge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgePiece {
    pub subject: String,
    pub statement: String,
    pub confidence: f32,
    pub source: String,
}

impl MemoryEntry {
    fn root_identity(id: &str, content: &MemoryContent) -> Result<(MemoryId, CanonicalContentHash), &'static str> {
        Ok((MemoryId(id.to_string()), content.canonical_content_hash()?))
    }

    /// Seal an internal root-revision builder before it enters a live tier.
    /// Persisted entries never use empty identities; old snapshots therefore
    /// cannot be silently interpreted as the new schema.
    fn seal_root_identity(&mut self) -> Result<(), &'static str> {
        if self.memory_id.is_empty() {
            self.memory_id = MemoryId(self.id.to_string());
        }
        if self.canonical_content_hash.is_empty() {
            self.canonical_content_hash = self.content.canonical_content_hash()?;
        }
        Ok(())
    }

    /// Default tenant ID when no tenant is specified.
    pub fn default_tenant() -> String {
        crate::DEFAULT_TENANT.to_string()
    }

    /// Create a new ephemeral memory entry.
    pub fn ephemeral(agent_id: impl Into<String>, content: impl Into<String>) -> Self {
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let content = MemoryContent::Text(content.into());
        let (memory_id, canonical_content_hash) =
            Self::root_identity(&id, &content).expect("text memory content must always have a canonical hash");
        Self {
            id,
            memory_id,
            parent_revision_id: None,
            canonical_content_hash,
            agent_id: agent_id.into(),
            tenant_id: Self::default_tenant(),
            tier: MemoryTier::Ephemeral,
            content,
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Untyped,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        }
    }

    /// Create a new long-term memory entry.
    pub fn long_term(agent_id: impl Into<String>, content: MemoryContent, tags: Vec<String>) -> Self {
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let (memory_id, canonical_content_hash) = Self::root_identity(&id, &content)
            .expect("MemoryEntry::long_term requires canonicalizable trusted content");
        Self {
            id,
            memory_id,
            parent_revision_id: None,
            canonical_content_hash,
            agent_id: agent_id.into(),
            tenant_id: Self::default_tenant(),
            tier: MemoryTier::LongTerm,
            content,
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Untyped,
            causal_parent: None,
            supersedes: None,
            superseded_by: None,
            deleted_at: None,
        }
    }

    /// Set the cognitive memory type.
    pub fn with_memory_type(mut self, memory_type: MemoryType) -> Self {
        self.memory_type = memory_type;
        self
    }

    /// Set the causal parent (the memory that led to this one).
    pub fn with_causal_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.causal_parent = Some(parent_id.into());
        self
    }

    /// Assign a caller-supplied identity to a newly built root revision.
    pub fn with_root_revision_id(mut self, revision_id: impl Into<String>) -> Self {
        let revision_id = revision_id.into();
        self.id = revision_id.clone();
        self.memory_id = MemoryId(revision_id);
        self.parent_revision_id = None;
        self
    }

    /// Record an access to this entry and refresh its TTL.
    ///
    /// TTL extension = original_ttl_ms * min(access_count, 5), capped at 5x original.
    /// This implements F-17: Access-Frequency TTL refresh.
    pub fn on_memory_access(&mut self) {
        self.access_count += 1;
        self.last_accessed = now_ms();

        // Refresh TTL if entry has one
        if let Some(original) = self.original_ttl_ms {
            let multiplier = std::cmp::min(self.access_count, 5) as u64;
            let new_ttl = original.saturating_mul(multiplier);
            self.ttl_ms = Some(new_ttl);
        }
    }
}

/// Global memory manager
///
/// Global memory manager — holds all agents' memory tiers.
///
/// Can optionally be paired with a [`CanonicalLedger`] for durable memory
/// across restarts.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Entry not found: id={0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Tier capacity exceeded: tier={tier}, agent={agent}")]
    TierCapacityExceeded { tier: MemoryTier, agent: String },

    #[error("Memory quota exceeded: agent={agent_id}, current={current}, limit={limit}")]
    QuotaExceeded {
        agent_id: String,
        current: usize,
        limit: u64,
    },

    #[error("canonical memory content cannot be hashed: {category}")]
    InvalidCanonicalContent { category: &'static str },

    #[error("canonical memory ledger rejected the entry: {0}")]
    Ledger(#[from] crate::memory::LedgerError),
}

/// A typed failure from a canonical Working Memory mutation.
///
/// These mutations publish one complete per-role Working snapshot only after
/// the configured canonical ledger has durably committed it. They intentionally do
/// not fall back to in-memory success.
#[derive(Debug, thiserror::Error)]
pub enum DurableMemoryMutationError {
    #[error("working memory entry not found: {entry_id}")]
    NotFound { entry_id: String },

    #[error("working memory entry is not active: {entry_id}")]
    Inactive { entry_id: String },

    #[error("working memory entry is outside the local namespace: {entry_id}")]
    NamespaceMismatch { entry_id: String },

    #[error("working memory update content must not be empty")]
    EmptyContent,

    #[error("canonical memory content cannot be hashed: {category}")]
    InvalidCanonicalContent { category: &'static str },

    #[error("memory quota exceeded: current={current}, limit={limit}")]
    QuotaExceeded { current: usize, limit: u64 },

    #[error("Working Memory mutation is not permitted")]
    PermissionDenied,

    #[error("durable Working Memory persistence is unavailable")]
    PersistenceUnavailable,

    #[error("failed to commit canonical Working Memory mutation: {0}")]
    Ledger(#[from] crate::memory::LedgerError),
}

impl DurableMemoryMutationError {
    pub(crate) fn category(&self) -> &'static str {
        match self {
            Self::NotFound { .. } => "not_found",
            Self::Inactive { .. } => "inactive",
            Self::NamespaceMismatch { .. } => "namespace_mismatch",
            Self::EmptyContent => "empty_content",
            Self::InvalidCanonicalContent { category } => category,
            Self::QuotaExceeded { .. } => "quota_exceeded",
            Self::PermissionDenied => "permission_denied",
            Self::PersistenceUnavailable => "persistence_unavailable",
            Self::Ledger(error) => error.category(),
        }
    }
}

pub struct LayeredMemory {
    /// Per-agent ephemeral memories (in-memory only).
    ephemeral: RwLock<HashMap<String, Vec<MemoryEntry>>>,

    /// Per-agent working memories (in-memory with persistence hint).
    working: RwLock<HashMap<String, Vec<MemoryEntry>>>,

    /// Long-term memories (persisted, vector-indexed).
    long_term: RwLock<HashMap<String, Vec<MemoryEntry>>>,

    /// Procedural memories (persistent, not evicted).
    procedural: RwLock<HashMap<String, Vec<MemoryEntry>>>,

    /// Optional canonical ledger for durable memory.
    ledger: RwLock<Option<Arc<dyn crate::memory::CanonicalLedger + Send + Sync>>>,

    /// Orders snapshot capture and publication for derived projection updates.
    persist_lock: Mutex<()>,

    /// Operation counter for auto-persist triggering.
    op_count: RwLock<u64>,
}

/// Default number of operations between auto-persists.
pub const DEFAULT_PERSIST_OP_COUNT: u64 = 50;

impl LayeredMemory {
    /// Create a new empty memory manager.
    pub fn new() -> Self {
        Self {
            ephemeral: RwLock::new(HashMap::new()),
            working: RwLock::new(HashMap::new()),
            long_term: RwLock::new(HashMap::new()),
            procedural: RwLock::new(HashMap::new()),
            ledger: RwLock::new(None),
            persist_lock: Mutex::new(()),
            op_count: RwLock::new(0),
        }
    }

    /// Attach the sole canonical durability path.
    pub(crate) fn set_ledger(&self, ledger: Arc<dyn crate::memory::CanonicalLedger + Send + Sync>) {
        *self.ledger.write().unwrap() = Some(ledger);
    }

    /// Flush already committed immutable ledger objects and root metadata.
    pub fn flush_ledger(&self) -> Result<bool, crate::memory::LedgerError> {
        let guard = self.ledger.read().unwrap();
        let Some(ledger) = guard.as_ref() else {
            return Ok(false);
        };
        ledger.flush()?;
        Ok(true)
    }

    /// Commit an immutable Working revision before publishing it to readers.
    ///
    /// The lock order is `persist_lock` before the tier lock.
    /// lock. Holding the Working write lock across durable persistence prevents
    /// a concurrent writer from being lost between candidate capture and
    /// publication. A failed persist leaves the live vector untouched.
    fn commit_working_mutation(
        &self,
        caller_role_id: &str,
        entry_id: &str,
        mutate: impl FnOnce(&mut Vec<MemoryEntry>) -> Result<Option<MemoryEntry>, DurableMemoryMutationError>,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        tracing::debug!(
            phase = "validate",
            outcome = "started",
            "validating Working Memory mutation"
        );
        let _commit = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ledger = match self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(DurableMemoryMutationError::PersistenceUnavailable)
        {
            Ok(ledger) => ledger,
            Err(error) => {
                tracing::warn!(
                    phase = "validate",
                    outcome = "error",
                    error_category = error.category(),
                    "Working Memory mutation rejected"
                );
                return Err(error);
            }
        };

        let origin_role = ledger
            .origin_for_revision(caller_role_id, entry_id, true)?
            .ok_or_else(|| DurableMemoryMutationError::NotFound {
                entry_id: entry_id.to_string(),
            })?;

        let mut working = self.working.write().unwrap();
        let live = match working
            .get_mut(&origin_role)
            .ok_or_else(|| DurableMemoryMutationError::NotFound {
                entry_id: entry_id.to_string(),
            }) {
            Ok(live) => live,
            Err(error) => {
                tracing::warn!(
                    phase = "validate",
                    outcome = "error",
                    error_category = error.category(),
                    "Working Memory mutation rejected"
                );
                return Err(error);
            }
        };
        let mut candidate = live.clone();
        match mutate(&mut candidate) {
            Ok(Some(existing)) => return Ok(existing),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    phase = "validate",
                    outcome = "error",
                    error_category = error.category(),
                    "Working Memory mutation rejected"
                );
                return Err(error);
            }
        }
        tracing::debug!(
            phase = "validate",
            outcome = "success",
            "Working Memory mutation validated"
        );
        let committed_entry = candidate.last().filter(|entry| entry.id != entry_id).ok_or(
            DurableMemoryMutationError::InvalidCanonicalContent {
                category: "mutation_did_not_append_revision",
            },
        )?;
        let revision = crate::memory::CanonicalRevision::from_entry(committed_entry)?;
        tracing::debug!(
            phase = "persist_ledger",
            outcome = "started",
            "persisting canonical Working Memory revision"
        );
        let receipt = match ledger.commit_expected(
            caller_role_id,
            MemoryTier::Working,
            crate::memory::ExpectedHead::Revision(entry_id.into()),
            revision,
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                tracing::warn!(
                    phase = "persist_ledger",
                    outcome = "error",
                    error_category = error.category(),
                    "canonical Working Memory revision was not published"
                );
                return Err(DurableMemoryMutationError::Ledger(error));
            }
        };
        if let Some(published) = candidate.last_mut() {
            published.created_at = receipt.committed_at;
            published.last_accessed = receipt.committed_at;
        }
        tracing::debug!(
            phase = "persist",
            outcome = "success",
            "Working Memory candidate persisted"
        );
        let published = candidate
            .last()
            .cloned()
            .ok_or(DurableMemoryMutationError::InvalidCanonicalContent {
                category: "mutation_did_not_append_revision",
            })?;
        *live = candidate;
        tracing::info!(
            phase = "publish",
            outcome = "success",
            "Working Memory mutation published"
        );
        Ok(published)
    }

    /// Append one canonical Working Memory root and publish its runtime
    /// projection only after the ledger returns a durable commit receipt.
    pub(crate) fn create_working_durable(
        &self,
        mut entry: MemoryEntry,
        quota: u64,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        entry
            .seal_root_identity()
            .map_err(|category| DurableMemoryMutationError::InvalidCanonicalContent { category })?;
        let entry_id = entry.id.clone();
        let agent_id = entry.agent_id.clone();
        let span = tracing::info_span!(
            "working_memory_create",
            operation = "memory.create",
            role_kind = role_kind(&agent_id),
            entry_id = %entry_id,
            memory_id = %entry.memory_id,
            hash_verified = true,
        );
        let _guard = span.enter();
        let _commit = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(DurableMemoryMutationError::PersistenceUnavailable)?;
        let non_working_count = [MemoryTier::Ephemeral, MemoryTier::LongTerm, MemoryTier::Procedural]
            .into_iter()
            .map(|tier| self.get_tier(&agent_id, tier).len())
            .sum::<usize>();
        let mut working = self.working.write().unwrap();
        let live = working.entry(agent_id.clone()).or_default();
        let current = non_working_count + live.len();
        if quota > 0 && current as u64 >= quota {
            return Err(DurableMemoryMutationError::QuotaExceeded { current, limit: quota });
        }
        let revision = crate::memory::CanonicalRevision::from_entry(&entry)?;
        tracing::debug!(
            phase = "persist_ledger",
            outcome = "started",
            "persisting Working Memory candidate"
        );
        let receipt = match ledger.commit_expected(
            &agent_id,
            MemoryTier::Working,
            crate::memory::ExpectedHead::Absent,
            revision,
        ) {
            Ok(receipt) => receipt,
            Err(error) => {
                tracing::warn!(
                    phase = "persist_ledger",
                    outcome = "error",
                    error_category = error.category(),
                    "Working Memory candidate was not published"
                );
                return Err(DurableMemoryMutationError::Ledger(error));
            }
        };
        entry.created_at = receipt.committed_at;
        entry.last_accessed = receipt.committed_at;
        live.push(entry.clone());
        tracing::info!(
            entry_id = %entry_id,
            phase = "publish",
            outcome = "success",
            "Working Memory mutation published"
        );
        Ok(entry)
    }

    /// Commit a durable root revision in any persistent tier, then publish its
    /// runtime view. Ephemeral entries are deliberately unsupported here.
    pub(crate) fn create_durable(
        &self,
        mut entry: MemoryEntry,
        quota: u64,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        if entry.tier == MemoryTier::Working {
            return self.create_working_durable(entry, quota);
        }
        if entry.tier == MemoryTier::Ephemeral {
            return Err(DurableMemoryMutationError::InvalidCanonicalContent {
                category: "ephemeral_revision_not_durable",
            });
        }
        entry
            .seal_root_identity()
            .map_err(|category| DurableMemoryMutationError::InvalidCanonicalContent { category })?;
        let _commit = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(DurableMemoryMutationError::PersistenceUnavailable)?;
        let current = self.count_for_agent(&entry.agent_id);
        if quota > 0 && current as u64 >= quota {
            return Err(DurableMemoryMutationError::QuotaExceeded { current, limit: quota });
        }
        let revision = crate::memory::CanonicalRevision::from_entry(&entry)?;
        let receipt = ledger.commit_expected(
            &entry.agent_id,
            entry.tier,
            crate::memory::ExpectedHead::Absent,
            revision,
        )?;
        entry.created_at = receipt.committed_at;
        entry.last_accessed = receipt.committed_at;
        self.store_inner(entry.clone());
        Ok(entry)
    }

    /// Atomically commit and publish a batch of persistent root revisions.
    pub(crate) fn create_batch_durable(
        &self,
        mut entries: Vec<MemoryEntry>,
        quota: u64,
    ) -> Result<Vec<MemoryEntry>, DurableMemoryMutationError> {
        if entries.is_empty() {
            return Ok(entries);
        }
        let agent_id = entries[0].agent_id.clone();
        let tier = entries[0].tier;
        if tier == MemoryTier::Ephemeral {
            return Err(DurableMemoryMutationError::InvalidCanonicalContent {
                category: "ephemeral_revision_not_durable",
            });
        }
        for entry in &mut entries {
            if entry.agent_id != agent_id || entry.tier != tier {
                return Err(DurableMemoryMutationError::InvalidCanonicalContent {
                    category: "invalid_batch_boundary",
                });
            }
            entry
                .seal_root_identity()
                .map_err(|category| DurableMemoryMutationError::InvalidCanonicalContent { category })?;
        }
        let _commit = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let current = self.count_for_agent(&agent_id);
        if quota > 0 && current.saturating_add(entries.len()) as u64 > quota {
            return Err(DurableMemoryMutationError::QuotaExceeded { current, limit: quota });
        }
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(DurableMemoryMutationError::PersistenceUnavailable)?;
        let revisions = entries
            .iter()
            .map(crate::memory::CanonicalRevision::from_entry)
            .collect::<Result<Vec<_>, _>>()?;
        let receipts = ledger.commit_roots(&agent_id, tier, revisions)?;
        for (entry, receipt) in entries.iter_mut().zip(receipts) {
            entry.created_at = receipt.committed_at;
            entry.last_accessed = receipt.committed_at;
        }
        let tier_map = match tier {
            MemoryTier::Working => &self.working,
            MemoryTier::LongTerm => &self.long_term,
            MemoryTier::Procedural => &self.procedural,
            MemoryTier::Ephemeral => {
                return Err(DurableMemoryMutationError::InvalidCanonicalContent {
                    category: "ephemeral_revision_not_durable",
                })
            }
        };
        tier_map
            .write()
            .unwrap()
            .entry(agent_id)
            .or_default()
            .extend(entries.iter().cloned());
        Ok(entries)
    }

    /// Append a validated Working Memory batch as one durable snapshot.
    pub(crate) fn create_working_batch_durable(
        &self,
        mut entries: Vec<MemoryEntry>,
        quota: u64,
    ) -> Result<Vec<MemoryEntry>, DurableMemoryMutationError> {
        if entries.is_empty() {
            return Ok(entries);
        }
        let agent_id = entries[0].agent_id.clone();
        for entry in &mut entries {
            if entry.agent_id != agent_id || entry.tier != MemoryTier::Working {
                return Err(DurableMemoryMutationError::InvalidCanonicalContent {
                    category: "invalid_batch_identity",
                });
            }
            entry
                .seal_root_identity()
                .map_err(|category| DurableMemoryMutationError::InvalidCanonicalContent { category })?;
        }
        let span = tracing::info_span!(
            "working_memory_batch_create",
            operation = "memory.batch_create",
            role_kind = role_kind(&agent_id),
            revision_count = entries.len(),
        );
        let _guard = span.enter();
        let _commit = self
            .persist_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(DurableMemoryMutationError::PersistenceUnavailable)?;
        let non_working_count = [MemoryTier::Ephemeral, MemoryTier::LongTerm, MemoryTier::Procedural]
            .into_iter()
            .map(|tier| self.get_tier(&agent_id, tier).len())
            .sum::<usize>();
        let mut working = self.working.write().unwrap();
        let live = working.entry(agent_id.clone()).or_default();
        let current = non_working_count + live.len();
        if quota > 0 && current.saturating_add(entries.len()) as u64 > quota {
            return Err(DurableMemoryMutationError::QuotaExceeded { current, limit: quota });
        }
        let revisions = entries
            .iter()
            .map(crate::memory::CanonicalRevision::from_entry)
            .collect::<Result<Vec<_>, _>>()?;
        tracing::debug!(
            phase = "persist_ledger",
            outcome = "started",
            "persisting Working Memory batch"
        );
        let receipts = ledger.commit_roots(&agent_id, MemoryTier::Working, revisions)?;
        for (entry, receipt) in entries.iter_mut().zip(receipts) {
            entry.created_at = receipt.committed_at;
            entry.last_accessed = receipt.committed_at;
        }
        live.extend(entries.iter().cloned());
        tracing::info!(
            phase = "publish",
            outcome = "success",
            revision_count = entries.len(),
            "Working Memory batch published"
        );
        Ok(entries)
    }

    /// Append a new active Working revision and supersede the previous one in
    /// the same durable snapshot.
    pub(crate) fn update_working_durable(
        &self,
        agent_id: &str,
        namespace: &str,
        entry_id: &str,
        new_content: String,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        let span = tracing::info_span!(
            "working_memory_update",
            operation = "memory.update",
            role_kind = role_kind(agent_id),
            entry_id = %entry_id,
            previous_revision_id = %entry_id,
            new_revision_id = tracing::field::Empty,
        );
        let _guard = span.enter();
        if new_content.trim().is_empty() {
            tracing::warn!(
                phase = "validate",
                outcome = "error",
                error_category = "empty_content",
                "Working Memory mutation rejected"
            );
            return Err(DurableMemoryMutationError::EmptyContent);
        }
        let new_revision_id = MemoryRevisionId(Uuid::new_v4().to_string());
        span.record("new_revision_id", tracing::field::display(&new_revision_id));
        self.commit_working_mutation(agent_id, entry_id, |candidate| {
            let position = candidate.iter().position(|entry| entry.id == entry_id).ok_or_else(|| {
                DurableMemoryMutationError::NotFound {
                    entry_id: entry_id.to_string(),
                }
            })?;
            let previous = &candidate[position];
            if previous.tenant_id != namespace {
                return Err(DurableMemoryMutationError::NamespaceMismatch {
                    entry_id: entry_id.to_string(),
                });
            }
            if previous.deleted_at.is_some() || previous.superseded_by.is_some() {
                return Err(DurableMemoryMutationError::Inactive {
                    entry_id: entry_id.to_string(),
                });
            }

            let now = now_ms();
            let new_entry = MemoryEntry {
                id: new_revision_id.to_string(),
                memory_id: previous.memory_id.clone(),
                parent_revision_id: Some(previous.id.as_str().into()),
                canonical_content_hash: MemoryContent::Text(new_content.clone())
                    .canonical_content_hash()
                    .map_err(|category| DurableMemoryMutationError::InvalidCanonicalContent { category })?,
                agent_id: previous.agent_id.clone(),
                tenant_id: previous.tenant_id.clone(),
                tier: MemoryTier::Working,
                content: MemoryContent::Text(new_content),
                importance: previous.importance,
                access_count: 0,
                last_accessed: now,
                created_at: now,
                tags: previous.tags.clone(),
                ttl_ms: previous.ttl_ms,
                original_ttl_ms: previous.original_ttl_ms,
                scope: previous.scope.clone(),
                memory_type: previous.memory_type,
                causal_parent: previous.causal_parent.clone(),
                supersedes: None,
                superseded_by: None,
                deleted_at: None,
            };
            candidate.push(new_entry);
            Ok(None)
        })
    }

    /// Soft-delete one active Working entry and durably publish the deletion
    /// before it becomes visible to readers.
    pub(crate) fn delete_working_durable(
        &self,
        agent_id: &str,
        namespace: &str,
        entry_id: &str,
    ) -> Result<MemoryEntry, DurableMemoryMutationError> {
        let span = tracing::info_span!(
            "working_memory_delete",
            operation = "memory.delete",
            role_kind = role_kind(agent_id),
            entry_id = %entry_id,
            previous_revision_id = %entry_id,
            new_revision_id = tracing::field::Empty,
        );
        let _guard = span.enter();
        let new_revision_id = MemoryRevisionId(Uuid::new_v4().to_string());
        span.record("new_revision_id", tracing::field::display(&new_revision_id));
        self.commit_working_mutation(agent_id, entry_id, |candidate| {
            let entry = candidate.iter().find(|entry| entry.id == entry_id).ok_or_else(|| {
                DurableMemoryMutationError::NotFound {
                    entry_id: entry_id.to_string(),
                }
            })?;
            if entry.tenant_id != namespace {
                return Err(DurableMemoryMutationError::NamespaceMismatch {
                    entry_id: entry_id.to_string(),
                });
            }
            if entry.deleted_at.is_some() {
                return Err(DurableMemoryMutationError::Ledger(
                    crate::memory::LedgerError::HeadConflict {
                        memory_id: entry.memory_id.clone(),
                        expected: crate::memory::ExpectedHead::Revision(entry_id.into()),
                        actual: Some(entry.id.as_str().into()),
                    },
                ));
            }
            if let Some(child) = candidate.iter().find(|candidate| {
                candidate
                    .parent_revision_id
                    .as_ref()
                    .is_some_and(|parent| parent.as_str() == entry_id)
            }) {
                if child.deleted_at.is_some() {
                    return Ok(Some(child.clone()));
                }
                return Err(DurableMemoryMutationError::Ledger(
                    crate::memory::LedgerError::HeadConflict {
                        memory_id: entry.memory_id.clone(),
                        expected: crate::memory::ExpectedHead::Revision(entry_id.into()),
                        actual: Some(child.id.as_str().into()),
                    },
                ));
            }
            let mut tombstone = entry.clone();
            tombstone.id = new_revision_id.to_string();
            tombstone.parent_revision_id = Some(entry.id.as_str().into());
            tombstone.access_count = 0;
            tombstone.last_accessed = now_ms();
            tombstone.created_at = tombstone.last_accessed;
            tombstone.deleted_at = Some(tombstone.last_accessed);
            tombstone.superseded_by = None;
            candidate.push(tombstone);
            Ok(None)
        })
    }

    /// Rebuild one role's runtime view from the canonical ledger.
    /// Called on kernel startup.
    pub fn restore_agent(&self, agent_id: &str) -> Result<(), crate::memory::LedgerError> {
        let ledger = {
            let guard = self.ledger.read().unwrap();
            match guard.as_ref() {
                Some(p) => Arc::clone(p),
                None => return Ok(()),
            }
        };

        let restored = ledger.rebuild_origin_role(agent_id)?;

        let mut restored_by_tier = HashMap::new();
        for (tier, records) in restored {
            let count = records.len();
            let entries = records
                .into_iter()
                .map(|record| record.into_runtime_entry(agent_id))
                .collect::<Vec<_>>();
            restored_by_tier.insert(tier, entries);
            tracing::debug!(
                role_kind = role_kind(agent_id),
                tier = %tier.name(),
                count = count,
                "Restored memory tier from CAS",
            );
        }
        for (tier, lock) in [
            (MemoryTier::Working, &self.working),
            (MemoryTier::LongTerm, &self.long_term),
            (MemoryTier::Procedural, &self.procedural),
        ] {
            let mut map = lock.write().unwrap();
            match restored_by_tier.remove(&tier) {
                Some(entries) => {
                    map.insert(agent_id.to_string(), entries);
                }
                None => {
                    map.remove(agent_id);
                }
            }
        }

        Ok(())
    }

    /// Increment operation counter and trigger persist if threshold reached.
    /// Returns true if a persist was triggered.
    pub fn tick(&self) -> bool {
        let threshold = {
            let mut cnt = self.op_count.write().unwrap();
            *cnt += 1;
            *cnt >= DEFAULT_PERSIST_OP_COUNT
        };

        if threshold {
            *self.op_count.write().unwrap() = 0;
            if let Err(error) = self.flush_ledger() {
                tracing::warn!(
                    phase = "flush_ledger",
                    outcome = "error",
                    error_category = error.category(),
                    "canonical ledger flush failed"
                );
            }
            true
        } else {
            false
        }
    }

    /// Store a memory entry in the appropriate tier.
    pub fn store(&self, mut entry: MemoryEntry) -> Result<(), MemoryError> {
        entry
            .seal_root_identity()
            .map_err(|category| MemoryError::InvalidCanonicalContent { category })?;
        if entry.tier != MemoryTier::Ephemeral {
            return Err(crate::memory::LedgerError::Invalid {
                category: "durable_entry_requires_create_durable",
            }
            .into());
        }
        self.tick();
        self.store_inner(entry);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn store_test_entry(&self, mut entry: MemoryEntry) {
        entry
            .seal_root_identity()
            .expect("test memory entry must be canonicalizable");
        self.store_inner(entry);
    }

    /// Store with quota enforcement. Returns Err if the agent's total memory
    /// entry count would exceed `quota`. `quota == 0` means unlimited.
    pub fn store_checked(&self, entry: MemoryEntry, quota: u64) -> Result<(), MemoryError> {
        if quota > 0 {
            let current = self.count_for_agent(&entry.agent_id);
            if current as u64 >= quota {
                return Err(MemoryError::QuotaExceeded {
                    agent_id: entry.agent_id.clone(),
                    current,
                    limit: quota,
                });
            }
        }
        self.store(entry)
    }

    /// Count total memory entries across all tiers for an agent.
    pub fn count_for_agent(&self, agent_id: &str) -> usize {
        let mut count = 0;
        if let Ok(map) = self.ephemeral.read() {
            count += map.get(agent_id).map(|v| v.len()).unwrap_or(0);
        }
        if let Ok(map) = self.working.read() {
            count += map.get(agent_id).map(|v| v.len()).unwrap_or(0);
        }
        if let Ok(map) = self.long_term.read() {
            count += map.get(agent_id).map(|v| v.len()).unwrap_or(0);
        }
        if let Ok(map) = self.procedural.read() {
            count += map.get(agent_id).map(|v| v.len()).unwrap_or(0);
        }
        count
    }

    fn store_inner(&self, entry: MemoryEntry) {
        match entry.tier {
            MemoryTier::Ephemeral => {
                let mut map = self.ephemeral.write().unwrap();
                map.entry(entry.agent_id.clone()).or_default().push(entry);
            }
            MemoryTier::Working => {
                let mut map = self.working.write().unwrap();
                map.entry(entry.agent_id.clone()).or_default().push(entry);
            }
            MemoryTier::LongTerm => {
                let mut map = self.long_term.write().unwrap();
                map.entry(entry.agent_id.clone()).or_default().push(entry);
            }
            MemoryTier::Procedural => {
                let mut map = self.procedural.write().unwrap();
                map.entry(entry.agent_id.clone()).or_default().push(entry);
            }
        }
    }

    /// Retrieve memory entries for an agent from a specific tier.
    pub fn get_tier(&self, agent_id: &str, tier: MemoryTier) -> Vec<MemoryEntry> {
        let map = match tier {
            MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
            MemoryTier::Working => self.working.read().unwrap(),
            MemoryTier::LongTerm => self.long_term.read().unwrap(),
            MemoryTier::Procedural => self.procedural.read().unwrap(),
        };

        map.get(agent_id).cloned().unwrap_or_default()
    }

    /// Retrieve all memory for an agent across all tiers.
    pub fn get_all(&self, agent_id: &str) -> Vec<MemoryEntry> {
        let mut all = Vec::new();
        for tier in [
            MemoryTier::Ephemeral,
            MemoryTier::Working,
            MemoryTier::LongTerm,
            MemoryTier::Procedural,
        ] {
            all.extend(self.get_tier(agent_id, tier));
        }
        all
    }

    fn active_revisions(entries: Vec<MemoryEntry>) -> Vec<MemoryEntry> {
        let parent_ids: std::collections::HashSet<String> = entries
            .iter()
            .filter_map(|entry| entry.parent_revision_id.as_ref().map(ToString::to_string))
            .collect();
        entries
            .into_iter()
            .filter(|entry| {
                !parent_ids.contains(&entry.id) && entry.deleted_at.is_none() && entry.superseded_by.is_none()
            })
            .collect()
    }

    /// Retrieve only active heads from one memory tier.
    pub fn get_active_tier(&self, agent_id: &str, tier: MemoryTier) -> Vec<MemoryEntry> {
        Self::active_revisions(self.get_tier(agent_id, tier))
    }

    /// Resolve a revision only when it is the current active head.
    pub fn find_active_entry(&self, agent_id: &str, tier: MemoryTier, revision_id: &str) -> Option<MemoryEntry> {
        self.get_active_tier(agent_id, tier)
            .into_iter()
            .find(|entry| entry.id == revision_id)
    }

    pub(crate) fn find_active_authorized(
        &self,
        role_id: &str,
        tier: MemoryTier,
        revision_id: &str,
    ) -> Result<Option<MemoryEntry>, crate::memory::LedgerError> {
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(crate::memory::LedgerError::Invalid {
                category: "canonical_ledger_unavailable",
            })?;
        let Some(origin) = ledger.origin_for_revision(role_id, revision_id, false)? else {
            return Ok(None);
        };
        Ok(self.find_active_entry(&origin, tier, revision_id))
    }

    /// Clear and return all runtime-only ephemeral memories for an agent.
    ///
    /// Cognitive-tier changes require a canonical revision commit; eviction
    /// therefore never turns an ephemeral entry into durable Working Memory.
    pub fn evict_ephemeral(&self, agent_id: &str) -> Vec<MemoryEntry> {
        self.ephemeral.write().unwrap().remove(agent_id).unwrap_or_default()
    }

    /// Tag-based retrieval from a specific tier.
    pub fn get_by_tags(&self, agent_id: &str, tier: MemoryTier, tags: &[String]) -> Vec<MemoryEntry> {
        self.get_tier(agent_id, tier)
            .into_iter()
            .filter(|e| tags.iter().any(|t| e.tags.contains(t)))
            .collect()
    }

    /// Retrieve all memories with access tracking.
    ///
    /// Unlike `get_all()`, this updates the runtime-only `access_count` and
    /// `last_accessed` projections on every returned entry. It never changes
    /// a canonical cognitive tier.
    pub fn recall_with_tracking(&self, agent_id: &str) -> Vec<MemoryEntry> {
        let mut all = Vec::new();

        for tier in [
            MemoryTier::Ephemeral,
            MemoryTier::Working,
            MemoryTier::LongTerm,
            MemoryTier::Procedural,
        ] {
            let map = match tier {
                MemoryTier::Ephemeral => &self.ephemeral,
                MemoryTier::Working => &self.working,
                MemoryTier::LongTerm => &self.long_term,
                MemoryTier::Procedural => &self.procedural,
            };
            if let Some(entries) = map.write().unwrap().get_mut(agent_id) {
                for entry in entries.iter_mut() {
                    entry.on_memory_access();
                }
                all.extend(entries.iter().cloned());
            }
        }

        all
    }

    /// Retrieve the most relevant memories within a token budget.
    ///
    /// Uses relevance scoring (recency × frequency × importance) to rank
    /// all memories, then greedily selects entries fitting the budget.
    pub fn recall_relevant(&self, agent_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        let now = now_ms();
        let all = self.recall_with_tracking(agent_id);
        let selected = crate::memory::relevance::select_within_budget(&all, budget_tokens, now);
        selected.into_iter().map(|(entry, _score)| entry).collect()
    }

    /// Retrieve all active (non-deleted, non-superseded) entries for an agent.
    ///
    /// Filters out entries where `deleted_at` is set or `superseded_by` is set.
    pub fn get_active(&self, agent_id: &str) -> Vec<MemoryEntry> {
        Self::active_revisions(self.get_all(agent_id))
    }

    /// Remove ephemeral (L0) memory entries for an agent.
    /// Durable working/long-term/procedural revisions are unaffected.
    pub fn clear_ephemeral(&self, agent_id: &str) -> usize {
        let mut map = self.ephemeral.write().unwrap();
        if let Some(entries) = map.remove(agent_id) {
            entries.len()
        } else {
            0
        }
    }

    // ─── Cognitive Memory Type Retrieval ───────────────────────────

    /// Filter entries by cognitive memory type from a specific tier.
    pub fn get_by_type(&self, agent_id: &str, tier: MemoryTier, memory_type: MemoryType) -> Vec<MemoryEntry> {
        self.get_tier(agent_id, tier)
            .into_iter()
            .filter(|e| e.memory_type == memory_type)
            .collect()
    }

    // ─── Storage Governance (F-18) ─────────────────────────────────

    /// Check if a CID is referenced by any memory entry.
    /// Returns true if any memory entry has an ObjectRef content referencing this CID.
    pub fn is_cid_referenced(&self, cid: &str) -> bool {
        // Check Ephemeral tier
        {
            let map = self.ephemeral.read().unwrap();
            for entries in map.values() {
                for entry in entries {
                    if let MemoryContent::ObjectRef(ref entry_cid) = entry.content {
                        if entry_cid == cid {
                            return true;
                        }
                    }
                }
            }
        }
        // Check Working tier
        {
            let map = self.working.read().unwrap();
            for entries in map.values() {
                for entry in entries {
                    if let MemoryContent::ObjectRef(ref entry_cid) = entry.content {
                        if entry_cid == cid {
                            return true;
                        }
                    }
                }
            }
        }
        // Check LongTerm tier
        {
            let map = self.long_term.read().unwrap();
            for entries in map.values() {
                for entry in entries {
                    if let MemoryContent::ObjectRef(ref entry_cid) = entry.content {
                        if entry_cid == cid {
                            return true;
                        }
                    }
                }
            }
        }
        // Check Procedural tier
        {
            let map = self.procedural.read().unwrap();
            for entries in map.values() {
                for entry in entries {
                    if let MemoryContent::ObjectRef(ref entry_cid) = entry.content {
                        if entry_cid == cid {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }

    /// Lexical retrieval over the memory domain.
    ///
    /// This deliberately does not use SemanticFS BM25 because that index is
    /// keyed by CAS CID while memory candidates are keyed by `MemoryEntry::id`.
    /// It provides immediate read-after-write retrieval while vector indexing
    /// is eventually consistent.
    pub fn recall_lexical(&self, agent_id: &str, tenant_id: &str, query: &str, k: usize) -> Vec<(MemoryEntry, f32)> {
        if k == 0 {
            return Vec::new();
        }

        let query_lower = query.to_lowercase();
        let query_terms = lexical_terms(&query_lower);
        let mut scored = Vec::new();

        for entry in self.get_active(agent_id) {
            if entry.tenant_id != tenant_id {
                continue;
            }
            let MemoryContent::Text(text) = &entry.content else {
                continue;
            };
            let text_lower = text.to_lowercase();
            let text_terms = lexical_terms(&text_lower);
            let tag_text = entry.tags.join(" ").to_lowercase();
            let tag_terms = lexical_terms(&tag_text);

            let overlap = query_terms
                .iter()
                .filter(|term| text_terms.contains(*term) || tag_terms.contains(*term))
                .count() as f32;
            let phrase_bonus = if !query_lower.is_empty() && text_lower.contains(&query_lower) {
                2.0
            } else {
                0.0
            };
            let term_score = if query_terms.is_empty() {
                0.0
            } else {
                overlap / query_terms.len() as f32
            };
            let score = (term_score + phrase_bonus * 0.25).min(1.0);
            if score > 0.0 {
                scored.push((entry, score));
            }
        }

        scored.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.0.created_at.cmp(&left.0.created_at))
        });
        scored.truncate(k);
        scored
    }

    pub(crate) fn recall_working_lexical_authorized(
        &self,
        role_id: &str,
        namespace: &str,
        query: &str,
        k: usize,
    ) -> Result<Vec<(MemoryEntry, f32)>, crate::memory::LedgerError> {
        let ledger = self
            .ledger
            .read()
            .unwrap()
            .as_ref()
            .cloned()
            .ok_or(crate::memory::LedgerError::Invalid {
                category: "canonical_ledger_unavailable",
            })?;
        let readable_ids: std::collections::HashSet<String> = ledger
            .readable_active_revision_ids(role_id)?
            .into_iter()
            .map(|revision_id| revision_id.to_string())
            .collect();
        let query_lower = query.to_lowercase();
        let query_terms = lexical_terms(&query_lower);
        let mut hits = Vec::new();
        let working = self.working.read().unwrap();
        for entry in working
            .values()
            .flat_map(|entries| Self::active_revisions(entries.clone()))
            .filter(|entry| readable_ids.contains(&entry.id) && entry.tenant_id == namespace)
        {
            let MemoryContent::Text(text) = &entry.content else {
                continue;
            };
            let text_lower = text.to_lowercase();
            let text_terms = lexical_terms(&text_lower);
            let tag_terms = lexical_terms(&entry.tags.join(" ").to_lowercase());
            let overlap = query_terms
                .iter()
                .filter(|term| text_terms.contains(*term) || tag_terms.contains(*term))
                .count() as f32;
            let phrase_bonus = if !query_lower.is_empty() && text_lower.contains(&query_lower) {
                2.0
            } else {
                0.0
            };
            let term_score = if query_terms.is_empty() {
                0.0
            } else {
                overlap / query_terms.len() as f32
            };
            let score = (term_score + phrase_bonus * 0.25).min(1.0);
            if score > 0.0 {
                hits.push((entry, score));
            }
        }
        hits.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| right.0.created_at.cmp(&left.0.created_at))
                .then_with(|| left.0.id.cmp(&right.0.id))
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// Find a memory entry by ID.
    pub fn find_entry(&self, agent_id: &str, entry_id: &str) -> Option<MemoryEntry> {
        for tier_lock in [&self.working, &self.long_term, &self.procedural] {
            let map = tier_lock.read().unwrap();
            if let Some(entries) = map.get(agent_id) {
                if let Some(entry) = entries.iter().find(|e| e.id == entry_id) {
                    return Some(entry.clone());
                }
            }
        }
        None
    }

    /// Touch a memory entry: increment access_count and update last_accessed.
    /// Returns true if the entry was found and updated.
    pub fn touch_entry(&self, agent_id: &str, entry_id: &str) -> bool {
        let now = now_ms();
        for tier_lock in [&self.ephemeral, &self.working, &self.long_term, &self.procedural] {
            let mut map = tier_lock.write().unwrap();
            if let Some(entries) = map.get_mut(agent_id) {
                if let Some(entry) = entries.iter_mut().find(|e| e.id == entry_id) {
                    entry.access_count += 1;
                    entry.last_accessed = now;
                    return true;
                }
            }
        }
        false
    }

    /// Get memory statistics for observability (F-17/F-18).
    /// Returns counts per tier and aggregate stats.
    pub fn get_stats(&self) -> MemoryStats {
        let now = now_ms();
        let mut total_entries = 0;
        let mut total_bytes = 0;
        let mut never_accessed_count = 0;
        let mut about_to_expire_count = 0;
        let mut oldest_entry_age_ms: u64 = 0;
        let mut total_access_count = 0u64;

        let ephemeral_entries: usize;
        let working_entries: usize;
        let longterm_entries: usize;

        // Process Ephemeral tier
        {
            let map = self.ephemeral.read().unwrap();
            let entries: Vec<_> = map.values().flat_map(|v| v.iter()).collect();
            ephemeral_entries = entries.len();
            total_entries += ephemeral_entries;
            for entry in entries {
                let entry_bytes = entry.content.display().len();
                total_bytes += entry_bytes;
                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }
                if let Some(ttl_ms) = entry.ttl_ms {
                    if let Some(original) = entry.original_ttl_ms {
                        let elapsed = now.saturating_sub(entry.created_at);
                        let remaining = ttl_ms.saturating_sub(elapsed);
                        let ten_percent = original / 10;
                        if remaining <= ten_percent {
                            about_to_expire_count += 1;
                        }
                    }
                }
                let age = now.saturating_sub(entry.created_at);
                if age > oldest_entry_age_ms {
                    oldest_entry_age_ms = age;
                }
            }
        }

        // Process Working tier
        {
            let map = self.working.read().unwrap();
            let entries: Vec<_> = map.values().flat_map(|v| v.iter()).collect();
            working_entries = entries.len();
            total_entries += working_entries;
            for entry in entries {
                let entry_bytes = entry.content.display().len();
                total_bytes += entry_bytes;
                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }
                if let Some(ttl_ms) = entry.ttl_ms {
                    if let Some(original) = entry.original_ttl_ms {
                        let elapsed = now.saturating_sub(entry.created_at);
                        let remaining = ttl_ms.saturating_sub(elapsed);
                        let ten_percent = original / 10;
                        if remaining <= ten_percent {
                            about_to_expire_count += 1;
                        }
                    }
                }
                let age = now.saturating_sub(entry.created_at);
                if age > oldest_entry_age_ms {
                    oldest_entry_age_ms = age;
                }
            }
        }

        // Process LongTerm tier
        {
            let map = self.long_term.read().unwrap();
            let entries: Vec<_> = map.values().flat_map(|v| v.iter()).collect();
            longterm_entries = entries.len();
            total_entries += longterm_entries;
            for entry in entries {
                let entry_bytes = entry.content.display().len();
                total_bytes += entry_bytes;
                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }
                if let Some(ttl_ms) = entry.ttl_ms {
                    if let Some(original) = entry.original_ttl_ms {
                        let elapsed = now.saturating_sub(entry.created_at);
                        let remaining = ttl_ms.saturating_sub(elapsed);
                        let ten_percent = original / 10;
                        if remaining <= ten_percent {
                            about_to_expire_count += 1;
                        }
                    }
                }
                let age = now.saturating_sub(entry.created_at);
                if age > oldest_entry_age_ms {
                    oldest_entry_age_ms = age;
                }
            }
        }

        // Process Procedural tier (only used for never_accessed_count/about_to_expire_count)
        {
            let map = self.procedural.read().unwrap();
            let entries: Vec<_> = map.values().flat_map(|v| v.iter()).collect();
            for entry in entries {
                total_access_count += entry.access_count as u64;
                if entry.access_count == 0 {
                    never_accessed_count += 1;
                }
                if let Some(ttl_ms) = entry.ttl_ms {
                    if let Some(original) = entry.original_ttl_ms {
                        let elapsed = now.saturating_sub(entry.created_at);
                        let remaining = ttl_ms.saturating_sub(elapsed);
                        let ten_percent = original / 10;
                        if remaining <= ten_percent {
                            about_to_expire_count += 1;
                        }
                    }
                }
                let age = now.saturating_sub(entry.created_at);
                if age > oldest_entry_age_ms {
                    oldest_entry_age_ms = age;
                }
            }
        }

        MemoryStats {
            total_entries,
            total_bytes,
            oldest_entry_age_ms,
            avg_access_count: if total_entries > 0 {
                total_access_count as f32 / total_entries as f32
            } else {
                0.0
            },
            never_accessed_count,
            about_to_expire_count,
            ephemeral_entries,
            working_entries,
            longterm_entries,
        }
    }
}

fn lexical_terms(text: &str) -> std::collections::HashSet<String> {
    let mut terms = std::collections::HashSet::new();
    let mut word = String::new();
    let mut cjk = Vec::new();

    let flush_word = |word: &mut String, terms: &mut std::collections::HashSet<String>| {
        if !word.is_empty() {
            terms.insert(std::mem::take(word));
        }
    };
    let flush_cjk = |cjk: &mut Vec<char>, terms: &mut std::collections::HashSet<String>| {
        match cjk.as_slice() {
            [] => {}
            [character] => {
                terms.insert(character.to_string());
            }
            characters => {
                terms.extend(characters.windows(2).map(|pair| pair.iter().collect::<String>()));
            }
        }
        cjk.clear();
    };

    for character in text.chars() {
        if is_cjk(character) {
            flush_word(&mut word, &mut terms);
            cjk.push(character);
        } else if character.is_alphanumeric() {
            flush_cjk(&mut cjk, &mut terms);
            word.push(character);
        } else {
            flush_word(&mut word, &mut terms);
            flush_cjk(&mut cjk, &mut terms);
        }
    }
    flush_word(&mut word, &mut terms);
    flush_cjk(&mut cjk, &mut terms);
    terms
}

fn is_cjk(character: char) -> bool {
    matches!(
        character,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
    )
}

fn role_kind(role_id: &str) -> &'static str {
    if role_id == crate::PERSONAL_OWNER_ROLE_ID {
        "personal_owner"
    } else {
        "authenticated_role"
    }
}

/// Memory statistics for observability (F-17/F-18).
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_entries: usize,
    pub total_bytes: usize,
    pub oldest_entry_age_ms: u64,
    pub avg_access_count: f32,
    pub never_accessed_count: usize,
    pub about_to_expire_count: usize,
    pub ephemeral_entries: usize,
    pub working_entries: usize,
    pub longterm_entries: usize,
}

impl Default for LayeredMemory {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) use crate::util::now_ms;
