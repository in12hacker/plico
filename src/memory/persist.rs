//! Memory Persistence Layer — L1/L2/L3 ↔ CAS
//!
//! Handles serializing memory entries to CAS for durability and
//! restoring them on restart.
//!
//! # Persistence Model
//!
//! - Working/L1 and LongTerm/L2 entries are serialized to JSON and stored in CAS.
//! - A persistence index (`memory_index.json`) maps each agent_id to the
//!   CIDs of their persisted entries.
//! - On startup, `MemoryLoader` reads the index and restores entries from CAS.
//!
//! # Trigger Strategy
//!
//! Persistence is triggered by:
//! 1. **Time-based**: every `PERSIST_INTERVAL_MS` (default 30 minutes).
//! 2. **Count-based**: every `PERSIST_OP_COUNT` operations (default 50).
//!
//! Files layout:
//! ```text
//! root/
//! ├── memory_index.json   (agent_id → Vec<PersistedTier>)
//! └── cas/                (serialized memory entries, stored as CAS objects)
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cas::{AIObject, AIObjectMeta, CASStorage};
use super::layered::{MemoryEntry, MemoryTier};

/// Persistence index — maps agent_id to their persisted tier data.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct PersistenceIndex {
    /// agent_id → list of persisted tiers
    pub agents: HashMap<String, Vec<PersistedTier>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistedTier {
    pub tier: String,
    pub cid: String,
    pub entry_count: usize,
}

/// Errors that can occur during persistence operations.
#[derive(Debug, thiserror::Error)]
pub enum PersistError {
    #[error("Index I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("CAS error: {0}")]
    CAS(String),

    #[error("Agent not found in index: {0}")]
    AgentNotFound(String),
}

impl PersistError {
    pub fn cas(msg: impl Into<String>) -> Self {
        PersistError::CAS(msg.into())
    }
}

/// Trait for persisting memory entries to durable storage.
pub trait MemoryPersister: Send + Sync {
    /// Persist entries for a specific tier and agent.
    /// Returns the CID of the stored blob.
    fn persist(&self, agent_id: &str, tier: MemoryTier, entries: &[MemoryEntry]) -> Result<String, PersistError>;

    /// Load all persisted entries for an agent and tier.
    fn load(&self, agent_id: &str, tier: MemoryTier) -> Result<Vec<MemoryEntry>, PersistError>;

    /// Get the list of persisted CIDs for an agent.
    fn list_persisted(&self, agent_id: &str) -> Result<Vec<PersistedTier>, PersistError>;

    /// Check if any data is persisted for an agent.
    fn has_persisted(&self, agent_id: &str) -> bool;

    /// List all agent IDs that have persisted data.
    fn list_all_agent_ids(&self) -> Vec<String>;
}

/// A memory persister that stores serialized entries in CAS.
#[derive(Clone)]
pub struct CASPersister {
    cas: std::sync::Arc<CASStorage>,
    index_path: PathBuf,
    /// Arc<RwLock> allows cheap cloning while preserving interior mutability.
    index: std::sync::Arc<std::sync::RwLock<PersistenceIndex>>,
}

impl CASPersister {
    /// Create a new CASPersister rooted at `root`.
    /// The root should point to the kernel data root (e.g. `/tmp/plico`).
    pub fn new(cas: std::sync::Arc<CASStorage>, root: PathBuf) -> std::io::Result<Self> {
        let index_path = root.join("memory_index.json");
        let index = Self::load_index(&index_path).unwrap_or_default();
        Ok(Self {
            cas,
            index_path,
            index: std::sync::Arc::new(std::sync::RwLock::new(index)),
        })
    }

    fn load_index(path: &Path) -> std::io::Result<PersistenceIndex> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    fn save_index(&self) -> std::io::Result<()> {
        let index = self.index.read().unwrap();
        let json = serde_json::to_string_pretty(&*index)?;
        fs::write(&self.index_path, json)
    }

