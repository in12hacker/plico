//! Reflective Meta-Memory — the AI brain's self-awareness about its own memory quality.
//!
//! Tracks meta-metrics about how well the memory system is performing:
//! - Retrieval hit rate (per QueryIntent)
//! - Dedup efficiency
//! - Distillation fidelity
//! - Causal chain completeness
//! - Foresight accuracy
//! - Cross-agent knowledge utilization
//!
//! Detects trends and auto-tunes parameters when quality degrades.

use std::collections::HashMap;
use crate::fs::retrieval_router::QueryIntent;

/// A snapshot of meta-metrics at a point in time.
#[derive(Debug, Clone)]
pub struct MetaSnapshot {
    pub timestamp_ms: u64,
    pub retrieval_hits: HashMap<QueryIntent, HitRate>,
    pub dedup_rate: f64,
    pub causal_completeness: f64,
    pub foresight_hit_rate: f64,
    pub cross_agent_utilization: f64,
}

/// Hit rate for a single query intent category.
#[derive(Debug, Clone, Copy, Default)]
pub struct HitRate {
    pub queries: u64,
    pub hits: u64,
}

impl HitRate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rate(&self) -> f64 {
        if self.queries == 0 {
            0.0
        } else {
            self.hits as f64 / self.queries as f64
        }
    }

    pub fn record(&mut self, hit: bool) {
        self.queries += 1;
        if hit {
            self.hits += 1;
        }
    }
}

/// The reflective meta-memory tracker.
#[derive(Debug)]
pub struct MetaMemory {
    intent_hits: HashMap<QueryIntent, HitRate>,
    dedup_total: u64,
    dedup_merged: u64,
    causal_with_parent: u64,
    causal_total: u64,
    foresight_predictions: u64,
    foresight_correct: u64,
    shared_accessed: u64,
    shared_total: u64,
    history: Vec<MetaSnapshot>,
    max_history: usize,
}

impl MetaMemory {
    pub fn new(max_history: usize) -> Self {
        Self {
            intent_hits: HashMap::new(),
            dedup_total: 0,
            dedup_merged: 0,
            causal_with_parent: 0,
            causal_total: 0,
            foresight_predictions: 0,
            foresight_correct: 0,
            shared_accessed: 0,
            shared_total: 0,
            history: Vec::new(),
            max_history,
        }
    }

    pub fn default_tracker() -> Self {
        Self::new(100)
    }

    // ─── Recording ─────────────────────────────────────────────

    pub fn record_retrieval(&mut self, intent: QueryIntent, hit: bool) {
        self.intent_hits
            .entry(intent)
            .or_default()
            .record(hit);
    }

    pub fn record_dedup(&mut self, was_duplicate: bool) {
        self.dedup_total += 1;
        if was_duplicate {
            self.dedup_merged += 1;
        }
    }

    pub fn record_causal(&mut self, has_parent: bool) {
        self.causal_total += 1;
        if has_parent {
            self.causal_with_parent += 1;
        }
    }

    pub fn record_foresight(&mut self, was_correct: bool) {
        self.foresight_predictions += 1;
        if was_correct {
            self.foresight_correct += 1;
        }
    }

    pub fn record_shared_access(&mut self, total_shared: u64, accessed: u64) {
        self.shared_total = total_shared;
        self.shared_accessed = accessed;
    }

    // ─── Metrics ───────────────────────────────────────────────

    pub fn retrieval_hit_rate(&self, intent: QueryIntent) -> f64 {
        self.intent_hits
            .get(&intent)
            .map(|h| h.rate())
            .unwrap_or(0.0)
    }

    pub fn overall_hit_rate(&self) -> f64 {
        let total_queries: u64 = self.intent_hits.values().map(|h| h.queries).sum();
        let total_hits: u64 = self.intent_hits.values().map(|h| h.hits).sum();
        if total_queries == 0 {
            0.0
        } else {
            total_hits as f64 / total_queries as f64
        }
    }

    pub fn dedup_rate(&self) -> f64 {
        if self.dedup_total == 0 {
            0.0
        } else {
            self.dedup_merged as f64 / self.dedup_total as f64
        }
    }

    pub fn causal_completeness(&self) -> f64 {
        if self.causal_total == 0 {
            0.0
        } else {
            self.causal_with_parent as f64 / self.causal_total as f64
        }
    }

    pub fn foresight_accuracy(&self) -> f64 {
        if self.foresight_predictions == 0 {
            0.0
        } else {
            self.foresight_correct as f64 / self.foresight_predictions as f64
        }
    }

    pub fn cross_agent_utilization(&self) -> f64 {
        if self.shared_total == 0 {
            0.0
        } else {
            self.shared_accessed as f64 / self.shared_total as f64
        }
    }

    // ─── Snapshot & Trend ──────────────────────────────────────

    /// Take a snapshot of current metrics and add to history.
    pub fn take_snapshot(&mut self, timestamp_ms: u64) {
        let snapshot = MetaSnapshot {
            timestamp_ms,
            retrieval_hits: self.intent_hits.clone(),
            dedup_rate: self.dedup_rate(),
            causal_completeness: self.causal_completeness(),
            foresight_hit_rate: self.foresight_accuracy(),
            cross_agent_utilization: self.cross_agent_utilization(),
        };
        self.history.push(snapshot);
        if self.history.len() > self.max_history {
            self.history.remove(0);
        }
    }

    /// Detect if a metric is trending downward over the last N snapshots.
    pub fn is_declining(&self, metric_fn: impl Fn(&MetaSnapshot) -> f64, window: usize) -> bool {
        if self.history.len() < window + 1 {
            return false;
        }
        let recent = &self.history[self.history.len() - window..];
        let first_half = &recent[..window / 2];
        let second_half = &recent[window / 2..];

        let avg_first: f64 = first_half.iter().map(&metric_fn).sum::<f64>()
            / first_half.len().max(1) as f64;
        let avg_second: f64 = second_half.iter().map(&metric_fn).sum::<f64>()
            / second_half.len().max(1) as f64;

        avg_second < avg_first * 0.9
    }

