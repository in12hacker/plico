//! Memory Relevance Scoring — cognitive memory intelligence.
//!
//! Computes a combined relevance score for memory entries based on:
//! - **Recency**: exponential time decay — recent memories are more relevant
//! - **Frequency**: access count normalized — frequently recalled memories matter
//! - **Importance**: explicit agent-set weight — critical knowledge persists
//!
//! Used by `recall_relevant()` to select the optimal memory set within a
//! token budget, and by promotion logic to decide tier transitions.

use super::layered::{MemoryEntry, MemoryTier};
use std::collections::HashMap;

/// Weights for the combined relevance formula.
const W_RECENCY: f32 = 0.4;
const W_FREQUENCY: f32 = 0.3;
const W_IMPORTANCE: f32 = 0.3;

/// Time decay constant: halves relevance every 24 hours.
const DECAY_LAMBDA: f64 = 0.693 / (24.0 * 3600.0 * 1000.0); // ln(2) / 24h_ms

/// Relevance score breakdown for a single memory entry.
#[derive(Debug, Clone)]
pub struct RelevanceScore {
    pub recency: f32,
    pub frequency: f32,
    pub importance: f32,
    pub combined: f32,
}

/// Compute relevance score for a single entry.
///
/// `now_ms` — current time in milliseconds since epoch.
/// `max_access` — maximum access_count across all entries being scored
///                (for normalization). Must be >= 1.
pub fn score_entry(entry: &MemoryEntry, now_ms: u64, max_access: u32) -> RelevanceScore {
    let age_ms = now_ms.saturating_sub(entry.last_accessed) as f64;
    let recency = (-DECAY_LAMBDA * age_ms).exp() as f32;

    let max_a = max_access.max(1) as f32;
    let frequency = entry.access_count as f32 / max_a;

    let importance = entry.importance as f32 / 100.0;

    let combined = W_RECENCY * recency + W_FREQUENCY * frequency + W_IMPORTANCE * importance;

    RelevanceScore { recency, frequency, importance, combined }
}

/// Rank entries by relevance and return the top entries fitting within
/// `budget_tokens`. Approximate token count = content chars / 4.
pub fn select_within_budget(
    entries: &[MemoryEntry],
    budget_tokens: usize,
    now_ms: u64,
) -> Vec<(MemoryEntry, RelevanceScore)> {
    if entries.is_empty() {
        return Vec::new();
    }

    let max_access = entries.iter().map(|e| e.access_count).max().unwrap_or(1);

    let mut scored: Vec<(MemoryEntry, RelevanceScore)> = entries.iter()
        .map(|e| {
            let s = score_entry(e, now_ms, max_access);
            (e.clone(), s)
        })
        .collect();

    scored.sort_by(|a, b| b.1.combined.partial_cmp(&a.1.combined).unwrap_or(std::cmp::Ordering::Equal));

    let mut used_tokens = 0usize;
    let mut result = Vec::new();
    for (entry, score) in scored {
        let entry_tokens = estimate_tokens(&entry);
        if used_tokens + entry_tokens > budget_tokens && !result.is_empty() {
            break;
        }
        used_tokens += entry_tokens;
        result.push((entry, score));
    }

    result
}

/// Weights when semantic score is available (4-factor model).
const W_RECENCY_SEM: f32 = 0.25;
const W_FREQUENCY_SEM: f32 = 0.15;
const W_IMPORTANCE_SEM: f32 = 0.20;
const W_SEMANTIC: f32 = 0.40;

/// Compute relevance score incorporating an optional semantic similarity score.
pub fn score_entry_with_semantic(
    entry: &MemoryEntry,
    now_ms: u64,
    max_access: u32,
    semantic_score: Option<f32>,
) -> RelevanceScore {
    let base = score_entry(entry, now_ms, max_access);
    match semantic_score {
        Some(sem) => {
            let combined = W_RECENCY_SEM * base.recency
                + W_FREQUENCY_SEM * base.frequency
                + W_IMPORTANCE_SEM * base.importance
                + W_SEMANTIC * sem;
            RelevanceScore { combined, ..base }
        }
        None => base,
    }
}