    fn tier_name(tier: MemoryTier) -> &'static str {
        match tier {
            MemoryTier::Ephemeral => "ephemeral",
            MemoryTier::Working => "working",
            MemoryTier::LongTerm => "long_term",
            MemoryTier::Procedural => "procedural",
        }
    }

    fn make_meta(agent_id: &str, tier: MemoryTier) -> AIObjectMeta {
        AIObjectMeta {
            content_type: crate::cas::ContentType::Structured,
            tags: vec![
                "memory".to_string(),
                Self::tier_name(tier).to_string(),
                format!("agent:{}", agent_id),
            ],
            created_by: "plico:memory-persister".to_string(),
            created_at: now_ms(),
            intent: Some(format!("Persisted {} memory for agent {}", Self::tier_name(tier), agent_id)),
        }
    }
}

impl MemoryPersister for CASPersister {
    fn persist(&self, agent_id: &str, tier: MemoryTier, entries: &[MemoryEntry]) -> Result<String, PersistError> {
        if entries.is_empty() {
            return Ok(String::new());
        }

        // Serialize entries to JSON
        let json = serde_json::to_string(entries)?;
        let meta = Self::make_meta(agent_id, tier);
        let obj = AIObject::new(json.into_bytes(), meta);

        // Store in CAS
        let cid = self.cas
            .put(&obj)
            .map_err(|e| PersistError::cas(e.to_string()))?;

        // Update index
        {
            let mut index = self.index.write().unwrap();
            let tiers = index.agents.entry(agent_id.to_string()).or_default();

            // Remove existing entry for this tier
            tiers.retain(|pt| pt.tier != Self::tier_name(tier));

            tiers.push(PersistedTier {
                tier: Self::tier_name(tier).to_string(),
                cid: cid.clone(),
                entry_count: entries.len(),
            });
        }

        self.save_index().map_err(PersistError::Io)?;

        Ok(cid)
    }

    fn load(&self, agent_id: &str, tier: MemoryTier) -> Result<Vec<MemoryEntry>, PersistError> {
        let cid = {
            let index = self.index.read().unwrap();
            let tiers = index.agents.get(agent_id)
                .ok_or_else(|| PersistError::AgentNotFound(agent_id.to_string()))?;

            let pt = tiers.iter()
                .find(|pt| pt.tier == Self::tier_name(tier))
                .ok_or_else(|| PersistError::AgentNotFound(format!("{} tier not found for {}", Self::tier_name(tier), agent_id)))?;

            pt.cid.clone()
        };

        let obj = self.cas.get(&cid)
            .map_err(|e| PersistError::cas(e.to_string()))?;

        let entries: Vec<MemoryEntry> = serde_json::from_slice(&obj.data)
            .map_err(PersistError::Serialization)?;

        Ok(entries)
    }

    fn list_persisted(&self, agent_id: &str) -> Result<Vec<PersistedTier>, PersistError> {
        let index = self.index.read().unwrap();
        Ok(index.agents.get(agent_id).cloned().unwrap_or_default())
    }

    fn has_persisted(&self, agent_id: &str) -> bool {
        let index = self.index.read().unwrap();
        index.agents.contains_key(agent_id)
    }

    fn list_all_agent_ids(&self) -> Vec<String> {
        let index = self.index.read().unwrap();
        index.agents.keys().cloned().collect()
    }
}

/// Load persisted memories for all agents from CAS.
pub struct MemoryLoader {
    persister: std::sync::Arc<dyn MemoryPersister>,
}

impl MemoryLoader {
    pub fn new(persister: std::sync::Arc<dyn MemoryPersister>) -> Self {
        Self { persister }
    }

    /// Restore all persisted memories for all agents.
    /// Returns a map of agent_id → Vec<(tier, entries)>.
    pub fn restore_all(&self) -> HashMap<String, Vec<(MemoryTier, Vec<MemoryEntry>)>> {
        let mut result = HashMap::new();
        let agent_ids = self.persister.list_all_agent_ids();

        for agent_id in agent_ids {
            match self.restore_agent(&agent_id) {
                Ok(tiers) => {
                    result.insert(agent_id, tiers);
                }
                Err(e) => {
                    tracing::warn!("Failed to restore memories for agent: {}", e);
                }
            }
        }

        result
    }

