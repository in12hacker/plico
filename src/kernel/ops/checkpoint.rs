//! Agent Checkpoint Types (v21.0)
//!
//! Provides structured checkpoint types for agent state persistence.
//! The actual checkpoint/restore implementation uses the existing CAS-based
//! mechanism in agent.rs (checkpoint_agent, restore_agent_checkpoint).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use crate::cas::{AIObject, AIObjectMeta, CASStorage, ContentType};
use crate::memory::layered::{MemoryEntry, MemoryTier, MemoryContent, MemoryScope};
use crate::kernel::ops::distributed::{NodeId, MigrationTicket};
use crate::kernel::persistence::atomic_write_json;

/// Tag for checkpoint memory entries.
pub const CHECKPOINT_TAG: &str = "plico:internal:checkpoint";

/// Complete agent checkpoint — all cognitive state needed for continuity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpoint {
    /// Unique checkpoint ID.
    pub checkpoint_id: String,
    /// Agent this checkpoint belongs to.
    pub agent_id: String,
    /// Tenant ID for multi-tenant isolation.
    pub tenant_id: String,
    /// When this checkpoint was created.
    pub created_at_ms: u64,
    /// Agent state at checkpoint time.
    pub agent_state: String,
    /// Pending intent count at checkpoint time.
    pub pending_intents: usize,
    /// Agent's memories serialized for restoration.
    pub memories: Vec<CheckpointMemory>,
    /// KG node IDs associated with this agent.
    pub kg_associations: Vec<String>,
    /// Last intent description if any.
    pub last_intent_description: Option<String>,
    /// Source node if migrated from elsewhere.
    pub source_node: Option<String>,
    /// Checkpoint version for compatibility.
    pub version: u32,
}

/// A memory entry captured in a checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMemory {
    pub id: String,
    pub tier: String,
    pub content_json: String,
    pub importance: u8,
    pub tags: Vec<String>,
    pub scope: String,
    pub created_at_ms: u64,
    pub access_count: u32,
}

impl CheckpointMemory {
    pub fn from_entry(entry: MemoryEntry) -> Self {
        let content_json = match &entry.content {
            MemoryContent::Text(s) => serde_json::json!({ "type": "text", "content": s }),
            MemoryContent::ObjectRef(cid) => serde_json::json!({ "type": "object_ref", "cid": cid }),
            MemoryContent::Structured(v) => v.clone(),
            MemoryContent::Procedure(p) => serde_json::json!({ "type": "procedure", "name": p.name, "description": p.description }),
            MemoryContent::Knowledge(k) => serde_json::json!({ "type": "knowledge", "statement": k.statement }),
        };

        Self {
            id: entry.id.clone(),
            tier: entry.tier.name().to_string(),
            content_json: content_json.to_string(),
            importance: entry.importance,
            tags: entry.tags.clone(),
            scope: match entry.scope {
                MemoryScope::Private => "private".to_string(),
                MemoryScope::Shared => "shared".to_string(),
                MemoryScope::Group(g) => format!("group:{}", g),
            },
            created_at_ms: entry.created_at,
            access_count: entry.access_count,
        }
    }

