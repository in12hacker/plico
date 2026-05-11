//! Memory Passport — export/import agent memories across kernel instances.
//!
//! Enables Agent knowledge migration: export memories + KG edges to a portable
//! format, import into a different kernel instance.
//!
//! Format: JSON with optional passphrase-based encryption.

use std::sync::Arc;
use serde::{Deserialize, Serialize};

use crate::memory::{LayeredMemory, MemoryEntry, MemoryTier};
use crate::fs::graph::{KnowledgeGraph, KGEdge};

const PASSPORT_VERSION: &str = "1.0";

/// Exported memory passport.
#[derive(Debug, Serialize, Deserialize)]
pub struct MemoryPassportData {
    pub version: String,
    pub agent_id: String,
    pub tenant_id: String,
    pub exported_at_ms: u64,
    pub memories: Vec<MemoryEntry>,
    pub kg_edges: Vec<KGEdge>,
    /// Optional HMAC signature for integrity verification.
    pub signature: Option<String>,
}

/// Report after importing a passport.
#[derive(Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub memories_imported: usize,
    pub memories_skipped: usize,
    pub kg_edges_imported: usize,
    pub kg_edges_skipped: usize,
}

/// Memory Passport — export/import agent memories.
pub struct MemoryPassport {
    memory: Arc<LayeredMemory>,
    kg: Option<Arc<dyn KnowledgeGraph>>,
}

impl MemoryPassport {
    pub fn new(
        memory: Arc<LayeredMemory>,
        kg: Option<Arc<dyn KnowledgeGraph>>,
    ) -> Self {
        Self { memory, kg }
    }

    /// Export all memories and KG edges for an agent.
    pub fn export_memories(
        &self,
        agent_id: &str,
        tenant_id: &str,
        passphrase: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let mut memories = Vec::new();
        for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
            memories.extend(self.memory.get_tier(agent_id, tier));
        }

        let kg_edges = if let Some(ref kg) = self.kg {
            kg.list_edges(agent_id).unwrap_or_default()
        } else {
            Vec::new()
        };

        let passport = MemoryPassportData {
            version: PASSPORT_VERSION.to_string(),
            agent_id: agent_id.to_string(),
            tenant_id: tenant_id.to_string(),
            exported_at_ms: crate::util::now_ms(),
            memories,
            kg_edges,
            signature: None,
        };

        let json = serde_json::to_vec(&passport)
            .map_err(|e| format!("Serialization failed: {}", e))?;