    /// Number of snapshots in history.
    pub fn snapshot_count(&self) -> usize {
        self.history.len()
    }

    // ─── Auto-Tuning Recommendations ──────────────────────────

    /// Generate tuning recommendations based on current metrics.
    pub fn recommend_tuning(&self) -> Vec<TuningAction> {
        let mut actions = Vec::new();

        if self.overall_hit_rate() < 0.5 && self.intent_hits.values().any(|h| h.queries > 10) {
            actions.push(TuningAction::IncreaseTopK { by: 5 });
        }

        if self.dedup_rate() > 0.4 && self.dedup_total > 20 {
            actions.push(TuningAction::LowerDedupThreshold { to: 0.85 });
        }

        if self.causal_completeness() < 0.3 && self.causal_total > 10 {
            actions.push(TuningAction::EnableCausalInference);
        }

        if self.foresight_accuracy() < 0.2 && self.foresight_predictions > 20 {
            actions.push(TuningAction::IncreaseForesightHops { to: 3 });
        }

        if self.cross_agent_utilization() < 0.1 && self.shared_total > 5 {
            actions.push(TuningAction::PromoteSharedVisibility);
        }

        actions
    }
}

/// A concrete tuning action recommended by the meta-memory system.
#[derive(Debug, Clone, PartialEq)]
pub enum TuningAction {
    IncreaseTopK { by: usize },
    LowerDedupThreshold { to: f64 },
    EnableCausalInference,
    IncreaseForesightHops { to: usize },
    PromoteSharedVisibility,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hit_rate_tracking() {
        let mut meta = MetaMemory::default_tracker();
        meta.record_retrieval(QueryIntent::Factual, true);
        meta.record_retrieval(QueryIntent::Factual, true);
        meta.record_retrieval(QueryIntent::Factual, false);
        assert!((meta.retrieval_hit_rate(QueryIntent::Factual) - 0.667).abs() < 0.01);
    }

    #[test]
    fn test_overall_hit_rate() {
        let mut meta = MetaMemory::default_tracker();
        meta.record_retrieval(QueryIntent::Factual, true);
        meta.record_retrieval(QueryIntent::Temporal, false);
        assert!((meta.overall_hit_rate() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_dedup_rate() {
        let mut meta = MetaMemory::default_tracker();
        meta.record_dedup(true);
        meta.record_dedup(true);
        meta.record_dedup(false);
        assert!((meta.dedup_rate() - 0.667).abs() < 0.01);
    }

    #[test]
    fn test_causal_completeness() {
        let mut meta = MetaMemory::default_tracker();
        meta.record_causal(true);
        meta.record_causal(true);
        meta.record_causal(false);
        meta.record_causal(false);
        assert!((meta.causal_completeness() - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_snapshot_history() {
        let mut meta = MetaMemory::new(3);
        meta.take_snapshot(1000);
        meta.take_snapshot(2000);
        meta.take_snapshot(3000);
        meta.take_snapshot(4000);
        assert_eq!(meta.snapshot_count(), 3, "should cap at max_history");
    }

    #[test]
    fn test_declining_trend_detection() {
        let mut meta = MetaMemory::new(100);

        for i in 0..5 {
            meta.record_retrieval(QueryIntent::Factual, true);
            meta.take_snapshot(i * 1000);
        }
        for i in 5..10 {
            meta.record_retrieval(QueryIntent::Factual, false);
            meta.record_retrieval(QueryIntent::Factual, false);
            meta.take_snapshot(i * 1000);
        }

        let declining = meta.is_declining(
            |s| {
                s.retrieval_hits
                    .get(&QueryIntent::Factual)
                    .map(|h| h.rate())
                    .unwrap_or(0.0)
            },
            8,
        );
        assert!(declining, "hit rate should show declining trend");
    }

    #[test]
    fn test_no_decline_on_stable_metric() {
        let mut meta = MetaMemory::new(100);
        for i in 0..10 {
            meta.record_retrieval(QueryIntent::Factual, true);
            meta.take_snapshot(i * 1000);
        }

        let declining = meta.is_declining(
            |s| {
                s.retrieval_hits
                    .get(&QueryIntent::Factual)
                    .map(|h| h.rate())
                    .unwrap_or(0.0)
            },
            8,
        );
        assert!(!declining, "stable metric should not show decline");
    }

    #[test]
    fn test_recommend_increase_topk() {
        let mut meta = MetaMemory::default_tracker();
        for _ in 0..20 {
            meta.record_retrieval(QueryIntent::Factual, false);
        }
        let actions = meta.recommend_tuning();
        assert!(actions.contains(&TuningAction::IncreaseTopK { by: 5 }));
    }

    #[test]
    fn test_recommend_lower_dedup_threshold() {
        let mut meta = MetaMemory::default_tracker();
        for _ in 0..30 {
            meta.record_dedup(true);
        }
        let actions = meta.recommend_tuning();
        assert!(actions.contains(&TuningAction::LowerDedupThreshold { to: 0.85 }));
    }

    #[test]
    fn test_no_recommendations_when_healthy() {
        let mut meta = MetaMemory::default_tracker();
        for _ in 0..20 {
            meta.record_retrieval(QueryIntent::Factual, true);
            meta.record_dedup(false);
            meta.record_causal(true);
            meta.record_foresight(true);
        }
        meta.record_shared_access(10, 8);
        let actions = meta.recommend_tuning();
        assert!(actions.is_empty(), "healthy system needs no tuning");
    }
}
