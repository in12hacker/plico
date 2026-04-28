//! Agent Profile — per-agent adaptive retrieval weights (v31, Axiom 9).
//!
//! Tracks an agent's usage patterns (intent histogram, memory type preference,
//! retrieval latency) and uses exponential moving average (EMA) to learn
//! personalized FusionWeights for the Retrieval Fusion Engine.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::fs::retrieval_fusion::FusionWeights;
use crate::fs::retrieval_router::QueryIntent;
use crate::memory::layered::MemoryType;

/// Per-agent profile tracking usage patterns and learned retrieval weights.
#[derive(Debug, Clone)]
pub struct AgentProfile {
    pub agent_id: String,
    pub intent_histogram: HashMap<QueryIntent, u64>,
    pub memory_type_preference: HashMap<MemoryType, f32>,
    pub avg_retrieval_latency_ms: f64,
    pub retrieval_weights: FusionWeights,
    pub total_sessions: u64,
    pub total_queries: u64,
}

impl AgentProfile {
    pub fn new(agent_id: &str) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            intent_histogram: HashMap::new(),
            memory_type_preference: HashMap::new(),
            avg_retrieval_latency_ms: 0.0,
            retrieval_weights: FusionWeights::default(),
            total_sessions: 0,
            total_queries: 0,
        }
    }

    /// Record an intent query, updating the histogram and query count.
    pub fn record_query(&mut self, intent: QueryIntent, latency_ms: f64) {
        *self.intent_histogram.entry(intent).or_insert(0) += 1;
        self.total_queries += 1;

        // EMA for latency (alpha = 0.1)
        const ALPHA: f64 = 0.1;
        if self.total_queries == 1 {
            self.avg_retrieval_latency_ms = latency_ms;
        } else {
            self.avg_retrieval_latency_ms =
                ALPHA * latency_ms + (1.0 - ALPHA) * self.avg_retrieval_latency_ms;
        }
    }

    /// Record which memory types were useful in a retrieval result.
    pub fn record_memory_type_hit(&mut self, memory_type: MemoryType) {
        let entry = self.memory_type_preference.entry(memory_type).or_insert(0.0);
        *entry += 1.0;
    }

    /// Record a session start.
    pub fn record_session(&mut self) {
        self.total_sessions += 1;
    }

    /// Update retrieval weights based on feedback (which results were used).
    ///
    /// The learning rate is adaptive: higher early on (exploration), lower
    /// after ~50 queries (exploitation). Uses EMA to nudge weights toward
    /// signals that predicted useful results.
    pub fn learn_weights(&mut self, used_signals: &[SignalFeedback]) {
        if used_signals.is_empty() {
            return;
        }
        let learning_rate = (0.3 / (1.0 + self.total_queries as f32 / 50.0)).max(0.01);

        let mut nudge = FusionWeights {
            semantic: 0.0,
            causal: 0.0,
            access: 0.0,
            tag: 0.0,
            temporal: 0.0,
            type_match: 0.0,
            bm25_keyword: 0.0,
        };

        for fb in used_signals {
            nudge.semantic += fb.semantic_was_high as u8 as f32;
            nudge.causal += fb.causal_was_high as u8 as f32;
            nudge.access += fb.access_was_high as u8 as f32;
            nudge.tag += fb.tag_was_high as u8 as f32;
            nudge.temporal += fb.temporal_was_high as u8 as f32;
            nudge.type_match += fb.type_was_match as u8 as f32;
            nudge.bm25_keyword += fb.bm25_was_high as u8 as f32;
        }

        let n = used_signals.len() as f32;
        let w = &mut self.retrieval_weights;
        w.semantic += learning_rate * (nudge.semantic / n - 0.5);
        w.causal += learning_rate * (nudge.causal / n - 0.5);
        w.access += learning_rate * (nudge.access / n - 0.5);
        w.tag += learning_rate * (nudge.tag / n - 0.5);
        w.temporal += learning_rate * (nudge.temporal / n - 0.5);
        w.type_match += learning_rate * (nudge.type_match / n - 0.5);
        w.bm25_keyword += learning_rate * (nudge.bm25_keyword / n - 0.5);

        let clamp = |v: f32| v.max(0.02);
        w.semantic = clamp(w.semantic);
        w.causal = clamp(w.causal);
        w.access = clamp(w.access);
        w.tag = clamp(w.tag);
        w.temporal = clamp(w.temporal);
        w.type_match = clamp(w.type_match);
        w.bm25_keyword = clamp(w.bm25_keyword);

        w.normalize();
    }

    /// Get the dominant intent type for this agent.
    pub fn dominant_intent(&self) -> Option<QueryIntent> {
        self.intent_histogram
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(intent, _)| *intent)
    }
}

