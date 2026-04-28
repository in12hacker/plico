//! Layered Memory Implementation
//!
//! Implements the 4-tier memory hierarchy. Each tier has different
//! characteristics for capacity, latency, and persistence.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[cfg(test)]
pub mod tests;

/// Memory visibility scope — controls cross-agent access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[derive(Default)]
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

    /// Tenant ID for multi-tenant isolation.
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

    /// Embedding vector for semantic search (L1+ tiers only).
    pub embedding: Option<Vec<f32>>,

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
    /// Default tenant ID when no tenant is specified.
    pub fn default_tenant() -> String {
        crate::DEFAULT_TENANT.to_string()
    }

    /// Create a new ephemeral memory entry.
    pub fn ephemeral(agent_id: impl Into<String>, content: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4().to_string(),
            agent_id: agent_id.into(),
            tenant_id: Self::default_tenant(),
            tier: MemoryTier::Ephemeral,
            content: MemoryContent::Text(content.into()),
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags: Vec::new(),
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Untyped,
            causal_parent: None,
            supersedes: None,
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
            tenant_id: Self::default_tenant(),
            tier: MemoryTier::LongTerm,
            content,
            importance: 50,
            access_count: 0,
            last_accessed: now,
            created_at: now,
            tags,
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Untyped,
            causal_parent: None,
            supersedes: None,
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

    /// Mark this entry as superseding another (contradiction resolution).
    pub fn with_supersedes(mut self, old_id: impl Into<String>) -> Self {
        self.supersedes = Some(old_id.into());
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

    #[error("Memory quota exceeded: agent={agent_id}, current={current}, limit={limit}")]
    QuotaExceeded { agent_id: String, current: usize, limit: u64 },
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
                if !entries.is_empty() && persister.persist(&agent_id, tier, &entries).is_ok() {
                    persisted += 1;
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
        self.store(entry);
        Ok(())
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

    // ─── Cross-Agent Memory (Scope-Based) ──────────────────────────

    /// Retrieve all Shared-scope entries across all agents from a given tier.
    pub fn get_shared(&self, tier: MemoryTier) -> Vec<MemoryEntry> {
        let map = match tier {
            MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
            MemoryTier::Working => self.working.read().unwrap(),
            MemoryTier::LongTerm => self.long_term.read().unwrap(),
            MemoryTier::Procedural => self.procedural.read().unwrap(),
        };
        map.values()
            .flat_map(|entries| entries.iter())
            .filter(|e| e.scope == MemoryScope::Shared)
            .cloned()
            .collect()
    }

    /// Retrieve Group-scope entries visible to agents in the named group.
    pub fn get_by_group(&self, group: &str, tier: MemoryTier) -> Vec<MemoryEntry> {
        let map = match tier {
            MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
            MemoryTier::Working => self.working.read().unwrap(),
            MemoryTier::LongTerm => self.long_term.read().unwrap(),
            MemoryTier::Procedural => self.procedural.read().unwrap(),
        };
        map.values()
            .flat_map(|entries| entries.iter())
            .filter(|e| matches!(&e.scope, MemoryScope::Group(g) if g == group))
            .cloned()
            .collect()
    }

    /// Retrieve all memories visible to an agent: own private + all shared + matching groups.
    pub fn recall_visible(
        &self,
        agent_id: &str,
        groups: &[String],
    ) -> Vec<MemoryEntry> {
        let mut visible = Vec::new();
        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            let map = match tier {
                MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
                MemoryTier::Working => self.working.read().unwrap(),
                MemoryTier::LongTerm => self.long_term.read().unwrap(),
                MemoryTier::Procedural => self.procedural.read().unwrap(),
            };
            for entries in map.values() {
                for entry in entries {
                    let is_visible = match &entry.scope {
                        MemoryScope::Private => entry.agent_id == agent_id,
                        MemoryScope::Shared => true,
                        MemoryScope::Group(g) => entry.agent_id == agent_id || groups.contains(g),
                    };
                    if is_visible {
                        visible.push(entry.clone());
                    }
                }
            }
        }
        visible
    }

    /// Retrieve all Shared-scope entries across all agents from all tiers.
    pub fn get_shared_entries_all_agents(&self) -> Vec<MemoryEntry> {
        let mut results = Vec::new();
        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            let map = match tier {
                MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
                MemoryTier::Working => self.working.read().unwrap(),
                MemoryTier::LongTerm => self.long_term.read().unwrap(),
                MemoryTier::Procedural => self.procedural.read().unwrap(),
            };
            for entries in map.values() {
                for entry in entries {
                    if entry.scope == MemoryScope::Shared {
                        results.push(entry.clone());
                    }
                }
            }
        }
        results
    }

    /// Retrieve all Group-scope entries for a specific group across all agents from all tiers.
    pub fn get_group_entries_all_agents(&self, group_id: &str) -> Vec<MemoryEntry> {
        let mut results = Vec::new();
        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            let map = match tier {
                MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
                MemoryTier::Working => self.working.read().unwrap(),
                MemoryTier::LongTerm => self.long_term.read().unwrap(),
                MemoryTier::Procedural => self.procedural.read().unwrap(),
            };
            for entries in map.values() {
                for entry in entries {
                    if matches!(&entry.scope, MemoryScope::Group(g) if g == group_id) {
                        results.push(entry.clone());
                    }
                }
            }
        }
        results
    }

    /// Retrieve all entries across all agents and all tiers (AllAccessible scope).
    pub fn get_all_entries_all_agents(&self) -> Vec<MemoryEntry> {
        let mut results = Vec::new();
        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            let map = match tier {
                MemoryTier::Ephemeral => self.ephemeral.read().unwrap(),
                MemoryTier::Working => self.working.read().unwrap(),
                MemoryTier::LongTerm => self.long_term.read().unwrap(),
                MemoryTier::Procedural => self.procedural.read().unwrap(),
            };
            for entries in map.values() {
                for entry in entries {
                    // Include Shared and Group entries, but not Private
                    if matches!(&entry.scope, MemoryScope::Shared | MemoryScope::Group(_)) {
                        results.push(entry.clone());
                    }
                }
            }
        }
        results
    }

    /// Retrieve all memories with access tracking.
    ///
    /// Unlike `get_all()`, this updates `access_count` and `last_accessed`
    /// on every returned entry, then checks for tier promotion.
    pub fn recall_with_tracking(&self, agent_id: &str) -> Vec<MemoryEntry> {
        let mut all = Vec::new();

        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
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

        self.promote_check(agent_id);
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

    /// Evict expired entries (TTL-based) across all tiers for an agent.
    ///
    /// Returns the number of entries evicted.
    pub fn evict_expired(&self, agent_id: &str) -> usize {
        let now = now_ms();
        let mut evicted = 0;

        for tier_map in [&self.ephemeral, &self.working, &self.long_term] {
            let mut map = tier_map.write().unwrap();
            if let Some(entries) = map.get_mut(agent_id) {
                let before = entries.len();
                entries.retain(|e| !crate::memory::relevance::is_expired(e, now));
                evicted += before - entries.len();
            }
        }

        evicted
    }

    /// Check and execute tier promotions for an agent.
    ///
    /// Moves entries that meet promotion thresholds to the next tier:
    /// - Ephemeral → Working (access_count >= 3)
    /// - Working → LongTerm (access_count >= 10 && importance >= 50)
    pub fn promote_check(&self, agent_id: &str) {
        let thresholds = crate::memory::relevance::PromotionThresholds::default();

        // Ephemeral → Working
        let to_promote_working = {
            let mut eph = self.ephemeral.write().unwrap();
            if let Some(entries) = eph.get_mut(agent_id) {
                let (promote, keep): (Vec<_>, Vec<_>) = entries.drain(..).partition(|e| {
                    crate::memory::relevance::check_promotion(e, &thresholds) == Some(MemoryTier::Working)
                });
                *entries = keep;
                promote
            } else {
                Vec::new()
            }
        };
        if !to_promote_working.is_empty() {
            let mut working = self.working.write().unwrap();
            let vec = working.entry(agent_id.to_string()).or_default();
            for mut e in to_promote_working {
                e.tier = MemoryTier::Working;
                vec.push(e);
            }
        }

        // Working → LongTerm
        let to_promote_lt = {
            let mut wk = self.working.write().unwrap();
            if let Some(entries) = wk.get_mut(agent_id) {
                let (promote, keep): (Vec<_>, Vec<_>) = entries.drain(..).partition(|e| {
                    crate::memory::relevance::check_promotion(e, &thresholds) == Some(MemoryTier::LongTerm)
                });
                *entries = keep;
                promote
            } else {
                Vec::new()
            }
        };
        if !to_promote_lt.is_empty() {
            let mut lt = self.long_term.write().unwrap();
            let vec = lt.entry(agent_id.to_string()).or_default();
            for mut e in to_promote_lt {
                e.tier = MemoryTier::LongTerm;
                vec.push(e);
            }
        }
    }

    /// Move a memory entry to a different tier for a specific agent.
    ///
    /// Returns `true` if the entry was found and moved, `false` if not found.
    pub fn move_entry(&self, agent_id: &str, entry_id: &str, target_tier: MemoryTier) -> bool {
        let mut entry_to_move = None;

        for tier_map in [&self.ephemeral, &self.working, &self.long_term, &self.procedural] {
            let mut map = tier_map.write().unwrap();
            if let Some(entries) = map.get_mut(agent_id) {
                if let Some(pos) = entries.iter().position(|e| e.id == entry_id) {
                    entry_to_move = Some(entries.remove(pos));
                    break;
                }
            }
        }

        let Some(mut entry) = entry_to_move else { return false; };
        entry.tier = target_tier;

        let target_map = match target_tier {
            MemoryTier::Ephemeral => &self.ephemeral,
            MemoryTier::Working => &self.working,
            MemoryTier::LongTerm => &self.long_term,
            MemoryTier::Procedural => &self.procedural,
        };
        let mut map = target_map.write().unwrap();
        map.entry(agent_id.to_string()).or_default().push(entry);
        true
    }

    /// Move a memory entry to a different tier (alias for move_entry).
    ///
    /// This is the preferred API for tier movement as it matches
    /// the semantic naming in the memory tier automation spec.
    pub fn move_entry_to_tier(&self, agent_id: &str, entry_id: &str, target_tier: MemoryTier) -> bool {
        self.move_entry(agent_id, entry_id, target_tier)
    }

    /// Delete a specific memory entry by ID.
    ///
    /// Returns `true` if the entry was found and deleted, `false` if not found.
    pub fn delete_entry(&self, agent_id: &str, entry_id: &str) -> bool {
        for tier_map in [&self.ephemeral, &self.working, &self.long_term, &self.procedural] {
            let mut map = tier_map.write().unwrap();
            if let Some(entries) = map.get_mut(agent_id) {
                if let Some(pos) = entries.iter().position(|e| e.id == entry_id) {
                    entries.remove(pos);
                    return true;
                }
            }
        }
        false
    }

    /// Remove all memory entries for an agent across all tiers.
    /// Returns the number of entries removed.
    pub fn clear_agent(&self, agent_id: &str) -> usize {
        let mut count = 0;
        for tier_map in [&self.ephemeral, &self.working, &self.long_term, &self.procedural] {
            let mut map = tier_map.write().unwrap();
            if let Some(entries) = map.remove(agent_id) {
                count += entries.len();
            }
        }
        count
    }

    /// Remove ephemeral (L0) memory entries for an agent.
    /// Used during Memory Consolidation Cycle (A-5) to clear short-term
    /// memories while preserving working/long-term memories across sessions.
    pub fn clear_ephemeral(&self, agent_id: &str) -> usize {
        let mut map = self.ephemeral.write().unwrap();
        if let Some(entries) = map.remove(agent_id) {
            entries.len()
        } else {
            0
        }
    }

    /// Retrieve long-term memories most semantically similar to a query embedding.
    /// Also refreshes TTL for entries that are hit (F-17).
    pub fn recall_semantic(
        &self,
        agent_id: &str,
        query_embedding: &[f32],
        k: usize,
    ) -> Vec<(MemoryEntry, f32)> {
        let mut lt = self.long_term.write().unwrap();
        let entries = match lt.get_mut(agent_id) {
            Some(e) => e,
            None => return Vec::new(),
        };

        // Compute similarities and update access tracking for all entries with embeddings.
        // We do this in two passes to avoid borrowing issues: first collect similarities,
        // then sort and truncate.
        let mut scored: Vec<(MemoryEntry, f32)> = Vec::new();
        for entry in entries.iter_mut() {
            if let Some(emb) = entry.embedding.as_ref() {
                let sim = cosine_similarity(query_embedding, emb);
                if sim > 0.0 {
                    // F-17: refresh TTL on access hit
                    entry.on_memory_access();
                    scored.push((entry.clone(), sim));
                }
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    /// Retrieve relevant memories combining recency/frequency/importance with semantic similarity.
    pub fn recall_relevant_semantic(
        &self,
        agent_id: &str,
        query_embedding: &[f32],
        budget_tokens: usize,
    ) -> Vec<MemoryEntry> {
        let now = now_ms();
        let all = self.recall_with_tracking(agent_id);

        let semantic_scores: std::collections::HashMap<String, f32> = all.iter()
            .filter_map(|e| {
                e.embedding.as_ref().map(|emb| {
                    (e.id.clone(), cosine_similarity(query_embedding, emb))
                })
            })
            .collect();

        let selected = crate::memory::relevance::select_within_budget_semantic(
            &all, budget_tokens, now, &semantic_scores
        );
        selected.into_iter().map(|(entry, _score)| entry).collect()
    }

    // ─── Cognitive Memory Type Retrieval ───────────────────────────

    /// Filter entries by cognitive memory type from a specific tier.
    pub fn get_by_type(
        &self,
        agent_id: &str,
        tier: MemoryTier,
        memory_type: MemoryType,
    ) -> Vec<MemoryEntry> {
        self.get_tier(agent_id, tier)
            .into_iter()
            .filter(|e| e.memory_type == memory_type)
            .collect()
    }

    /// Per-type top-k semantic recall then merge (ENGRAM strategy).
    ///
    /// Instead of a single global top-k, retrieves top-k from each memory type
    /// independently, then merges and re-ranks. This ensures minority types
    /// (e.g., procedural) aren't drowned out by majority types (e.g., episodic).
    pub fn recall_semantic_typed(
        &self,
        agent_id: &str,
        query_embedding: &[f32],
        k_per_type: usize,
    ) -> Vec<(MemoryEntry, f32)> {
        let mut lt = self.long_term.write().unwrap();
        let entries = match lt.get_mut(agent_id) {
            Some(e) => e,
            None => return Vec::new(),
        };

        let mut by_type: HashMap<MemoryType, Vec<(usize, f32)>> = HashMap::new();

        for (idx, entry) in entries.iter().enumerate() {
            if let Some(emb) = entry.embedding.as_ref() {
                let sim = cosine_similarity(query_embedding, emb);
                if sim > 0.0 {
                    by_type.entry(entry.memory_type).or_default().push((idx, sim));
                }
            }
        }

        let mut selected_indices: Vec<(usize, f32)> = Vec::new();
        for (_mtype, mut scored) in by_type {
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(k_per_type);
            selected_indices.extend(scored);
        }

        for &(idx, _) in &selected_indices {
            entries[idx].on_memory_access();
        }

        let mut results: Vec<(MemoryEntry, f32)> = selected_indices
            .into_iter()
            .map(|(idx, sim)| (entries[idx].clone(), sim))
            .collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results
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

    /// Get memory statistics for observability (F-17/F-18).
    /// Returns counts per tier and aggregate stats.
    pub fn get_stats(&self) -> MemoryStats {
        let now = now_ms();
        let mut total_entries = 0;
        let mut total_bytes = 0;
        let mut never_accessed_count = 0;
        let mut about_to_expire_count = 0;
        let mut oldest_entry_age_ms = u64::MAX;
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
                if age < oldest_entry_age_ms {
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
                if age < oldest_entry_age_ms {
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
                if age < oldest_entry_age_ms {
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
                if age < oldest_entry_age_ms {
                    oldest_entry_age_ms = age;
                }
            }
        }

        if oldest_entry_age_ms == u64::MAX {
            oldest_entry_age_ms = 0;
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

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 { 0.0 } else { dot / denom }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