    /// Restore persisted memories for a specific agent.
    pub fn restore_agent(&self, agent_id: &str) -> Result<Vec<(MemoryTier, Vec<MemoryEntry>)>, PersistError> {
        let mut result = Vec::new();

        for tier in [MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            match self.persister.load(agent_id, tier) {
                Ok(entries) if !entries.is_empty() => {
                    result.push((tier, entries));
                }
                Ok(_) => {}
                Err(PersistError::AgentNotFound(_)) => {}
                Err(e) => return Err(e),
            }
        }

        Ok(result)
    }

    /// Check if any memory is persisted for a given agent.
    pub fn has_persisted(&self, agent_id: &str) -> bool {
        self.persister.has_persisted(agent_id)
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
    use tempfile::tempdir;
    use crate::memory::MemoryContent;
    use crate::cas::CASStorage;

    fn make_persister() -> (CASPersister, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let cas = CASStorage::new(dir.path().join("cas")).unwrap();
        let persister = CASPersister::new(std::sync::Arc::new(cas), dir.path().to_path_buf()).unwrap();
        (persister, dir)
    }

    #[test]
    fn test_persist_and_load_round_trip() {
        let (persister, _dir) = make_persister();

        let entries = vec![
            MemoryEntry::long_term(
                "agent1",
                MemoryContent::Text("Important fact about Rust".to_string()),
                vec!["rust".to_string(), "facts".to_string()],
            ),
        ];

        let cid = persister.persist("agent1", MemoryTier::LongTerm, &entries).unwrap();
        assert!(!cid.is_empty());

        let loaded: Vec<MemoryEntry> = persister.load("agent1", MemoryTier::LongTerm).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].agent_id, "agent1");
    }

    #[test]
    fn test_load_nonexistent_agent() {
        let (persister, _dir) = make_persister();
        let result: Result<Vec<MemoryEntry>, _> = persister.load("ghost", MemoryTier::LongTerm);
        assert!(result.is_err());
    }

    #[test]
    fn test_has_persisted() {
        let (persister, _dir) = make_persister();
        assert!(!persister.has_persisted("agent1"));

        let entries = vec![MemoryEntry::long_term("agent1", MemoryContent::Text("x".to_string()), vec![])];
        persister.persist("agent1", MemoryTier::LongTerm, &entries).unwrap();

        assert!(persister.has_persisted("agent1"));
        assert!(!persister.has_persisted("agent2"));
    }

    #[test]
    fn test_index_persists_across_restart() {
        let (persister, _dir) = make_persister();
        let root = _dir.path().to_path_buf();

        let entries = vec![MemoryEntry::long_term("agent1", MemoryContent::Text("y".to_string()), vec![])];
        persister.persist("agent1", MemoryTier::Working, &entries).unwrap();

        // Simulate restart: create new persister from same root
        let cas = CASStorage::new(root.join("cas")).unwrap();
        let persister2 = CASPersister::new(std::sync::Arc::new(cas), root).unwrap();

        assert!(persister2.has_persisted("agent1"));
        let loaded = persister2.load("agent1", MemoryTier::Working).unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[test]
    fn test_persist_multiple_tiers_isolated() {
        let (persister, _dir) = make_persister();

        let working = vec![MemoryEntry::long_term("agent1", MemoryContent::Text("w".to_string()), vec![])];
        let long_term = vec![MemoryEntry::long_term("agent1", MemoryContent::Text("l".to_string()), vec![])];

        persister.persist("agent1", MemoryTier::Working, &working).unwrap();
        persister.persist("agent1", MemoryTier::LongTerm, &long_term).unwrap();

        let w: Vec<MemoryEntry> = persister.load("agent1", MemoryTier::Working).unwrap();
        let l: Vec<MemoryEntry> = persister.load("agent1", MemoryTier::LongTerm).unwrap();

        assert_eq!(w.len(), 1);
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn test_memory_loader_restore_agent() {
        let (persister, _dir) = make_persister();

        let entries = vec![
            MemoryEntry::long_term("agent1", MemoryContent::Text("w".to_string()), vec![]),
        ];
        persister.persist("agent1", MemoryTier::Working, &entries).unwrap();

        // Restore using loader (which creates its own Arc reference)
        let loader = MemoryLoader::new(std::sync::Arc::new(persister));
        let restored = loader.restore_agent("agent1").unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].0, MemoryTier::Working);
        assert_eq!(restored[0].1.len(), 1);
    }
}