/// Feedback on which signals predicted a useful result.
#[derive(Debug, Clone)]
pub struct SignalFeedback {
    pub semantic_was_high: bool,
    pub causal_was_high: bool,
    pub access_was_high: bool,
    pub tag_was_high: bool,
    pub temporal_was_high: bool,
    pub type_was_match: bool,
    pub bm25_was_high: bool,
}

/// Thread-safe store for all agent profiles.
pub struct AgentProfileStore {
    profiles: RwLock<HashMap<String, AgentProfile>>,
}

impl Default for AgentProfileStore {
    fn default() -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
        }
    }
}

impl AgentProfileStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_or_create(&self, agent_id: &str) -> AgentProfile {
        let profiles = self.profiles.read().unwrap();
        if let Some(p) = profiles.get(agent_id) {
            return p.clone();
        }
        drop(profiles);

        let mut profiles = self.profiles.write().unwrap();
        profiles.entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id))
            .clone()
    }

    pub fn update(&self, profile: AgentProfile) {
        let mut profiles = self.profiles.write().unwrap();
        profiles.insert(profile.agent_id.clone(), profile);
    }

    pub fn get_weights(&self, agent_id: &str) -> FusionWeights {
        let profiles = self.profiles.read().unwrap();
        profiles
            .get(agent_id)
            .map(|p| p.retrieval_weights.clone())
            .unwrap_or_default()
    }

    pub fn list_agents(&self) -> Vec<String> {
        self.profiles.read().unwrap().keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_profile_has_default_weights() {
        let p = AgentProfile::new("test-agent");
        let w = &p.retrieval_weights;
        assert!((w.total() - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_record_query_updates_histogram() {
        let mut p = AgentProfile::new("test");
        p.record_query(QueryIntent::Factual, 100.0);
        p.record_query(QueryIntent::Factual, 200.0);
        p.record_query(QueryIntent::Temporal, 50.0);
        assert_eq!(p.intent_histogram[&QueryIntent::Factual], 2);
        assert_eq!(p.intent_histogram[&QueryIntent::Temporal], 1);
        assert_eq!(p.total_queries, 3);
    }

    #[test]
    fn test_learn_weights_normalizes() {
        let mut p = AgentProfile::new("test");
        p.total_queries = 10;

        let feedback = vec![
            SignalFeedback {
                semantic_was_high: true,
                causal_was_high: false,
                access_was_high: false,
                tag_was_high: true,
                temporal_was_high: false,
                type_was_match: true,
                bm25_was_high: false,
            },
        ];
        p.learn_weights(&feedback);

        let w = &p.retrieval_weights;
        assert!((w.total() - 1.0).abs() < 0.01, "weights sum to {}", w.total());
    }

    #[test]
    fn test_dominant_intent() {
        let mut p = AgentProfile::new("test");
        p.record_query(QueryIntent::Factual, 10.0);
        p.record_query(QueryIntent::Factual, 10.0);
        p.record_query(QueryIntent::Temporal, 10.0);
        assert_eq!(p.dominant_intent(), Some(QueryIntent::Factual));
    }

    #[test]
    fn test_profile_store_get_or_create() {
        let store = AgentProfileStore::new();
        let p1 = store.get_or_create("agent-1");
        assert_eq!(p1.agent_id, "agent-1");
        assert_eq!(p1.total_queries, 0);

        let mut p1_mut = p1.clone();
        p1_mut.record_query(QueryIntent::Factual, 10.0);
        store.update(p1_mut);

        let p1_again = store.get_or_create("agent-1");
        assert_eq!(p1_again.total_queries, 1);
    }
}