/// Rank entries by relevance (with semantic scores) and return top entries within budget.
pub fn select_within_budget_semantic(
    entries: &[MemoryEntry],
    budget_tokens: usize,
    now_ms: u64,
    semantic_scores: &HashMap<String, f32>,
) -> Vec<(MemoryEntry, RelevanceScore)> {
    if entries.is_empty() {
        return Vec::new();
    }

    let max_access = entries.iter().map(|e| e.access_count).max().unwrap_or(1);

    let mut scored: Vec<(MemoryEntry, RelevanceScore)> = entries.iter()
        .map(|e| {
            let sem = semantic_scores.get(&e.id).copied();
            let s = score_entry_with_semantic(e, now_ms, max_access, sem);
            (e.clone(), s)
        })
        .collect();

    scored.sort_by(|a, b| b.1.combined.partial_cmp(&a.1.combined).unwrap_or(std::cmp::Ordering::Equal));

    let mut used_tokens = 0usize;
    let mut result = Vec::new();
    for (entry, score) in scored {
        let entry_tokens = estimate_tokens(&entry);
        if used_tokens + entry_tokens > budget_tokens && !result.is_empty() {
            break;
        }
        used_tokens += entry_tokens;
        result.push((entry, score));
    }

    result
}

/// Rough token estimate: chars / 4 (common approximation for English/code).
fn estimate_tokens(entry: &MemoryEntry) -> usize {
    let chars = entry.content.display().len();
    (chars / 4).max(1)
}

/// Check if an entry has expired based on its TTL.
pub fn is_expired(entry: &MemoryEntry, now_ms: u64) -> bool {
    match entry.ttl_ms {
        Some(ttl) => now_ms > entry.created_at.saturating_add(ttl),
        None => false,
    }
}

/// Promotion thresholds — determines when entries should move to a higher tier.
pub struct PromotionThresholds {
    pub ephemeral_to_working_access: u32,
    pub working_to_longterm_access: u32,
    pub working_to_longterm_importance: u8,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            ephemeral_to_working_access: 3,
            working_to_longterm_access: 10,
            working_to_longterm_importance: 50,
        }
    }
}

