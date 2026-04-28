//! Active Forgetting — TTL decay, semantic dedup, contradiction detection.
//!
//! Three-dimensional forgetting strategy (OS mechanisms, agent-configurable):
//! - TTL decay: different MemoryType defaults (Episodic: 7d, Semantic/Procedural: permanent)
//! - Semantic dedup: cosine similarity >threshold → merge instead of add
//! - Contradiction detection: same-entity new/old conflicts → mark old as superseded

use crate::memory::layered::{MemoryEntry, MemoryType, now_ms};

/// Default TTL values by memory type (in milliseconds).
pub fn default_ttl_ms(memory_type: MemoryType) -> Option<u64> {
    match memory_type {
        MemoryType::Episodic => Some(7 * 24 * 60 * 60 * 1000), // 7 days
        MemoryType::Semantic => None,
        MemoryType::Procedural => None,
        MemoryType::Untyped => Some(30 * 24 * 60 * 60 * 1000), // 30 days
    }
}

/// Apply default TTL to a memory entry if it doesn't already have one.
pub fn apply_default_ttl(entry: &mut MemoryEntry) {
    if entry.ttl_ms.is_none() {
        if let Some(ttl) = default_ttl_ms(entry.memory_type) {
            entry.ttl_ms = Some(ttl);
            entry.original_ttl_ms = Some(ttl);
        }
    }
}

/// Result of a dedup check.
#[derive(Debug, PartialEq)]
pub enum DedupResult {
    /// No duplicate found, store as new.
    Unique,
    /// Found a near-duplicate (id of existing entry, similarity score).
    Duplicate { existing_id: String, similarity: f32 },
}

/// Check if a new entry is a semantic duplicate of any existing entry.
///
/// Returns `DedupResult::Duplicate` if cosine similarity with any same-type
/// entry exceeds the threshold.
pub fn check_semantic_dedup(
    new_embedding: &[f32],
    new_memory_type: MemoryType,
    existing: &[MemoryEntry],
    threshold: f32,
) -> DedupResult {
    let mut best_match: Option<(&str, f32)> = None;

    for entry in existing {
        if entry.memory_type != new_memory_type && new_memory_type != MemoryType::Untyped {
            continue;
        }
        if let Some(ref emb) = entry.embedding {
            let sim = cosine_similarity(new_embedding, emb);
            if sim > threshold {
                match best_match {
                    None => { best_match = Some((&entry.id, sim)); }
                    Some((_, prev_sim)) if sim > prev_sim => { best_match = Some((&entry.id, sim)); }
                    _ => {}
                }
            }
        }
    }

    match best_match {
        Some((id, similarity)) => DedupResult::Duplicate {
            existing_id: id.to_string(),
            similarity,
        },
        None => DedupResult::Unique,
    }
}

/// Stub dedup check when embedding is unavailable — exact content hash comparison.
pub fn check_exact_dedup(new_content: &str, existing: &[MemoryEntry]) -> DedupResult {
    for entry in existing {
        if entry.content.display().to_string() == new_content {
            return DedupResult::Duplicate {
                existing_id: entry.id.clone(),
                similarity: 1.0,
            };
        }
    }
    DedupResult::Unique
}

/// Result of a contradiction check.
#[derive(Debug, PartialEq)]
pub enum ContradictionResult {
    /// No contradiction found.
    NoConflict,
    /// Potential contradiction with existing entry (LLM confirmed or high keyword overlap).
    Conflict {
        existing_id: String,
        confidence: f32,
    },
}

