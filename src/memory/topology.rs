//! Memory Topology Evolver — Split/Merge/Update operations for self-evolving memory.
//!
//! Three topology operations enable the memory graph to restructure itself:
//! - **Split**: One memory serving diverse intents → multiple specialized memories
//! - **Merge**: Two similar same-type memories → one consolidated memory
//! - **Update**: Contradiction detected → fuse new information into existing memory
//!
//! All operations have CPU-only rule-based paths and optional LLM-enhanced paths.

use crate::memory::layered::{MemoryEntry, MemoryContent, MemoryType, MemoryTier, MemoryScope};
use crate::fs::retrieval_router::QueryIntent;
use std::collections::HashMap;
use uuid::Uuid;

/// Record of which QueryIntent hit a given memory.
#[derive(Debug, Clone)]
pub struct IntentHitRecord {
    pub memory_id: String,
    pub hits: HashMap<QueryIntent, u32>,
}

impl IntentHitRecord {
    pub fn new(memory_id: &str) -> Self {
        Self {
            memory_id: memory_id.to_string(),
            hits: HashMap::new(),
        }
    }

    pub fn record_hit(&mut self, intent: QueryIntent) {
        *self.hits.entry(intent).or_insert(0) += 1;
    }

    pub fn total_hits(&self) -> u32 {
        self.hits.values().sum()
    }

    /// Shannon entropy of the intent distribution. High entropy = diverse usage.
    pub fn entropy(&self) -> f64 {
        let total = self.total_hits() as f64;
        if total == 0.0 {
            return 0.0;
        }
        let mut h = 0.0;
        for &count in self.hits.values() {
            if count > 0 {
                let p = count as f64 / total;
                h -= p * p.ln();
            }
        }
        h
    }
}

/// Result of a topology evolution pass.
#[derive(Debug, Clone)]
pub struct TopologyAction {
    pub kind: ActionKind,
    pub source_ids: Vec<String>,
    pub result_entries: Vec<MemoryEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Split,
    Merge,
    Update,
}

// ─── Split ─────────────────────────────────────────────────────

const SPLIT_ENTROPY_THRESHOLD: f64 = 1.0;
const SPLIT_MIN_HITS: u32 = 4;

/// Determine if a memory should be split based on intent hit diversity.
pub fn should_split(record: &IntentHitRecord) -> bool {
    record.total_hits() >= SPLIT_MIN_HITS && record.entropy() >= SPLIT_ENTROPY_THRESHOLD
}

/// Split a memory into per-MemoryType variants (rule-based, no LLM).
///
/// Returns new entries — one per intent type that had hits, each with the
/// same content but typed accordingly.
pub fn split_by_intent(
    entry: &MemoryEntry,
    record: &IntentHitRecord,
) -> Vec<MemoryEntry> {
    let intent_to_type = |intent: &QueryIntent| -> MemoryType {
        match intent {
            QueryIntent::Factual => MemoryType::Semantic,
            QueryIntent::Temporal => MemoryType::Episodic,
            QueryIntent::MultiHop => MemoryType::Semantic,
            QueryIntent::Preference => MemoryType::Semantic,
            QueryIntent::Aggregation => MemoryType::Semantic,
        }
    };

    let mut type_hits: HashMap<MemoryType, u32> = HashMap::new();
    for (intent, &count) in &record.hits {
        *type_hits.entry(intent_to_type(intent)).or_insert(0) += count;
    }

    if type_hits.len() <= 1 {
        return vec![];
    }

    type_hits
        .into_iter()
        .map(|(mem_type, _)| {
            let mut new_entry = entry.clone();
            new_entry.id = Uuid::new_v4().to_string();
            new_entry.memory_type = mem_type;
            new_entry.causal_parent = Some(entry.id.clone());
            new_entry.importance = entry.importance.saturating_sub(10);
            new_entry
        })
        .collect()
}

/// Split using LLM-generated content. The `llm_fn` takes a prompt and returns
/// generated text, or None if LLM is unavailable.
pub fn split_with_llm(
    entry: &MemoryEntry,
    record: &IntentHitRecord,
    llm_fn: impl Fn(&str) -> Option<String>,
) -> Vec<MemoryEntry> {
    let prompt = split_prompt(entry, record);
    match llm_fn(&prompt) {
        Some(response) => parse_split_response(entry, &response),
        None => split_by_intent(entry, record),
    }
}

pub fn split_prompt(entry: &MemoryEntry, record: &IntentHitRecord) -> String {
    let hits: Vec<String> = record
        .hits
        .iter()
        .map(|(intent, count)| format!("  {:?}: {} hits", intent, count))
        .collect();
    format!(
        "This memory is used by multiple intent types:\n\
         Content: \"{}\"\n\
         Intent hits:\n{}\n\n\
         Split this into separate memories, one per intent type.\n\
         Output format: one line per split, format: TYPE|CONTENT\n\
         Types: episodic, semantic, procedural",
        entry.content.display(),
        hits.join("\n")
    )
}