/// Determine the target tier for an entry based on promotion rules.
/// Returns `Some(new_tier)` if promotion should occur, `None` otherwise.
pub fn check_promotion(entry: &MemoryEntry, thresholds: &PromotionThresholds) -> Option<MemoryTier> {
    match entry.tier {
        MemoryTier::Ephemeral => {
            if entry.access_count >= thresholds.ephemeral_to_working_access {
                Some(MemoryTier::Working)
            } else {
                None
            }
        }
        MemoryTier::Working => {
            if entry.access_count >= thresholds.working_to_longterm_access
                && entry.importance >= thresholds.working_to_longterm_importance
            {
                Some(MemoryTier::LongTerm)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::MemoryContent;

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    fn make_entry(text: &str, access: u32, importance: u8, age_ms: u64) -> MemoryEntry {
        let now = now_ms();
        MemoryEntry {
            id: uuid::Uuid::new_v4().to_string(),
            agent_id: "test".into(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Ephemeral,
            content: MemoryContent::Text(text.into()),
            importance,
            access_count: access,
            last_accessed: now.saturating_sub(age_ms),
            created_at: now.saturating_sub(age_ms),
            tags: Vec::new(),
            embedding: None,
            ttl_ms: None,
            scope: crate::memory::layered::MemoryScope::Private,
        }
    }

    #[test]
    fn recent_entry_scores_higher() {
        let now = now_ms();
        let recent = make_entry("recent", 1, 50, 1000);
        let old = make_entry("old", 1, 50, 48 * 3600 * 1000);
        let s_recent = score_entry(&recent, now, 1);
        let s_old = score_entry(&old, now, 1);
        assert!(s_recent.recency > s_old.recency);
        assert!(s_recent.combined > s_old.combined);
    }

    #[test]
    fn frequent_entry_scores_higher() {
        let now = now_ms();
        let frequent = make_entry("freq", 10, 50, 1000);
        let rare = make_entry("rare", 1, 50, 1000);
        let s_freq = score_entry(&frequent, now, 10);
        let s_rare = score_entry(&rare, now, 10);
        assert!(s_freq.frequency > s_rare.frequency);
    }

    #[test]
    fn important_entry_scores_higher() {
        let now = now_ms();
        let important = make_entry("imp", 1, 90, 1000);
        let unimportant = make_entry("unimp", 1, 10, 1000);
        let s_imp = score_entry(&important, now, 1);
        let s_uni = score_entry(&unimportant, now, 1);
        assert!(s_imp.importance > s_uni.importance);
        assert!(s_imp.combined > s_uni.combined);
    }

    #[test]
    fn budget_selection_respects_limit() {
        let now = now_ms();
        let entries: Vec<MemoryEntry> = (0..10)
            .map(|i| make_entry(&format!("entry-{}", i), i, 50, 1000))
            .collect();
        let selected = select_within_budget(&entries, 10, now);
        assert!(!selected.is_empty());
        assert!(selected.len() <= 10);
    }

    #[test]
    fn ttl_expiration_check() {
        let mut entry = make_entry("ephemeral", 0, 50, 5000);
        entry.ttl_ms = Some(3000);
        let now = now_ms();
        assert!(is_expired(&entry, now));

        let mut fresh = make_entry("fresh", 0, 50, 0);
        fresh.ttl_ms = Some(60_000);
        assert!(!is_expired(&fresh, now));
    }

    #[test]
    fn promotion_ephemeral_to_working() {
        let thresholds = PromotionThresholds::default();
        let mut entry = make_entry("x", 2, 50, 0);
        assert!(check_promotion(&entry, &thresholds).is_none());
        entry.access_count = 3;
        assert_eq!(check_promotion(&entry, &thresholds), Some(MemoryTier::Working));
    }

    #[test]
    fn promotion_working_to_longterm() {
        let thresholds = PromotionThresholds::default();
        let mut entry = make_entry("x", 10, 50, 0);
        entry.tier = MemoryTier::Working;
        assert_eq!(check_promotion(&entry, &thresholds), Some(MemoryTier::LongTerm));

        entry.importance = 30;
        assert!(check_promotion(&entry, &thresholds).is_none());
    }

    #[test]
    fn no_promotion_for_longterm_or_procedural() {
        let thresholds = PromotionThresholds::default();
        let mut entry = make_entry("x", 100, 100, 0);
        entry.tier = MemoryTier::LongTerm;
        assert!(check_promotion(&entry, &thresholds).is_none());
        entry.tier = MemoryTier::Procedural;
        assert!(check_promotion(&entry, &thresholds).is_none());
    }

    #[test]
    fn semantic_score_boosts_relevance() {
        let now = now_ms();
        let entry = make_entry("x", 1, 50, 1000);
        let base = score_entry(&entry, now, 1);
        let with_sem = score_entry_with_semantic(&entry, now, 1, Some(0.9));
        assert!(with_sem.combined > base.combined,
            "Semantic score should boost combined relevance: {} vs {}", with_sem.combined, base.combined);
    }

    #[test]
    fn semantic_score_none_falls_back_to_base() {
        let now = now_ms();
        let entry = make_entry("x", 1, 50, 1000);
        let base = score_entry(&entry, now, 1);
        let no_sem = score_entry_with_semantic(&entry, now, 1, None);
        assert!((no_sem.combined - base.combined).abs() < f32::EPSILON,
            "Without semantic score, should equal base scoring");
    }
}
