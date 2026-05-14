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
            MemoryContent::Procedure(p) => serde_json::json!({
                "type": "procedure",
                "name": p.name,
                "description": p.description,
                "steps": p.steps,
                "learned_from": p.learned_from,
            }),
            MemoryContent::Knowledge(k) => serde_json::json!({
                "type": "knowledge",
                "subject": k.subject,
                "statement": k.statement,
                "confidence": k.confidence,
                "source": k.source,
            }),
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
            if let Some(obj) = v.as_object() {
                match obj.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                    "text" => {
                        let c = obj.get("content").and_then(|c| c.as_str()).unwrap_or("");
                        MemoryContent::Text(c.to_string())
                    }
                    "object_ref" => {
                        let cid = obj.get("cid").and_then(|c| c.as_str()).unwrap_or("");
                        MemoryContent::ObjectRef(cid.to_string())
                    }
                    "procedure" => {
                        // F-39: Restore as proper Procedure type, not Structured.
                        if let Ok(proc) = serde_json::from_value::<crate::memory::layered::Procedure>(v.clone()) {
                            MemoryContent::Procedure(proc)
                        } else {
                            // Fallback: construct from available fields.
                            MemoryContent::Procedure(crate::memory::layered::Procedure {
                                name: obj.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string(),
                                description: obj.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                                steps: obj.get("steps").and_then(|s| serde_json::from_value(s.clone()).ok()).unwrap_or_default(),
                                learned_from: obj.get("learned_from").and_then(|l| l.as_str()).unwrap_or("").to_string(),
                            })
                        }
                    }
                    "knowledge" => {
                        // F-39: Restore as proper KnowledgePiece type, not Structured.
                        if let Ok(k) = serde_json::from_value::<crate::memory::layered::KnowledgePiece>(v.clone()) {
                            MemoryContent::Knowledge(k)
                        } else {
                            // Fallback: construct from available fields.
                            MemoryContent::Knowledge(crate::memory::layered::KnowledgePiece {
                                subject: obj.get("subject").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                                statement: obj.get("statement").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                                confidence: obj.get("confidence").and_then(|c| c.as_f64()).unwrap_or(1.0) as f32,
                                source: obj.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                            })
                        }
                    }
                    _ => MemoryContent::Structured(v),
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
            original_ttl_ms: None,
            scope,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
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
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Structured(serde_json::to_value(self).unwrap_or_default()),
            importance: 100,
            access_count: 0,
            last_accessed: now,
            created_at: self.created_at_ms,
            tags: vec![CHECKPOINT_TAG.to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
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
            with_timestamps.sort_by_key(|(_, ts)| *ts);

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

        agent_checkpoints.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));

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
        // Load old index to collect CIDs that will be replaced
        let old_index: HashMap<String, Vec<String>> = std::fs::read_to_string(Self::index_path(root))
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default();

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

        // Release the lock before cleanup
        drop(checkpoints);

        atomic_write_json(&Self::index_path(root), &index);

        // Delete old CAS objects that are no longer referenced
        let new_cids: std::collections::HashSet<&str> = index.values()
            .flat_map(|v| v.iter().map(|s| s.as_str()))
            .collect();
        for old_cids in old_index.values() {
            for cid in old_cids {
                if !new_cids.contains(cid.as_str()) {
                    let _ = cas.delete(cid);
                }
            }
        }
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
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
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

    #[test]
    fn test_checkpoint_memory_text_roundtrip() {
        let entry = MemoryEntry {
            id: "mem-text".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text("hello world".to_string()),
            importance: 80,
            access_count: 10,
            last_accessed: 2000,
            created_at: 1000,
            tags: vec!["important".to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Shared,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };

        let cm = CheckpointMemory::from_entry(entry.clone());
        assert_eq!(cm.tier, "long_term");
        assert_eq!(cm.importance, 80);
        assert_eq!(cm.scope, "shared");
        assert_eq!(cm.access_count, 10);

        let restored = cm.to_memory_entry("agent-1", "default");
        assert_eq!(restored.id, "mem-text");
        assert_eq!(restored.importance, 80);
        match &restored.content {
            MemoryContent::Text(s) => assert_eq!(s, "hello world"),
            _ => panic!("expected Text content"),
        }
    }

    #[test]
    fn test_checkpoint_memory_object_ref_roundtrip() {
        let entry = MemoryEntry {
            id: "mem-obj".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::ObjectRef("sha256abc".to_string()),
            importance: 50,
            access_count: 3,
            last_accessed: 1500,
            created_at: 1000,
            tags: vec![],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };

        let cm = CheckpointMemory::from_entry(entry);
        let restored = cm.to_memory_entry("agent-1", "default");
        match &restored.content {
            MemoryContent::ObjectRef(cid) => assert_eq!(cid, "sha256abc"),
            _ => panic!("expected ObjectRef"),
        }
    }

    #[test]
    fn test_checkpoint_memory_procedure_roundtrip() {
        use crate::memory::layered::Procedure;
        let entry = MemoryEntry {
            id: "mem-proc".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Procedural,
            content: MemoryContent::Procedure(Procedure {
                name: "test_proc".to_string(),
                description: "a test procedure".to_string(),
                steps: vec![
                    crate::memory::layered::ProcedureStep {
                        step_number: 1,
                        description: "first step".to_string(),
                        action: "do thing".to_string(),
                        expected_outcome: "done".to_string(),
                    },
                    crate::memory::layered::ProcedureStep {
                        step_number: 2,
                        description: "second step".to_string(),
                        action: "do other".to_string(),
                        expected_outcome: "finished".to_string(),
                    },
                ],
                learned_from: "experience".to_string(),
            }),
            importance: 90,
            access_count: 7,
            last_accessed: 3000,
            created_at: 2000,
            tags: vec!["skill".to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };

        let cm = CheckpointMemory::from_entry(entry);
        let restored = cm.to_memory_entry("agent-1", "default");
        match &restored.content {
            MemoryContent::Procedure(p) => {
                assert_eq!(p.name, "test_proc");
                assert_eq!(p.steps.len(), 2);
            }
            _ => panic!("expected Procedure"),
        }
    }

    #[test]
    fn test_checkpoint_memory_knowledge_roundtrip() {
        use crate::memory::layered::KnowledgePiece;
        let entry = MemoryEntry {
            id: "mem-know".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Knowledge(KnowledgePiece {
                subject: "rust".to_string(),
                statement: "borrow checker prevents data races".to_string(),
                confidence: 0.95,
                source: "experience".to_string(),
            }),
            importance: 95,
            access_count: 20,
            last_accessed: 5000,
            created_at: 4000,
            tags: vec!["knowledge".to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };

        let cm = CheckpointMemory::from_entry(entry);
        let restored = cm.to_memory_entry("agent-1", "default");
        match &restored.content {
            MemoryContent::Knowledge(k) => {
                assert_eq!(k.subject, "rust");
                assert!((k.confidence - 0.95).abs() < 0.01);
            }
            _ => panic!("expected Knowledge"),
        }
    }

    #[test]
    fn test_checkpoint_memory_group_scope() {
        let entry = MemoryEntry {
            id: "mem-group".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text("group data".to_string()),
            importance: 50,
            access_count: 1,
            last_accessed: 1000,
            created_at: 1000,
            tags: vec![],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Group("team-a".to_string()),
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        };

        let cm = CheckpointMemory::from_entry(entry);
        assert_eq!(cm.scope, "group:team-a");

        let restored = cm.to_memory_entry("agent-1", "default");
        assert_eq!(restored.scope, MemoryScope::Group("team-a".to_string()));
    }

    #[test]
    fn test_checkpoint_memory_unknown_tier_defaults_to_working() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "unknown_tier".to_string(),
            content_json: r#"{"type":"text","content":"test"}"#.to_string(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        assert_eq!(entry.tier, MemoryTier::Working);
    }

    #[test]
    fn test_checkpoint_memory_unknown_scope_defaults_to_private() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: r#"{"type":"text","content":"test"}"#.to_string(),
            importance: 50,
            tags: vec![],
            scope: "unknown_scope".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        assert_eq!(entry.scope, MemoryScope::Private);
    }

    #[test]
    fn test_checkpoint_memory_empty_content_json() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: String::new(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        match &entry.content {
            MemoryContent::Text(s) => assert!(s.is_empty()),
            _ => panic!("expected Text for empty content"),
        }
    }

    #[test]
    fn test_checkpoint_memory_invalid_json_fallback() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: "not valid json".to_string(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        match &entry.content {
            MemoryContent::Text(s) => assert_eq!(s, "not valid json"),
            _ => panic!("expected Text fallback"),
        }
    }

    #[test]
    fn test_checkpoint_memory_structured_content() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: r#"{"custom":"data","nested":{"key":"value"}}"#.to_string(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        // Should be Structured since it's a JSON object but type is not text/object_ref/procedure/knowledge
        assert!(matches!(entry.content, MemoryContent::Structured(_)));
    }

    #[test]
    fn test_find_latest_checkpoint_empty() {
        let result = find_latest_checkpoint(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn test_find_latest_checkpoint_no_matching_tags() {
        let entries = vec![MemoryEntry {
            id: "m1".to_string(),
            agent_id: "agent-1".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text("not a checkpoint".to_string()),
            importance: 50,
            access_count: 0,
            last_accessed: 1000,
            created_at: 1000,
            tags: vec!["other".to_string()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: crate::memory::MemoryType::default(),
            causal_parent: None,
            supersedes: None,
        }];
        let result = find_latest_checkpoint(&entries);
        assert!(result.is_none());
    }

    #[test]
    fn test_checkpoint_store_get_nonexistent() {
        let store = CheckpointStore::new(5);
        assert!(store.get("nonexistent").is_none());
    }

    #[test]
    fn test_checkpoint_store_latest_for_agent_empty() {
        let store = CheckpointStore::new(5);
        assert!(store.latest_for_agent("agent-1").is_none());
    }

    #[test]
    fn test_checkpoint_store_list_for_agent_empty() {
        let store = CheckpointStore::new(5);
        assert!(store.list_for_agent("agent-1").is_empty());
    }

    #[test]
    fn test_checkpoint_store_list_all_empty() {
        let store = CheckpointStore::new(5);
        assert!(store.list_all().is_empty());
    }

    #[test]
    fn test_checkpoint_store_save_and_get() {
        let store = CheckpointStore::new(5);
        let cp = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Running,
            1,
            vec![],
            vec![],
            None,
        );
        let id = cp.checkpoint_id.clone();
        store.save(cp);

        let retrieved = store.get(&id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().agent_id, "agent-1");
    }

    #[test]
    fn test_checkpoint_store_latest_for_agent() {
        let store = CheckpointStore::new(5);
        let cp1 = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Running,
            1,
            vec![],
            vec![],
            None,
        );
        let cp2 = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Waiting,
            2,
            vec![],
            vec![],
            None,
        );
        store.save(cp1);
        store.save(cp2);

        let latest = store.latest_for_agent("agent-1");
        assert!(latest.is_some());
    }

    #[test]
    fn test_checkpoint_store_prune_old() {
        let store = CheckpointStore::new(10);
        for i in 0..5 {
            let cp = AgentCheckpoint::new(
                "agent-1".to_string(),
                "default".to_string(),
                crate::scheduler::AgentState::Running,
                i,
                vec![],
                vec![],
                None,
            );
            store.save(cp);
        }
        assert_eq!(store.list_for_agent("agent-1").len(), 5);

        store.prune_old("agent-1", 2);
        assert_eq!(store.list_for_agent("agent-1").len(), 2);
    }

    #[test]
    fn test_checkpoint_store_list_all() {
        let store = CheckpointStore::new(5);
        let cp1 = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Running,
            1,
            vec![],
            vec![],
            None,
        );
        let cp2 = AgentCheckpoint::new(
            "agent-2".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Waiting,
            1,
            vec![],
            vec![],
            None,
        );
        store.save(cp1);
        store.save(cp2);

        let all = store.list_all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_checkpoint_to_migration_ticket() {
        let cp = AgentCheckpoint::new(
            "agent-1".to_string(),
            "default".to_string(),
            crate::scheduler::AgentState::Running,
            0,
            vec![],
            vec![],
            None,
        );
        let ticket = cp.to_migration_ticket(
            crate::kernel::ops::distributed::NodeId("node-a".to_string()),
            crate::kernel::ops::distributed::NodeId("node-b".to_string()),
        );
        assert_eq!(ticket.agent_id, "agent-1");
        assert_eq!(ticket.from_node.0, "node-a");
        assert_eq!(ticket.to_node.0, "node-b");
    }

    #[test]
    fn test_checkpoint_memory_unknown_content_type() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: r#"{"type":"unknown_type","data":"test"}"#.to_string(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        // Unknown type → Structured
        assert!(matches!(entry.content, MemoryContent::Structured(_)));
    }

    #[test]
    fn test_checkpoint_memory_non_object_json() {
        let cm = CheckpointMemory {
            id: "m1".to_string(),
            tier: "working".to_string(),
            content_json: r#"["array","not","object"]"#.to_string(),
            importance: 50,
            tags: vec![],
            scope: "private".to_string(),
            created_at_ms: 1000,
            access_count: 0,
        };
        let entry = cm.to_memory_entry("agent-1", "default");
        // JSON array → Structured (not an object)
        assert!(matches!(entry.content, MemoryContent::Structured(_)));
    }
}
