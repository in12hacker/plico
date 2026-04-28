//! Query Augmentation Engine — enhances queries before retrieval.
//!
//! Four augmentation steps (LLM-first, rule-based fallback):
//! 1. LLM rewrite (skipped if LLM unavailable)
//! 2. KG entity expansion
//! 3. Temporal context injection
//! 4. Tag-based synonym expansion

use crate::fs::graph::KnowledgeGraph;
use crate::temporal::HeuristicTemporalResolver;

/// Augmented query with enriched context for retrieval.
#[derive(Debug, Clone)]
pub struct AugmentedQuery {
    pub original: String,
    pub rewritten: Option<String>,
    pub expanded_entities: Vec<String>,
    pub time_range: Option<(i64, i64)>,
    pub expanded_tags: Vec<String>,
}

impl AugmentedQuery {
    /// Produce the final query string for embedding/search.
    /// Combines the rewritten (or original) query with entity names and expanded tags.
    pub fn effective_query(&self) -> String {
        let base = self.rewritten.as_deref().unwrap_or(&self.original);
        if self.expanded_entities.is_empty() && self.expanded_tags.is_empty() {
            return base.to_string();
        }
        let mut parts = vec![base.to_string()];
        if !self.expanded_entities.is_empty() {
            parts.push(self.expanded_entities.join(" "));
        }
        if !self.expanded_tags.is_empty() {
            parts.push(self.expanded_tags.join(" "));
        }
        parts.join(" ")
    }
}

/// Build the LLM prompt for query rewriting.
pub fn rewrite_prompt(query: &str) -> String {
    format!(
        "Rewrite the following query to improve search retrieval. \
         Add synonyms, expand abbreviations, and add helpful context. \
         Output ONLY the rewritten query, nothing else.\n\n\
         Original query: {query}\n\n\
         Rewritten query:"
    )
}

/// Parse an LLM rewrite response. Returns None if the response is empty or
/// too similar to the original (indicating the LLM returned a non-rewrite).
pub fn parse_rewrite_response(response: &str, original: &str) -> Option<String> {
    let trimmed = response.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.to_lowercase() == original.to_lowercase() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Extract entity labels from the query by matching against KG nodes.
///
/// Scans the query for substrings that match known node labels (case-insensitive).
pub fn expand_entities_from_kg(
    query: &str,
    kg: &dyn KnowledgeGraph,
    agent_id: &str,
) -> Vec<String> {
    let all_ids = kg.all_node_ids();
    let query_lower = query.to_lowercase();
    let mut expanded = Vec::new();

    for node_id in &all_ids {
        if let Ok(Some(node)) = kg.get_node(node_id) {
            if node.agent_id != agent_id {
                continue;
            }
            if query_lower.contains(&node.label.to_lowercase()) {
                if let Ok(neighbors) = kg.get_neighbors(node_id, None, 1) {
                    for (neighbor, _edge) in &neighbors {
                        if !expanded.contains(&neighbor.label) && neighbor.label.to_lowercase() != query_lower {
                            expanded.push(neighbor.label.clone());
                        }
                    }
                }
            }
        }
    }

    expanded.truncate(10);
    expanded
}

/// Extract a temporal range from the query using the TemporalResolver.
pub fn extract_time_range(query: &str) -> Option<(i64, i64)> {
    let resolver = HeuristicTemporalResolver::new();
    let temporal_phrases = extract_temporal_phrases(query);
    for phrase in temporal_phrases {
        if let Some((since, until, _conf, _gran)) = resolver.resolve(&phrase) {
            return Some((since, until));
        }
    }
    None
}

/// Expand the query with related tags from a known tag set.
///
/// For each word in the query, find tags that contain that word as a substring.
pub fn expand_with_tags(query: &str, known_tags: &[String]) -> Vec<String> {
    let query_words: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_string())
        .collect();

    let mut expanded = Vec::new();
    for tag in known_tags {
        let tag_lower = tag.to_lowercase();
        for word in &query_words {
            if tag_lower.contains(word.as_str()) && !query.to_lowercase().contains(&tag_lower) {
                if !expanded.contains(tag) {
                    expanded.push(tag.clone());
                }
                break;
            }
        }
    }

    expanded.truncate(8);
    expanded
}

/// Full augmentation pipeline: LLM rewrite → KG expansion → temporal → tag expansion.
///
/// `llm_rewrite` is an optional pre-computed LLM rewrite (pass None to skip).
pub fn augment_query(
    query: &str,
    llm_rewrite: Option<String>,
    kg: Option<&dyn KnowledgeGraph>,
    agent_id: &str,
    known_tags: &[String],
) -> AugmentedQuery {
    let rewritten = llm_rewrite.and_then(|r| parse_rewrite_response(&r, query));

    let expanded_entities = match kg {
        Some(kg) => expand_entities_from_kg(query, kg, agent_id),
        None => Vec::new(),
    };

    let time_range = extract_time_range(query);

    let expanded_tags = expand_with_tags(query, known_tags);

    AugmentedQuery {
        original: query.to_string(),
        rewritten,
        expanded_entities,
        time_range,
        expanded_tags,
    }
}