    pub fn to_memory_entry(&self, agent_id: &str, tenant_id: &str) -> MemoryEntry {
        let tier = match self.tier.as_str() {
            "ephemeral" => MemoryTier::Ephemeral,
            "working" => MemoryTier::Working,
            "long_term" => MemoryTier::LongTerm,
            "procedural" => MemoryTier::Procedural,
            _ => MemoryTier::Working,
        };

        let scope = if self.scope == "private" {
            MemoryScope::Private
        } else if self.scope == "shared" {
            MemoryScope::Shared
        } else if self.scope.starts_with("group:") {
            MemoryScope::Group(self.scope.trim_start_matches("group:").to_string())
        } else {
            MemoryScope::Private
        };

        let content = if self.content_json.is_empty() {
            MemoryContent::Text(String::new())
        } else if let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.content_json) {
            // Determine content type from the value
            if let Some(obj) = v.as_object() {
                if let Some(type_val) = obj.get("type") {
                    match type_val.as_str().unwrap_or("") {
                        "text" => {
                            if let Some(c) = obj.get("content") {
                                MemoryContent::Text(c.as_str().unwrap_or("").to_string())
                            } else {
                                MemoryContent::Text(String::new())
                            }
                        }
                        "object_ref" => {
                            if let Some(cid) = obj.get("cid") {
                                MemoryContent::ObjectRef(cid.as_str().unwrap_or("").to_string())
                            } else {
                                MemoryContent::Text(String::new())
                            }
                        }
                        "procedure" => MemoryContent::Structured(v),
                        "knowledge" => MemoryContent::Structured(v),
                        _ => MemoryContent::Structured(v),
                    }
                } else {
                    MemoryContent::Structured(v)
                }
            } else {
                MemoryContent::Structured(v)
            }
        } else {
            MemoryContent::Text(self.content_json.clone())
        };

        MemoryEntry {
            id: self.id.clone(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            tier,
            content,
            importance: self.importance,
            access_count: self.access_count,
            last_accessed: crate::memory::layered::now_ms(),
            created_at: self.created_at_ms,
            tags: self.tags.clone(),
            embedding: None,
            ttl_ms: None,
            scope,
        }
    }
}

impl AgentCheckpoint {
    /// Create a new checkpoint from current agent state.
    pub fn new(
        agent_id: String,
        tenant_id: String,
        agent_state: crate::scheduler::AgentState,
        pending_intents: usize,
        memories: Vec<MemoryEntry>,
        kg_associations: Vec<String>,
        last_intent_description: Option<String>,
    ) -> Self {
        Self {
            checkpoint_id: uuid::Uuid::new_v4().to_string(),
            agent_id,
            tenant_id,
            created_at_ms: crate::scheduler::agent::now_ms(),
            agent_state: format!("{:?}", agent_state),
            pending_intents,
            memories: memories.into_iter().map(CheckpointMemory::from_entry).collect(),
            kg_associations,
            last_intent_description,
            source_node: None,
            version: 1,
        }
    }

    /// Create a migration ticket for this checkpoint.
    pub fn to_migration_ticket(&self, from_node: NodeId, to_node: NodeId) -> MigrationTicket {
        MigrationTicket {
            agent_id: self.agent_id.clone(),
            from_node,
            to_node,
            checkpoint_cid: self.checkpoint_id.clone(),
            created_at_ms: self.created_at_ms,
        }
    }

    /// Convert checkpoint to a memory entry for persistence.
    pub fn to_memory_entry(&self) -> MemoryEntry {
        let now = crate::memory::layered::now_ms();
        MemoryEntry {
            id: format!("checkpoint-{}", self.checkpoint_id),
            agent_id: self.agent_id.clone(),
            tenant_id: self.tenant_id.clone(),
            tier: MemoryTier::LongTerm, // Checkpoints are always persisted
            content: MemoryContent::Structured(serde_json::to_value(self).unwrap_or_default()),
            importance: 100, // Checkpoints are highest priority
            access_count: 0,
            last_accessed: now,
            created_at: self.created_at_ms,
            tags: vec![CHECKPOINT_TAG.to_string()],
            embedding: None,
            ttl_ms: None,
            scope: MemoryScope::Private,
        }
    }
}

/// Find the most recent checkpoint for an agent from memory entries.
pub fn find_latest_checkpoint(entries: &[MemoryEntry]) -> Option<AgentCheckpoint> {
    entries.iter()
        .filter(|e| e.tags.contains(&CHECKPOINT_TAG.to_string()))
        .max_by_key(|e| e.created_at)
        .and_then(|e| {
            if let MemoryContent::Structured(ref v) = e.content {
                serde_json::from_value::<AgentCheckpoint>(v.clone()).ok()
            } else {
                None
            }
        })
}