pub fn parse_split_response(entry: &MemoryEntry, response: &str) -> Vec<MemoryEntry> {
    let mut results = Vec::new();
    for line in response.lines() {
        let line = line.trim();
        if let Some((type_str, content)) = line.split_once('|') {
            let mem_type = MemoryType::from_str_loose(type_str.trim());
            let mut new_entry = entry.clone();
            new_entry.id = Uuid::new_v4().to_string();
            new_entry.memory_type = mem_type;
            new_entry.content = MemoryContent::Text(content.trim().to_string());
            new_entry.causal_parent = Some(entry.id.clone());
            results.push(new_entry);
        }
    }
    if results.is_empty() {
        split_by_intent(entry, &IntentHitRecord::new(&entry.id))
    } else {
        results
    }
}

// ─── Merge ─────────────────────────────────────────────────────

const MERGE_SIMILARITY_THRESHOLD: f32 = 0.9;

/// Check if two entries should be merged (same type, high similarity).
pub fn should_merge(a: &MemoryEntry, b: &MemoryEntry) -> bool {
    if a.memory_type != b.memory_type {
        return false;
    }
    if a.id == b.id {
        return false;
    }
    match (&a.embedding, &b.embedding) {
        (Some(ea), Some(eb)) => cosine_similarity(ea, eb) >= MERGE_SIMILARITY_THRESHOLD,
        _ => false,
    }
}

/// Merge two entries (rule-based): keep the one with higher access_count,
/// union the tags, and set causal_parent.
pub fn merge_entries(a: &MemoryEntry, b: &MemoryEntry) -> MemoryEntry {
    let (winner, loser) = if a.access_count >= b.access_count {
        (a, b)
    } else {
        (b, a)
    };

    let mut merged = winner.clone();
    merged.id = Uuid::new_v4().to_string();
    for tag in &loser.tags {
        if !merged.tags.contains(tag) {
            merged.tags.push(tag.clone());
        }
    }
    merged.importance = merged.importance.max(loser.importance);
    merged.access_count = merged.access_count.saturating_add(loser.access_count);
    merged.causal_parent = Some(winner.id.clone());
    merged.supersedes = Some(loser.id.clone());
    merged
}

/// Merge with LLM to synthesize better content.
pub fn merge_with_llm(
    a: &MemoryEntry,
    b: &MemoryEntry,
    llm_fn: impl Fn(&str) -> Option<String>,
) -> MemoryEntry {
    let prompt = merge_prompt(a, b);
    let mut merged = merge_entries(a, b);
    if let Some(response) = llm_fn(&prompt) {
        let text = response.trim().to_string();
        if !text.is_empty() {
            merged.content = MemoryContent::Text(text);
        }
    }
    merged
}

pub fn merge_prompt(a: &MemoryEntry, b: &MemoryEntry) -> String {
    format!(
        "Merge these two similar memories into one concise memory:\n\
         Memory A: \"{}\"\n\
         Memory B: \"{}\"\n\n\
         Output ONLY the merged content, nothing else.",
        a.content.display(),
        b.content.display()
    )
}

// ─── Update (Contradiction Resolution) ─────────────────────────

/// Update old memory with new info (rule-based): halve old importance,
/// new entry supersedes old.
pub fn update_on_contradiction(
    old: &MemoryEntry,
    new_entry: &MemoryEntry,
) -> (MemoryEntry, MemoryEntry) {
    let mut updated_old = old.clone();
    updated_old.importance = updated_old.importance / 2;

    let mut updated_new = new_entry.clone();
    updated_new.supersedes = Some(old.id.clone());

    (updated_old, updated_new)
}

/// Update with LLM to fuse contradictory info into a single entry.
pub fn update_with_llm(
    old: &MemoryEntry,
    new_entry: &MemoryEntry,
    llm_fn: impl Fn(&str) -> Option<String>,
) -> MemoryEntry {
    let prompt = update_prompt(old, new_entry);
    let mut fused = new_entry.clone();
    fused.supersedes = Some(old.id.clone());

    if let Some(response) = llm_fn(&prompt) {
        let text = response.trim().to_string();
        if !text.is_empty() {
            fused.content = MemoryContent::Text(text);
        }
    }
    fused.importance = fused.importance.max(old.importance);
    fused
}

pub fn update_prompt(old: &MemoryEntry, new_entry: &MemoryEntry) -> String {
    format!(
        "Two memories contain contradictory information:\n\
         Old: \"{}\"\n\
         New: \"{}\"\n\n\
         Fuse them into a single accurate memory. Output ONLY the fused content.",
        old.content.display(),
        new_entry.content.display()
    )
}