/// Extract phrases from a query that might represent temporal expressions.
fn extract_temporal_phrases(query: &str) -> Vec<String> {
    let temporal_markers = [
        "yesterday", "today", "last week", "last month", "last year",
        "this week", "this month", "this year", "tomorrow",
        "two weeks ago", "three days ago", "a week ago", "a month ago",
        "昨天", "今天", "上周", "上个月", "去年", "本周", "本月", "今年",
    ];

    let query_lower = query.to_lowercase();
    let mut phrases = Vec::new();
    for marker in &temporal_markers {
        if query_lower.contains(marker) {
            phrases.push(marker.to_string());
        }
    }
    phrases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_augmented_query_effective_no_expansion() {
        let aq = AugmentedQuery {
            original: "What is Plico?".to_string(),
            rewritten: None,
            expanded_entities: vec![],
            time_range: None,
            expanded_tags: vec![],
        };
        assert_eq!(aq.effective_query(), "What is Plico?");
    }

    #[test]
    fn test_augmented_query_effective_with_rewrite() {
        let aq = AugmentedQuery {
            original: "What is Plico?".to_string(),
            rewritten: Some("What is Plico AI-Native OS framework?".to_string()),
            expanded_entities: vec![],
            time_range: None,
            expanded_tags: vec![],
        };
        assert_eq!(aq.effective_query(), "What is Plico AI-Native OS framework?");
    }

    #[test]
    fn test_augmented_query_effective_with_all_expansions() {
        let aq = AugmentedQuery {
            original: "Tell me about project alpha".to_string(),
            rewritten: None,
            expanded_entities: vec!["beta-module".to_string()],
            time_range: None,
            expanded_tags: vec!["project-management".to_string()],
        };
        let eff = aq.effective_query();
        assert!(eff.contains("Tell me about project alpha"));
        assert!(eff.contains("beta-module"));
        assert!(eff.contains("project-management"));
    }

    #[test]
    fn test_parse_rewrite_response_valid() {
        let result = parse_rewrite_response(
            "What is the Plico AI-Native Operating System?",
            "What is Plico?",
        );
        assert_eq!(result, Some("What is the Plico AI-Native Operating System?".to_string()));
    }

    #[test]
    fn test_parse_rewrite_response_same_as_original() {
        let result = parse_rewrite_response("What is Plico?", "What is Plico?");
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_rewrite_response_empty() {
        let result = parse_rewrite_response("", "What is Plico?");
        assert!(result.is_none());
    }

    #[test]
    fn test_expand_with_tags_basic() {
        let tags = vec![
            "rust-programming".to_string(),
            "python-dev".to_string(),
            "project-management".to_string(),
            "memory-system".to_string(),
        ];
        let expanded = expand_with_tags("rust performance tips", &tags);
        assert!(expanded.contains(&"rust-programming".to_string()));
        assert!(!expanded.contains(&"python-dev".to_string()));
    }

    #[test]
    fn test_expand_with_tags_no_match() {
        let tags = vec!["unrelated".to_string()];
        let expanded = expand_with_tags("quantum computing", &tags);
        assert!(expanded.is_empty());
    }

    #[test]
    fn test_extract_time_range_yesterday() {
        let result = extract_time_range("What happened yesterday?");
        assert!(result.is_some());
        let (since, until) = result.unwrap();
        assert!(since < until);
    }

    #[test]
    fn test_extract_time_range_no_temporal() {
        let result = extract_time_range("What is the meaning of life?");
        assert!(result.is_none());
    }

    #[test]
    fn test_augment_query_full_pipeline() {
        let tags = vec!["ai-memory".to_string(), "plico-core".to_string()];
        let aq = augment_query(
            "Show me yesterday's memory entries",
            Some("Display all memory entries from yesterday including embeddings".to_string()),
            None,
            "agent-1",
            &tags,
        );
        assert!(aq.rewritten.is_some());
        assert!(aq.time_range.is_some());
        assert!(aq.expanded_tags.contains(&"ai-memory".to_string()));
    }

    #[test]
    fn test_augment_query_no_llm() {
        let aq = augment_query(
            "What is Plico?",
            None,
            None,
            "agent-1",
            &[],
        );
        assert!(aq.rewritten.is_none());
        assert!(aq.expanded_entities.is_empty());
        assert!(aq.time_range.is_none());
    }

    #[test]
    fn test_rewrite_prompt_format() {
        let prompt = rewrite_prompt("test query");
        assert!(prompt.contains("test query"));
        assert!(prompt.contains("Rewrite"));
    }

    #[test]
    fn test_extract_temporal_phrases_chinese() {
        let phrases = extract_temporal_phrases("昨天讨论了什么？");
        assert!(phrases.contains(&"昨天".to_string()));
    }

    #[test]
    fn test_expand_with_tags_avoids_duplicates() {
        let tags = vec!["rust-dev".to_string(), "rust-dev".to_string()];
        let expanded = expand_with_tags("rust tips", &tags);
        let unique: std::collections::HashSet<_> = expanded.iter().collect();
        assert_eq!(expanded.len(), unique.len());
    }
}
