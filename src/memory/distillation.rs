//! Memory Distillation — compress Working Memory into LongTerm at session end.
//!
//! EndSession triggers: Working Memory fragments → grouped by MemoryType →
//! merged/summarized → written to LongTerm → original Working entries removed.
//!
//! LLM-first for summarization, rule-based fallback (concatenate + dedup).

use crate::memory::layered::{MemoryEntry, MemoryType, MemoryContent, MemoryTier, now_ms};
use crate::memory::MemoryScope;
use std::collections::HashMap;

/// A distilled memory ready to be stored in LongTerm.
#[derive(Debug, Clone)]
pub struct DistilledEntry {
    pub content: String,
    pub memory_type: MemoryType,
    pub tags: Vec<String>,
    pub importance: u8,
    pub source_ids: Vec<String>,
}

/// Group working memory entries by MemoryType and produce distilled entries.
///
/// Uses `summarizer` callback for LLM-powered compression; falls back to
/// rule-based concatenation when the callback returns None.
pub fn distill_working_memory(
    entries: &[MemoryEntry],
    summarizer: impl Fn(&str) -> Option<String>,
) -> Vec<DistilledEntry> {
    if entries.is_empty() {
        return Vec::new();
    }

    let mut by_type: HashMap<MemoryType, Vec<&MemoryEntry>> = HashMap::new();
    for entry in entries {
        by_type.entry(entry.memory_type).or_default().push(entry);
    }

    let mut distilled = Vec::new();

    for (mem_type, group) in by_type {
        if group.is_empty() {
            continue;
        }

        let source_ids: Vec<String> = group.iter().map(|e| e.id.clone()).collect();
        let all_tags: Vec<String> = group.iter()
            .flat_map(|e| e.tags.iter().cloned())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let max_importance = group.iter().map(|e| e.importance).max().unwrap_or(50);

        let combined_text = group.iter()
            .map(|e| e.content.display().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        let content = match summarizer(&combined_text) {
            Some(summary) => summary,
            None => rule_based_merge(&group),
        };

        distilled.push(DistilledEntry {
            content,
            memory_type: mem_type,
            tags: all_tags,
            importance: max_importance,
            source_ids,
        });
    }

    distilled
}

/// Rule-based merge: deduplicate, concatenate, and truncate.
fn rule_based_merge(entries: &[&MemoryEntry]) -> String {
    let mut seen = std::collections::HashSet::new();
    let mut parts = Vec::new();

    for entry in entries {
        let text = entry.content.display().to_string();
        if seen.insert(text.clone()) {
            parts.push(text);
        }
    }

    let merged = parts.join(" | ");
    if merged.len() > 2000 {
        format!("{}...", &merged[..2000])
    } else {
        merged
    }
}

/// Convert a DistilledEntry into a MemoryEntry for LongTerm storage.
pub fn to_long_term_entry(
    distilled: &DistilledEntry,
    agent_id: &str,
    tenant_id: &str,
) -> MemoryEntry {
    MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent_id.to_string(),
        tenant_id: tenant_id.to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text(distilled.content.clone()),
        importance: distilled.importance,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: distilled.tags.clone(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: distilled.memory_type,
        causal_parent: None,
        supersedes: None,
    }
}

/// Build an LLM prompt for session summarization.
pub fn summarization_prompt(entries_text: &str) -> String {
    format!(
        "Compress these memories into the SHORTEST possible summary (fewer words than the input). \
         Keep only key facts, decisions, and action items. Remove filler and redundancy. \
         Output ONLY the summary, nothing else.\n\n\
         Memories:\n{entries_text}\n\n\
         Summary:"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::now_ms;

    fn make_working_entry(id: &str, content: &str, mem_type: MemoryType, tags: Vec<String>) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            agent_id: "test-agent".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Working,
            content: MemoryContent::Text(content.to_string()),
            importance: 50,
            access_count: 0,
            last_accessed: now_ms(),
            created_at: now_ms(),
            tags,
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: mem_type,
            causal_parent: None,
            supersedes: None,
        }
    }

    #[test]
    fn test_distill_empty() {
        let result = distill_working_memory(&[], |_| None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_distill_single_entry_rule_based() {
        let entries = vec![
            make_working_entry("e1", "user discussed project alpha", MemoryType::Episodic, vec!["alpha".to_string()]),
        ];
        let result = distill_working_memory(&entries, |_| None);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].memory_type, MemoryType::Episodic);
        assert!(result[0].content.contains("project alpha"));
        assert_eq!(result[0].source_ids, vec!["e1".to_string()]);
    }

    #[test]
    fn test_distill_multiple_same_type() {
        let entries = vec![
            make_working_entry("e1", "fact A", MemoryType::Semantic, vec!["facts".to_string()]),
            make_working_entry("e2", "fact B", MemoryType::Semantic, vec!["facts".to_string()]),
        ];
        let result = distill_working_memory(&entries, |_| None);
        assert_eq!(result.len(), 1);
        assert!(result[0].content.contains("fact A"));
        assert!(result[0].content.contains("fact B"));
    }

    #[test]
    fn test_distill_groups_by_type() {
        let entries = vec![
            make_working_entry("e1", "event happened", MemoryType::Episodic, vec![]),
            make_working_entry("e2", "user likes X", MemoryType::Semantic, vec![]),
        ];
        let result = distill_working_memory(&entries, |_| None);
        assert_eq!(result.len(), 2);
        let types: Vec<MemoryType> = result.iter().map(|d| d.memory_type).collect();
        assert!(types.contains(&MemoryType::Episodic));
        assert!(types.contains(&MemoryType::Semantic));
    }

    #[test]
    fn test_distill_with_llm_summarizer() {
        let entries = vec![
            make_working_entry("e1", "discussed A", MemoryType::Episodic, vec![]),
            make_working_entry("e2", "discussed B", MemoryType::Episodic, vec![]),
        ];
        let result = distill_working_memory(&entries, |text| {
            Some(format!("Summary of: {}", &text[..20.min(text.len())]))
        });
        assert_eq!(result.len(), 1);
        assert!(result[0].content.starts_with("Summary of:"));
    }

    #[test]
    fn test_distill_deduplicates_content() {
        let entries = vec![
            make_working_entry("e1", "same content", MemoryType::Semantic, vec![]),
            make_working_entry("e2", "same content", MemoryType::Semantic, vec![]),
        ];
        let result = distill_working_memory(&entries, |_| None);
        assert_eq!(result.len(), 1);
        assert!(!result[0].content.contains(" | same content"));
    }

    #[test]
    fn test_distill_merges_tags() {
        let entries = vec![
            make_working_entry("e1", "A", MemoryType::Semantic, vec!["tag1".to_string()]),
            make_working_entry("e2", "B", MemoryType::Semantic, vec!["tag2".to_string()]),
        ];
        let result = distill_working_memory(&entries, |_| None);
        assert!(result[0].tags.contains(&"tag1".to_string()));
        assert!(result[0].tags.contains(&"tag2".to_string()));
    }

    #[test]
    fn test_distill_takes_max_importance() {
        let mut e1 = make_working_entry("e1", "A", MemoryType::Semantic, vec![]);
        e1.importance = 30;
        let mut e2 = make_working_entry("e2", "B", MemoryType::Semantic, vec![]);
        e2.importance = 90;
        let result = distill_working_memory(&[e1, e2], |_| None);
        assert_eq!(result[0].importance, 90);
    }

    #[test]
    fn test_to_long_term_entry() {
        let distilled = DistilledEntry {
            content: "summarized content".to_string(),
            memory_type: MemoryType::Episodic,
            tags: vec!["session".to_string()],
            importance: 75,
            source_ids: vec!["e1".to_string()],
        };
        let entry = to_long_term_entry(&distilled, "agent-1", "tenant-1");
        assert_eq!(entry.tier, MemoryTier::LongTerm);
        assert_eq!(entry.memory_type, MemoryType::Episodic);
        assert_eq!(entry.importance, 75);
        assert_eq!(entry.tenant_id, "tenant-1");
    }

    #[test]
    fn test_summarization_prompt_format() {
        let prompt = summarization_prompt("some memories here");
        assert!(prompt.contains("some memories here"));
        assert!(prompt.contains("Summarize"));
    }
}
