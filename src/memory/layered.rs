//! Layered Memory Implementation
//!
//! Implements the 4-tier memory hierarchy. Each tier has different
//! characteristics for capacity, latency, and persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

/// Memory tier classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
pub struct MemoryEntry {
    /// Unique ID for this memory entry.
    pub id: String,

    /// Which agent owns this memory.
    pub agent_id: String,

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

    /// Embedding vector for semantic search (L1+ tiers only).
    pub embedding: Option<Vec<f32>>,
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
    /// Create a new ephemeral memory entry.
    pub fn ephemeral(agent_id: impl Into<String>, content: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            tier: MemoryTier::Ephemeral,
            content: MemoryContent::Text(content.into()),
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            embedding: None,
        }
    }

    /// Create a new long-term memory entry.
    pub fn long_term(
        agent_id: impl Into<String>,
        content: MemoryContent,
        tags: Vec<String>,
    ) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            tier: MemoryTier::LongTerm,
            content,
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags,
            embedding: None,
        }
    }

    /// Record an access to this entry.
    pub fn access(&mut self) {
        self.access_count += 1;
        self.last_accessed = now_ms();
    }
}

/// Global memory manager — holds all agents' memory tiers.
///
/// Can optionally be paired with a [`MemoryPersister`] for L1/L2 persistence
/// across restarts.
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("Entry not found: id={0}")]
    NotFound(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Tier capacity exceeded: tier={tier}, agent={agent}")]
    TierCapacityExceeded { tier: MemoryTier, agent: String },
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

    /// Optional persister for L1/L2 durability.
    persister: RwLock<Option<Arc<dyn crate::memory::persist::MemoryPersister + Send + Sync>>>,

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
            persister: RwLock::new(None),
            op_count: RwLock::new(0),
        }
    }

    /// Attach a persister for L1/L2 durability.
    pub fn set_persister(&self, p: Arc<dyn crate::memory::persist::MemoryPersister + Send + Sync>) {
        *self.persister.write().unwrap() = Some(p);
    }

    /// Persist all Working, LongTerm, and Procedural memories to CAS.
    /// Returns the number of agents persisted.
    pub fn persist_all(&self) -> usize {
        let persister = {
            let guard = self.persister.read().unwrap();
            match guard.as_ref() {
                Some(p) => Arc::clone(p),
                None => return 0,
            }
        };

        let agent_ids: Vec<String> = {
            let w = self.working.read().unwrap();
            let l = self.long_term.read().unwrap();
            let pr = self.procedural.read().unwrap();
            let mut ids: std::collections::HashSet<_> = w.keys().cloned().collect();
            ids.extend(l.keys().cloned());
            ids.extend(pr.keys().cloned());
            ids.into_iter().collect()
        };

        let mut persisted = 0;
        for agent_id in agent_ids {
            for tier in [MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
                let entries = self.get_tier(&agent_id, tier);
                if !entries.is_empty() {
                    if persister.persist(&agent_id, tier, &entries).is_ok() {
                        persisted += 1;
                    }
                }
            }
        }
        persisted
    }

    /// Restore memories for a specific agent from the persister.
    /// Called on kernel startup.
    pub fn restore_agent(&self, agent_id: &str) -> Result<(), crate::memory::persist::PersistError> {
        let persister = {
            let guard = self.persister.read().unwrap();
            match guard.as_ref() {
                Some(p) => Arc::clone(p),
                None => return Ok(()),
            }
        };

        let loader = crate::memory::MemoryLoader::new(persister);
        let restored = loader.restore_agent(agent_id)?;

        for (tier, entries) in restored {
            let count = entries.len();
            self.store_batch(entries);
            tracing::debug!(
                agent_id = %agent_id,
                tier = %tier.name(),
                count = count,
                "Restored memory tier from CAS",
            );
        }

        Ok(())
    }

    /// Restore all persisted agents.
    pub fn restore_all(&self) -> usize {
        let persister = {
            let guard = self.persister.read().unwrap();
            match guard.as_ref() {
                Some(p) => Arc::clone(p),
                None => return 0,
            }
        };

        let agent_ids = persister.list_all_agent_ids();
        let mut restored = 0;
        for agent_id in agent_ids {
            if self.restore_agent(&agent_id).is_ok() {
                restored += 1;
            }
        }
        restored
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
            self.persist_all();
            true
        } else {
            false
        }
    }

    /// Store a memory entry in the appropriate tier.
    pub fn store(&self, entry: MemoryEntry) {
        self.tick();
        self.store_inner(entry);
    }

    fn store_inner(&self, entry: MemoryEntry) {
        match entry.tier {
            MemoryTier::Ephemeral => {
                let mut map = self.ephemeral.write().unwrap();
                map.entry(entry.agent_id.clone())
                    .or_default()
                    .push(entry);
            }
            MemoryTier::Working => {
                let mut map = self.working.write().unwrap();
                map.entry(entry.agent_id.clone())
                    .or_default()
                    .push(entry);
            }
            MemoryTier::LongTerm => {
                let mut map = self.long_term.write().unwrap();
                map.entry(entry.agent_id.clone())
                    .or_default()
                    .push(entry);
            }
            MemoryTier::Procedural => {
                let mut map = self.procedural.write().unwrap();
                map.entry(entry.agent_id.clone())
                    .or_default()
                    .push(entry);
            }
        }
    }

    /// Store multiple entries at once (without triggering auto-persist).
    fn store_batch(&self, entries: Vec<MemoryEntry>) {
        for entry in entries {
            self.store_inner(entry);
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

        map.get(agent_id)
            .cloned()
            .unwrap_or_default()
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

    /// Evict ephemeral memories for an agent (called on context window overflow).
    ///
    /// Entries with `importance >= 70` are promoted to Working Memory (L1).
    /// Entries with lower importance are discarded and returned.
    ///
    /// Returns the list of **discarded** entries (importance < 70).
    pub fn evict_ephemeral(&self, agent_id: &str) -> Vec<MemoryEntry> {
        let entries = {
            let mut map = self.ephemeral.write().unwrap();
            map.remove(agent_id).unwrap_or_default()
        };

        let (to_promote, discarded): (Vec<_>, Vec<_>) = entries
            .into_iter()
            .partition(|e| e.importance >= 70);

        // Promote important entries to Working Memory (L1)
        if !to_promote.is_empty() {
            let mut working = self.working.write().unwrap();
            let promoted: Vec<_> = to_promote
                .into_iter()
                .map(|mut e| {
                    e.tier = MemoryTier::Working;
                    e
                })
                .collect();
            working
                .entry(agent_id.to_string())
                .or_default()
                .extend(promoted);
        }

        // Return discarded entries (not promoted)
        discarded
    }

    /// Tag-based retrieval from a specific tier.
    pub fn get_by_tags(
        &self,
        agent_id: &str,
        tier: MemoryTier,
        tags: &[String],
    ) -> Vec<MemoryEntry> {
        self.get_tier(agent_id, tier)
            .into_iter()
            .filter(|e| tags.iter().any(|t| e.tags.contains(t)))
            .collect()
    }
}

impl Default for LayeredMemory {
    fn default() -> Self {
        Self::new()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