// ─── Cross-Agent Merge ─────────────────────────────────────────

/// Find merge candidates across agents: same MemoryType, high similarity,
/// different agent_id. Returns pairs of (agent_a_entry, agent_b_entry).
pub fn find_cross_agent_merge_candidates<'a>(
    entries: &'a [MemoryEntry],
) -> Vec<(&'a MemoryEntry, &'a MemoryEntry)> {
    let mut candidates = Vec::new();
    for i in 0..entries.len() {
        for j in (i + 1)..entries.len() {
            let a = &entries[i];
            let b = &entries[j];
            if a.agent_id != b.agent_id && should_merge(a, b) {
                candidates.push((a, b));
            }
        }
    }
    candidates
}

/// Merge two cross-agent entries into a Shared-scope memory.
pub fn cross_agent_merge(a: &MemoryEntry, b: &MemoryEntry) -> MemoryEntry {
    let mut merged = merge_entries(a, b);
    merged.scope = MemoryScope::Shared;
    merged.agent_id = a.agent_id.clone();
    merged
}

// ─── Full Topology Evolution Pass ──────────────────────────────

/// Run a full topology evolution pass over a set of entries.
///
/// Returns the list of actions taken. Caller is responsible for applying them
/// (storing new entries, removing old ones).
pub fn evolve_topology(
    entries: &[MemoryEntry],
    hit_records: &HashMap<String, IntentHitRecord>,
    llm_fn: impl Fn(&str) -> Option<String>,
) -> Vec<TopologyAction> {
    let mut actions = Vec::new();

    // 1. Check for splits
    for entry in entries {
        if let Some(record) = hit_records.get(&entry.id) {
            if should_split(record) {
                let new_entries = split_with_llm(entry, record, &llm_fn);
                if !new_entries.is_empty() {
                    actions.push(TopologyAction {
                        kind: ActionKind::Split,
                        source_ids: vec![entry.id.clone()],
                        result_entries: new_entries,
                    });
                }
            }
        }
    }

    // 2. Check for merges (including cross-agent)
    let mut merged_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for i in 0..entries.len() {
        if merged_ids.contains(&entries[i].id) {
            continue;
        }
        for j in (i + 1)..entries.len() {
            if merged_ids.contains(&entries[j].id) {
                continue;
            }
            let a = &entries[i];
            let b = &entries[j];
            if should_merge(a, b) {
                let merged = if a.agent_id != b.agent_id {
                    cross_agent_merge(a, b)
                } else {
                    merge_with_llm(a, b, &llm_fn)
                };
                merged_ids.insert(a.id.clone());
                merged_ids.insert(b.id.clone());
                actions.push(TopologyAction {
                    kind: ActionKind::Merge,
                    source_ids: vec![a.id.clone(), b.id.clone()],
                    result_entries: vec![merged],
                });
            }
        }
    }

    actions
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_entry(id: &str, agent: &str, content: &str, mem_type: MemoryType) -> MemoryEntry {
        let mut e = MemoryEntry::ephemeral(agent, content);
        e.id = id.to_string();
        e.memory_type = mem_type;
        e.tier = MemoryTier::LongTerm;
        e
    }

    fn make_entry_with_embedding(
        id: &str, agent: &str, content: &str,
        mem_type: MemoryType, embedding: Vec<f32>,
    ) -> MemoryEntry {
        let mut e = make_entry(id, agent, content, mem_type);
        e.embedding = Some(embedding);
        e
    }

    // ─── Split Tests ───────────────────────────────────────────

    #[test]
    fn test_should_split_low_entropy() {
        let mut record = IntentHitRecord::new("m1");
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Factual);
        assert!(!should_split(&record), "uniform single-intent should not split");
    }

    #[test]
    fn test_should_split_high_entropy() {
        let mut record = IntentHitRecord::new("m1");
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Temporal);
        record.record_hit(QueryIntent::MultiHop);
        record.record_hit(QueryIntent::Preference);
        assert!(should_split(&record), "diverse intents should trigger split");
    }

    #[test]
    fn test_split_by_intent_produces_typed_entries() {
        let entry = make_entry("m1", "agent-a", "user prefers dark mode and joined last week", MemoryType::Untyped);
        let mut record = IntentHitRecord::new("m1");
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Factual);
        record.record_hit(QueryIntent::Temporal);
        record.record_hit(QueryIntent::Temporal);

        let splits = split_by_intent(&entry, &record);
        assert!(splits.len() >= 2, "should split into at least 2 entries");
        let types: Vec<MemoryType> = splits.iter().map(|e| e.memory_type).collect();
        assert!(types.contains(&MemoryType::Semantic));
        assert!(types.contains(&MemoryType::Episodic));
        for s in &splits {
            assert_eq!(s.causal_parent.as_deref(), Some("m1"));
        }
    }

    // ─── Merge Tests ───────────────────────────────────────────

    #[test]
    fn test_should_merge_same_type_high_similarity() {
        let a = make_entry_with_embedding(
            "a", "agent-a", "user likes coffee",
            MemoryType::Semantic, vec![1.0, 0.0, 0.0],
        );
        let b = make_entry_with_embedding(
            "b", "agent-a", "user enjoys coffee",
            MemoryType::Semantic, vec![0.99, 0.1, 0.0],
        );
        assert!(should_merge(&a, &b));
    }

    #[test]
    fn test_should_not_merge_different_types() {
        let a = make_entry_with_embedding(
            "a", "agent-a", "meeting at 3pm",
            MemoryType::Episodic, vec![1.0, 0.0, 0.0],
        );
        let b = make_entry_with_embedding(
            "b", "agent-a", "meeting at 3pm",
            MemoryType::Semantic, vec![1.0, 0.0, 0.0],
        );
        assert!(!should_merge(&a, &b));
    }

    #[test]
    fn test_merge_entries_union_tags() {
        let mut a = make_entry_with_embedding(
            "a", "agent-a", "user likes coffee",
            MemoryType::Semantic, vec![1.0, 0.0],
        );
        a.tags = vec!["preference".into(), "beverage".into()];
        a.access_count = 5;

        let mut b = make_entry_with_embedding(
            "b", "agent-a", "user enjoys coffee",
            MemoryType::Semantic, vec![0.99, 0.1],
        );
        b.tags = vec!["preference".into(), "coffee".into()];
        b.access_count = 3;

        let merged = merge_entries(&a, &b);
        assert!(merged.tags.contains(&"beverage".to_string()));
        assert!(merged.tags.contains(&"coffee".to_string()));
        assert!(merged.tags.contains(&"preference".to_string()));
        assert_eq!(merged.access_count, 8);
        assert_eq!(merged.causal_parent.as_deref(), Some("a"));
        assert_eq!(merged.supersedes.as_deref(), Some("b"));
    }

    // ─── Update Tests ──────────────────────────────────────────

    #[test]
    fn test_update_on_contradiction_halves_importance() {
        let old = make_entry("old", "agent-a", "user prefers dark mode", MemoryType::Semantic);
        let new_entry = make_entry("new", "agent-a", "user prefers light mode", MemoryType::Semantic);

        let (updated_old, updated_new) = update_on_contradiction(&old, &new_entry);
        assert_eq!(updated_old.importance, 25);
        assert_eq!(updated_new.supersedes.as_deref(), Some("old"));
    }

    #[test]
    fn test_update_with_llm_fuses_content() {
        let old = make_entry("old", "agent-a", "user prefers dark mode", MemoryType::Semantic);
        let new_entry = make_entry("new", "agent-a", "user prefers light mode", MemoryType::Semantic);

        let fused = update_with_llm(&old, &new_entry, |_| {
            Some("user switched preference from dark to light mode".to_string())
        });
        assert!(fused.content.display().contains("switched"));
        assert_eq!(fused.supersedes.as_deref(), Some("old"));
    }

    // ─── Cross-Agent Merge Tests ───────────────────────────────

    #[test]
    fn test_cross_agent_merge_candidates() {
        let a = make_entry_with_embedding(
            "a", "agent-a", "deploy uses CI pipeline",
            MemoryType::Procedural, vec![1.0, 0.0, 0.0],
        );
        let b = make_entry_with_embedding(
            "b", "agent-b", "deployment through CI",
            MemoryType::Procedural, vec![0.99, 0.1, 0.0],
        );
        let c = make_entry_with_embedding(
            "c", "agent-c", "unrelated memory",
            MemoryType::Episodic, vec![0.0, 0.0, 1.0],
        );

        let entries = [a, b, c];
        let candidates = find_cross_agent_merge_candidates(&entries);
        assert_eq!(candidates.len(), 1);
        assert_ne!(candidates[0].0.agent_id, candidates[0].1.agent_id);
    }

    #[test]
    fn test_cross_agent_merge_produces_shared_scope() {
        let a = make_entry_with_embedding(
            "a", "agent-a", "deploy uses CI pipeline",
            MemoryType::Procedural, vec![1.0, 0.0],
        );
        let b = make_entry_with_embedding(
            "b", "agent-b", "deployment through CI",
            MemoryType::Procedural, vec![0.99, 0.1],
        );

        let merged = cross_agent_merge(&a, &b);
        assert_eq!(merged.scope, MemoryScope::Shared);
    }
}
