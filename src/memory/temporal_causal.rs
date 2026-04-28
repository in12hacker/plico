//! Temporal-Causal Index — inverted index combining time windows with causal chains.
//!
//! Maintains `(entity, timestamp, causal_parent) -> [memory_ids]` mapping.
//! Supports queries like "what changed last week that caused this problem?"
//!
//! Query flow:
//! 1. Parse time window (reuse v29 HeuristicTemporalResolver)
//! 2. Find all memories matching entity + time window
//! 3. Expand along causal chains to find root causes

use crate::memory::layered::MemoryEntry;
use crate::memory::causal::CausalGraph;
use std::collections::{HashMap, HashSet, BTreeMap};

/// An entry in the temporal-causal index.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub memory_id: String,
    pub timestamp_ms: u64,
    pub causal_parent: Option<String>,
    pub entities: Vec<String>,
}

/// The temporal-causal inverted index.
#[derive(Debug, Default)]
pub struct TemporalCausalIndex {
    by_entity: HashMap<String, Vec<String>>,
    by_time: BTreeMap<u64, Vec<String>>,
    entries: HashMap<String, IndexEntry>,
}

impl TemporalCausalIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the index from memory entries.
    ///
    /// Entities are extracted from tags and content keywords.
    pub fn build(memories: &[MemoryEntry]) -> Self {
        let mut index = Self::new();
        for entry in memories {
            let entities = extract_entities(entry);
            let idx_entry = IndexEntry {
                memory_id: entry.id.clone(),
                timestamp_ms: entry.created_at,
                causal_parent: entry.causal_parent.clone(),
                entities: entities.clone(),
            };

            for entity in &entities {
                index
                    .by_entity
                    .entry(entity.clone())
                    .or_default()
                    .push(entry.id.clone());
            }

            index
                .by_time
                .entry(entry.created_at)
                .or_default()
                .push(entry.id.clone());

            index.entries.insert(entry.id.clone(), idx_entry);
        }
        index
    }

    /// Query memories related to an entity within a time window.
    pub fn query_entity_in_window(
        &self,
        entity: &str,
        start_ms: u64,
        end_ms: u64,
    ) -> Vec<&IndexEntry> {
        let entity_lower = entity.to_lowercase();
        let candidate_ids: HashSet<&String> = self
            .by_entity
            .get(&entity_lower)
            .map(|ids| ids.iter().collect())
            .unwrap_or_default();

        let mut results = Vec::new();
        for (_, ids) in self.by_time.range(start_ms..=end_ms) {
            for id in ids {
                if candidate_ids.contains(id) {
                    if let Some(entry) = self.entries.get(id) {
                        results.push(entry);
                    }
                }
            }
        }
        results
    }

    /// Query all memories within a time window (no entity filter).
    pub fn query_time_window(&self, start_ms: u64, end_ms: u64) -> Vec<&IndexEntry> {
        let mut results = Vec::new();
        for (_, ids) in self.by_time.range(start_ms..=end_ms) {
            for id in ids {
                if let Some(entry) = self.entries.get(id) {
                    results.push(entry);
                }
            }
        }
        results
    }

    /// Trace root causes: find memories in a time window related to an entity,
    /// then walk causal chains backward to find the originating events.
    pub fn trace_root_causes(
        &self,
        entity: &str,
        start_ms: u64,
        end_ms: u64,
        causal_graph: &CausalGraph,
    ) -> Vec<String> {
        let hits = self.query_entity_in_window(entity, start_ms, end_ms);
        let mut roots = HashSet::new();

        for hit in hits {
            let root = causal_graph.root_cause(&hit.memory_id);
            roots.insert(root);
        }

        roots.into_iter().collect()
    }

    /// Get the total number of indexed entries.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get the total number of unique entities.
    pub fn entity_count(&self) -> usize {
        self.by_entity.len()
    }
}