/// Check for contradictions using keyword overlap heuristic (rule-based fallback).
///
/// Detects when two entries about the same entity contain conflicting information.
/// Works by finding entries with high tag overlap but different content.
pub fn check_contradiction_rules(
    new_entry: &MemoryEntry,
    existing: &[MemoryEntry],
    min_tag_overlap: usize,
) -> ContradictionResult {
    if new_entry.tags.is_empty() {
        return ContradictionResult::NoConflict;
    }

    let new_content = new_entry.content.display().to_string().to_lowercase();

    for entry in existing {
        if entry.id == new_entry.id {
            continue;
        }
        if entry.memory_type != new_entry.memory_type {
            continue;
        }

        let overlap = new_entry.tags.iter()
            .filter(|t| entry.tags.contains(t))
            .count();

        if overlap >= min_tag_overlap {
            let old_content = entry.content.display().to_string().to_lowercase();
            if old_content != new_content {
                let confidence = overlap as f32 / new_entry.tags.len().max(1) as f32;
                if confidence >= 0.5 {
                    return ContradictionResult::Conflict {
                        existing_id: entry.id.clone(),
                        confidence,
                    };
                }
            }
        }
    }

    ContradictionResult::NoConflict
}

/// Build an LLM prompt for contradiction detection.
pub fn contradiction_prompt(old_content: &str, new_content: &str) -> String {
    format!(
        "Two statements are contradictory if they assign DIFFERENT values to the SAME attribute \
         (e.g. different dates, versions, names, numbers, or choices for the same subject). \
         Even if phrased differently ('required' vs 'recommended'), conflicting specifics count.\n\n\
         Statement A: {old_content}\n\
         Statement B: {new_content}\n\n\
         Do these two statements contradict each other? Answer ONLY 'yes' or 'no'.\n\
         Answer:"
    )
}

/// Parse LLM contradiction response.
pub fn parse_contradiction_response(response: &str) -> bool {
    let r = response.trim().to_lowercase();
    r.starts_with("yes")
}