/// Checkpoint store — manages checkpoint metadata for quick lookup.
pub struct CheckpointStore {
    checkpoints: RwLock<HashMap<String, AgentCheckpoint>>,
    max_checkpoints_per_agent: usize,
}

impl CheckpointStore {
    pub fn new(max_checkpoints_per_agent: usize) -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
            max_checkpoints_per_agent,
        }
    }

    /// Save a checkpoint, evicting old ones if necessary.
    pub fn save(&self, checkpoint: AgentCheckpoint) {
        let mut store = self.checkpoints.write().unwrap();

        // Evict old checkpoints for this agent if needed
        let agent_checkpoint_ids: Vec<_> = store.iter()
            .filter(|(_, c)| c.agent_id == checkpoint.agent_id)
            .map(|(k, _)| k.clone())
            .collect();

        if agent_checkpoint_ids.len() >= self.max_checkpoints_per_agent {
            // Get checkpoints with timestamps for sorting
            let mut with_timestamps: Vec<_> = agent_checkpoint_ids.iter()
                .map(|k| {
                    let c = store.get(k).unwrap();
                    (k.clone(), c.created_at_ms)
                })
                .collect();
            with_timestamps.sort_by(|a, b| a.1.cmp(&b.1)); // Sort by timestamp ascending

            let to_remove_count = agent_checkpoint_ids.len() - self.max_checkpoints_per_agent + 1;
            for (k, _) in with_timestamps.into_iter().take(to_remove_count) {
                store.remove(&k);
            }
        }

        store.insert(checkpoint.checkpoint_id.clone(), checkpoint);
    }

    /// Get a checkpoint by ID.
    pub fn get(&self, checkpoint_id: &str) -> Option<AgentCheckpoint> {
        self.checkpoints.read().unwrap().get(checkpoint_id).cloned()
    }

    /// Get the latest checkpoint for an agent.
    pub fn latest_for_agent(&self, agent_id: &str) -> Option<AgentCheckpoint> {
        self.checkpoints.read().unwrap()
            .values()
            .filter(|c| c.agent_id == agent_id)
            .max_by_key(|c| c.created_at_ms)
            .cloned()
    }

    /// Delete old checkpoints for an agent.
    pub fn prune_old(&self, agent_id: &str, keep_count: usize) {
        let mut store = self.checkpoints.write().unwrap();
        let mut agent_checkpoints: Vec<_> = store.iter()
            .filter(|(_, c)| c.agent_id == agent_id)
            .map(|(k, c)| (k.clone(), c.created_at_ms))
            .collect();

        agent_checkpoints.sort_by(|a, b| b.1.cmp(&a.1)); // Newest first

        for (i, (k, _)) in agent_checkpoints.into_iter().enumerate() {
            if i >= keep_count {
                store.remove(&k);
            }
        }
    }

    /// List all checkpoint IDs for an agent.
    pub fn list_for_agent(&self, agent_id: &str) -> Vec<String> {
        self.checkpoints.read().unwrap()
            .values()
            .filter(|c| c.agent_id == agent_id)
            .map(|c| c.checkpoint_id.clone())
            .collect()
    }

    /// List all checkpoints in the store.
    pub fn list_all(&self) -> Vec<AgentCheckpoint> {
        self.checkpoints.read().unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// Path to the checkpoint index file.
    fn index_path(root: &Path) -> PathBuf {
        root.join("checkpoint_index.json")
    }

    /// Persist all checkpoints to CAS and write the index file.
    ///
    /// Each checkpoint is serialized as a JSON string and stored as a CAS object.
    /// The index file maps agent_id → Vec<checkpoint_cid>.
    pub fn persist(&self, root: &Path, cas: &CASStorage) {
        let checkpoints = self.checkpoints.read().unwrap();
        let mut index: HashMap<String, Vec<String>> = HashMap::new();

        for (_id, cp) in checkpoints.iter() {
            let json = serde_json::to_string(cp).unwrap_or_default();
            let meta = AIObjectMeta {
                content_type: ContentType::Structured,
                tags: vec!["checkpoint".into(), format!("agent:{}", cp.agent_id)],
                created_by: "plico:checkpoint-store".into(),
                created_at: cp.created_at_ms,
                intent: Some("Agent checkpoint".into()),
                tenant_id: cp.tenant_id.clone(),
            };
            let obj = AIObject::new(json.into_bytes(), meta);
            if let Ok(cid) = cas.put(&obj) {
                index.entry(cp.agent_id.clone()).or_default().push(cid);
            }
        }

        atomic_write_json(&Self::index_path(root), &index);
    }

    /// Restore checkpoints from CAS and the index file.
    ///
    /// Reads the index file, fetches each checkpoint from CAS,
    /// deserializes it, and populates the in-memory store.
    pub fn restore(root: &Path, cas: &CASStorage, max_checkpoints_per_agent: usize) -> Self {
        let path = Self::index_path(root);
        if !path.exists() {
            return Self::new(max_checkpoints_per_agent);
        }

        let index: HashMap<String, Vec<String>> = match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str(&json) {
                Ok(idx) => idx,
                Err(e) => {
                    tracing::warn!("Failed to parse checkpoint index: {e}");
                    return Self::new(max_checkpoints_per_agent);
                }
            },
            Err(e) => {
                tracing::warn!("Failed to read checkpoint index: {e}");
                return Self::new(max_checkpoints_per_agent);
            }
        };

        let mut store = HashMap::new();
        for (agent_id, cids) in index {
            for cid in cids {
                match cas.get(&cid) {
                    Ok(obj) => {
                        if let Ok(cp) = serde_json::from_slice::<AgentCheckpoint>(&obj.data) {
                            store.insert(cp.checkpoint_id.clone(), cp);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to restore checkpoint {} for agent {}: {}",
                            cid, agent_id, e);
                    }
                }
            }
        }

        tracing::info!("Restored {} checkpoints from persistent storage", store.len());
        Self {
            checkpoints: RwLock::new(store),
            max_checkpoints_per_agent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_checkpoint_memory_roundtrip() {
        let entry = MemoryEntry {
            id: "mem-1".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text("test content".to_string()),
            importance: 50,
            access_count: 5,
            last_accessed: 1000,
            created_at: 900,
            tags: vec!["test".to_string()],
            embedding: None,
            ttl_ms: None,
            scope: MemoryScope::Private,
        };

        let cm = CheckpointMemory::from_entry(entry);
        assert_eq!(cm.tier, "working");
        assert_eq!(cm.importance, 50);

        let restored = cm.to_memory_entry("agent-1", "default");
        assert_eq!(restored.id, "mem-1");
        assert_eq!(restored.agent_id, "agent-1");
    }

    #[test]
    fn test_checkpoint_store_eviction() {
        let store = CheckpointStore::new(3);

        for i in 0..5 {
            let checkpoint = AgentCheckpoint::new(
                format!("agent-{}", i % 2),
                "default".to_string(),
                crate::scheduler::AgentState::Waiting,
                0,
                vec![],
                vec![],
                None,
            );
            store.save(checkpoint);
        }

        // Should have at most 3 checkpoints per agent
        assert!(store.list_for_agent("agent-0").len() <= 3);
        assert!(store.list_for_agent("agent-1").len() <= 3);
    }

    #[test]
    fn test_checkpoint_to_memory_entry() {
        let checkpoint = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Running,
            2,
            vec![],
            vec!["kg-node-1".to_string()],
            Some("test task".to_string()),
        );

        let entry = checkpoint.to_memory_entry();
        assert!(entry.tags.contains(&CHECKPOINT_TAG.to_string()));
        assert_eq!(entry.tier, MemoryTier::LongTerm);
        assert_eq!(entry.importance, 100);
    }
}