/// Extract entities from a MemoryEntry — uses tags plus simple content tokenization.
fn extract_entities(entry: &MemoryEntry) -> Vec<String> {
    let mut entities: Vec<String> = entry.tags.iter().map(|t| t.to_lowercase()).collect();

    let content = entry.content.display();
    let words: Vec<String> = content
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase().trim_matches(|c: char| !c.is_alphanumeric()).to_string())
        .filter(|w| !w.is_empty())
        .collect();

    for word in words {
        if !entities.contains(&word) {
            entities.push(word);
        }
    }

    entities
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::MemoryEntry;

    fn make_entry(id: &str, content: &str, tags: Vec<&str>, created_at: u64, causal_parent: Option<&str>) -> MemoryEntry {
        let mut e = MemoryEntry::ephemeral("test-agent", content);
        e.id = id.to_string();
        e.tags = tags.into_iter().map(|t| t.to_string()).collect();
        e.created_at = created_at;
        e.causal_parent = causal_parent.map(|s| s.to_string());
        e
    }

    #[test]
    fn test_build_index() {
        let entries = vec![
            make_entry("m1", "deploy to production", vec!["deploy"], 1000, None),
            make_entry("m2", "rollback happened", vec!["deploy", "rollback"], 2000, Some("m1")),
        ];
        let index = TemporalCausalIndex::build(&entries);
        assert_eq!(index.entry_count(), 2);
        assert!(index.entity_count() > 0);
    }

    #[test]
    fn test_query_entity_in_window() {
        let entries = vec![
            make_entry("m1", "deploy v1", vec!["deploy"], 1000, None),
            make_entry("m2", "deploy v2", vec!["deploy"], 3000, None),
            make_entry("m3", "test passed", vec!["testing"], 2000, None),
        ];
        let index = TemporalCausalIndex::build(&entries);

        let results = index.query_entity_in_window("deploy", 0, 2000);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, "m1");

        let results_all = index.query_entity_in_window("deploy", 0, 5000);
        assert_eq!(results_all.len(), 2);
    }

    #[test]
    fn test_query_time_window() {
        let entries = vec![
            make_entry("m1", "event A", vec![], 1000, None),
            make_entry("m2", "event B", vec![], 2000, None),
            make_entry("m3", "event C", vec![], 3000, None),
        ];
        let index = TemporalCausalIndex::build(&entries);

        let results = index.query_time_window(1500, 2500);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory_id, "m2");
    }

    #[test]
    fn test_trace_root_causes() {
        let entries = vec![
            make_entry("root", "config changed", vec!["config"], 1000, None),
            make_entry("mid", "deploy failed", vec!["deploy"], 2000, Some("root")),
            make_entry("leaf", "user reported error", vec!["error", "deploy"], 3000, Some("mid")),
        ];
        let index = TemporalCausalIndex::build(&entries);
        let graph = CausalGraph::build(&entries);

        let roots = index.trace_root_causes("deploy", 2000, 4000, &graph);
        assert!(roots.contains(&"root".to_string()), "should trace back to config change");
    }

    #[test]
    fn test_trace_orphan_is_own_root() {
        let entries = vec![
            make_entry("orphan", "standalone event", vec!["event"], 1000, None),
        ];
        let index = TemporalCausalIndex::build(&entries);
        let graph = CausalGraph::build(&entries);

        let roots = index.trace_root_causes("event", 0, 2000, &graph);
        assert_eq!(roots.len(), 1);
        assert!(roots.contains(&"orphan".to_string()));
    }

    #[test]
    fn test_entity_extraction_from_tags_and_content() {
        let entry = make_entry("m1", "deploy production server", vec!["ci"], 1000, None);
        let entities = extract_entities(&entry);
        assert!(entities.contains(&"ci".to_string()));
        assert!(entities.contains(&"deploy".to_string()));
        assert!(entities.contains(&"production".to_string()));
        assert!(entities.contains(&"server".to_string()));
    }

    #[test]
    fn test_empty_index() {
        let index = TemporalCausalIndex::build(&[]);
        assert_eq!(index.entry_count(), 0);
        assert!(index.query_time_window(0, u64::MAX).is_empty());
    }

    #[test]
    fn test_case_insensitive_entity_query() {
        let entries = vec![
            make_entry("m1", "Deploy Production", vec!["Deploy"], 1000, None),
        ];
        let index = TemporalCausalIndex::build(&entries);

        let results = index.query_entity_in_window("deploy", 0, 2000);
        assert_eq!(results.len(), 1, "entity query should be case-insensitive");
    }
}