/// Collect expired entries (those past their TTL).
pub fn find_expired(entries: &[MemoryEntry]) -> Vec<String> {
    let now = now_ms();
    entries.iter()
        .filter(|e| {
            if let Some(ttl) = e.ttl_ms {
                e.created_at.saturating_add(ttl) < now
            } else {
                false
            }
        })
        .map(|e| e.id.clone())
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryContent, MemoryTier, MemoryScope};

    fn make_entry(id: &str, content: &str, mem_type: MemoryType, tags: Vec<String>, embedding: Option<Vec<f32>>) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            agent_id: "test-agent".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content.to_string()),
            importance: 50,
            access_count: 0,
            last_accessed: now_ms(),
            created_at: now_ms(),
            tags,
            embedding,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: mem_type,
            causal_parent: None,
            supersedes: None,
        }
    }

    // ─── TTL Defaults ─────────────────────────────────────────

    #[test]
    fn test_default_ttl_episodic() {
        let ttl = default_ttl_ms(MemoryType::Episodic);
        assert_eq!(ttl, Some(7 * 24 * 60 * 60 * 1000));
    }

    #[test]
    fn test_default_ttl_semantic_is_permanent() {
        assert_eq!(default_ttl_ms(MemoryType::Semantic), None);
    }

    #[test]
    fn test_default_ttl_procedural_is_permanent() {
        assert_eq!(default_ttl_ms(MemoryType::Procedural), None);
    }

    #[test]
    fn test_apply_default_ttl_sets_episodic() {
        let mut entry = make_entry("e1", "event", MemoryType::Episodic, vec![], None);
        assert!(entry.ttl_ms.is_none());
        apply_default_ttl(&mut entry);
        assert!(entry.ttl_ms.is_some());
        assert_eq!(entry.ttl_ms, entry.original_ttl_ms);
    }

    #[test]
    fn test_apply_default_ttl_preserves_existing() {
        let mut entry = make_entry("e1", "event", MemoryType::Episodic, vec![], None);
        entry.ttl_ms = Some(999);
        entry.original_ttl_ms = Some(999);
        apply_default_ttl(&mut entry);
        assert_eq!(entry.ttl_ms, Some(999));
    }

    // ─── Semantic Dedup ──────────────────────────────────────

    #[test]
    fn test_semantic_dedup_unique() {
        let emb_a = vec![1.0, 0.0, 0.0];
        let emb_b = vec![0.0, 1.0, 0.0];
        let existing = vec![
            make_entry("e1", "old", MemoryType::Semantic, vec![], Some(emb_b)),
        ];
        let result = check_semantic_dedup(&emb_a, MemoryType::Semantic, &existing, 0.85);
        assert_eq!(result, DedupResult::Unique);
    }

    #[test]
    fn test_semantic_dedup_duplicate() {
        let emb = vec![1.0, 0.0, 0.0];
        let existing = vec![
            make_entry("e1", "same direction", MemoryType::Semantic, vec![], Some(vec![0.99, 0.01, 0.0])),
        ];
        let result = check_semantic_dedup(&emb, MemoryType::Semantic, &existing, 0.85);
        assert!(matches!(result, DedupResult::Duplicate { .. }));
    }

    #[test]
    fn test_exact_dedup_found() {
        let existing = vec![
            make_entry("e1", "hello world", MemoryType::Semantic, vec![], None),
        ];
        let result = check_exact_dedup("hello world", &existing);
        assert!(matches!(result, DedupResult::Duplicate { .. }));
    }

    #[test]
    fn test_exact_dedup_not_found() {
        let existing = vec![
            make_entry("e1", "hello world", MemoryType::Semantic, vec![], None),
        ];
        let result = check_exact_dedup("different text", &existing);
        assert_eq!(result, DedupResult::Unique);
    }

    // ─── Contradiction Detection ────────────────────────────

    #[test]
    fn test_contradiction_rules_no_conflict() {
        let new = make_entry("new", "user prefers dark mode", MemoryType::Semantic,
            vec!["user".to_string(), "preference".to_string()], None);
        let existing = vec![
            make_entry("old", "user prefers dark mode", MemoryType::Semantic,
                vec!["user".to_string(), "preference".to_string()], None),
        ];
        let result = check_contradiction_rules(&new, &existing, 2);
        assert_eq!(result, ContradictionResult::NoConflict);
    }

    #[test]
    fn test_contradiction_rules_conflict_detected() {
        let new = make_entry("new", "user prefers light mode", MemoryType::Semantic,
            vec!["user".to_string(), "preference".to_string()], None);
        let existing = vec![
            make_entry("old", "user prefers dark mode", MemoryType::Semantic,
                vec!["user".to_string(), "preference".to_string()], None),
        ];
        let result = check_contradiction_rules(&new, &existing, 2);
        assert!(matches!(result, ContradictionResult::Conflict { .. }));
    }

    #[test]
    fn test_contradiction_rules_no_tags() {
        let new = make_entry("new", "something", MemoryType::Semantic, vec![], None);
        let existing = vec![
            make_entry("old", "something else", MemoryType::Semantic, vec![], None),
        ];
        let result = check_contradiction_rules(&new, &existing, 1);
        assert_eq!(result, ContradictionResult::NoConflict);
    }

    // ─── LLM Contradiction Prompt ───────────────────────────

    #[test]
    fn test_contradiction_prompt_format() {
        let prompt = contradiction_prompt("A is true", "A is false");
        assert!(prompt.contains("A is true"));
        assert!(prompt.contains("A is false"));
        assert!(prompt.contains("contradict"));
    }

    #[test]
    fn test_parse_contradiction_response_yes() {
        assert!(parse_contradiction_response("yes"));
        assert!(parse_contradiction_response("Yes, they contradict"));
    }

    #[test]
    fn test_parse_contradiction_response_no() {
        assert!(!parse_contradiction_response("no"));
        assert!(!parse_contradiction_response("No, they are consistent"));
    }

    // ─── Expired Entries ────────────────────────────────────

    #[test]
    fn test_find_expired_entries() {
        let mut expired = make_entry("e1", "old", MemoryType::Episodic, vec![], None);
        expired.created_at = 1000;
        expired.ttl_ms = Some(1); // expired long ago

        let fresh = make_entry("e2", "new", MemoryType::Semantic, vec![], None);

        let result = find_expired(&[expired, fresh]);
        assert_eq!(result, vec!["e1".to_string()]);
    }

    #[test]
    fn test_find_expired_none_when_no_ttl() {
        let entry = make_entry("e1", "perm", MemoryType::Semantic, vec![], None);
        let result = find_expired(&[entry]);
        assert!(result.is_empty());
    }
}