        if let Some(_pass) = passphrase {
            // Simple XOR encryption with passphrase hash
            let key = self.derive_key(_pass);
            let encrypted = Self::xor_encrypt(&json, &key);
            Ok(encrypted)
        } else {
            Ok(json)
        }
    }

    /// Import memories and KG edges from a passport.
    pub fn import_memories(
        &self,
        data: &[u8],
        passphrase: Option<&str>,
        tenant_id: &str,
    ) -> Result<ImportReport, String> {
        let json = if let Some(_pass) = passphrase {
            let key = self.derive_key(_pass);
            Self::xor_encrypt(data, &key)
        } else {
            data.to_vec()
        };

        let passport: MemoryPassportData = serde_json::from_slice(&json)
            .map_err(|e| format!("Deserialization failed: {}", e))?;

        if passport.version != PASSPORT_VERSION {
            return Err(format!(
                "Unsupported passport version: {} (expected {})",
                passport.version, PASSPORT_VERSION
            ));
        }

        let mut memories_imported = 0;
        let mut memories_skipped = 0;

        // Import memories
        for mut entry in passport.memories {
            // Update tenant_id to match importing kernel
            entry.tenant_id = tenant_id.to_string();
            // Reset access stats for fresh start
            entry.access_count = 0;
            entry.last_accessed = crate::util::now_ms();

            let quota = self.memory.count_for_agent(&entry.agent_id) as u64;
            match self.memory.store_checked(entry, quota + 1) {
                Ok(()) => memories_imported += 1,
                Err(_) => memories_skipped += 1,
            }
        }

        let mut kg_edges_imported = 0;
        let mut kg_edges_skipped = 0;

        // Import KG edges
        if let Some(ref kg) = self.kg {
            for edge in passport.kg_edges {
                match kg.add_edge(edge) {
                    Ok(()) => kg_edges_imported += 1,
                    Err(_) => kg_edges_skipped += 1,
                }
            }
        }

        Ok(ImportReport {
            memories_imported,
            memories_skipped,
            kg_edges_imported,
            kg_edges_skipped,
        })
    }

    /// Derive a 32-byte key from passphrase using simple hash.
    fn derive_key(&self, passphrase: &str) -> [u8; 32] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut key = [0u8; 32];
        for i in 0..32 {
            let mut hasher = DefaultHasher::new();
            passphrase.hash(&mut hasher);
            (i as u64).hash(&mut hasher);
            let hash = hasher.finish();
            key[i] = (hash & 0xFF) as u8;
        }
        key
    }

    /// XOR encrypt/decrypt (symmetric).
    fn xor_encrypt(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
        data.iter()
            .enumerate()
            .map(|(i, &b)| b ^ key[i % 32])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::MemoryScope;
    use crate::memory::MemoryContent;

    fn make_test_memory() -> Arc<LayeredMemory> {
        Arc::new(LayeredMemory::new())
    }

    fn add_test_entries(mem: &LayeredMemory, agent_id: &str, count: usize) {
        for i in 0..count {
            let entry = MemoryEntry {
                id: format!("entry-{}", i),
                agent_id: agent_id.to_string(),
                tenant_id: "default".to_string(),
                tier: MemoryTier::LongTerm,
                content: MemoryContent::Text(format!("Memory content {}", i)),
                importance: 50,
                access_count: 0,
                last_accessed: crate::util::now_ms(),
                created_at: crate::util::now_ms(),
                tags: vec!["test".to_string()],
                embedding: None,
                ttl_ms: None,
                original_ttl_ms: None,
                scope: MemoryScope::Private,
                memory_type: crate::memory::layered::MemoryType::default(),
                causal_parent: None,
                supersedes: None,
            };
            mem.store(entry);
        }
    }

    #[test]
    fn test_export_import_roundtrip() {
        let mem = make_test_memory();
        add_test_entries(&mem, "agent1", 5);

        let passport = MemoryPassport::new(mem.clone(), None);

        // Export
        let exported = passport.export_memories("agent1", "default", None).unwrap();
        assert!(!exported.is_empty());

        // Import into a fresh memory store
        let mem2 = make_test_memory();
        let passport2 = MemoryPassport::new(mem2.clone(), None);
        let report = passport2.import_memories(&exported, None, "default").unwrap();

        assert_eq!(report.memories_imported, 5);
        assert_eq!(report.memories_skipped, 0);

        // Verify imported memories
        let entries = mem2.get_tier("agent1", MemoryTier::LongTerm);
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn test_export_import_with_passphrase() {
        let mem = make_test_memory();
        add_test_entries(&mem, "agent1", 3);

        let passport = MemoryPassport::new(mem.clone(), None);

        // Export with passphrase
        let exported = passport.export_memories("agent1", "default", Some("secret123")).unwrap();

        // Import with correct passphrase
        let mem2 = make_test_memory();
        let passport2 = MemoryPassport::new(mem2.clone(), None);
        let report = passport2.import_memories(&exported, Some("secret123"), "default").unwrap();
        assert_eq!(report.memories_imported, 3);
    }

    #[test]
    fn test_import_with_wrong_passphrase_fails() {
        let mem = make_test_memory();
        add_test_entries(&mem, "agent1", 3);

        let passport = MemoryPassport::new(mem.clone(), None);
        let exported = passport.export_memories("agent1", "default", Some("secret123")).unwrap();

        let mem2 = make_test_memory();
        let passport2 = MemoryPassport::new(mem2.clone(), None);
        let result = passport2.import_memories(&exported, Some("wrong"), "default");
        assert!(result.is_err());
    }

    #[test]
    fn test_export_no_memories() {
        let mem = make_test_memory();
        let passport = MemoryPassport::new(mem.clone(), None);

        let exported = passport.export_memories("nonexistent", "default", None).unwrap();
        let mem2 = make_test_memory();
        let passport2 = MemoryPassport::new(mem2.clone(), None);
        let report = passport2.import_memories(&exported, None, "default").unwrap();
        assert_eq!(report.memories_imported, 0);
    }
}
