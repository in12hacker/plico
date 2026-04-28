//! OS-level Retrieval Fusion Engine (RFE) — v31 original algorithm.
//!
//! Fuses six independent signal families that only an AI-OS can provide:
//! semantic embeddings, causal graph proximity, access patterns, tag overlap,
//! temporal recency, and memory type alignment.
//!
//! FusionWeights are per-agent learnable (see AgentProfile) and ship with
//! sensible defaults that work out-of-the-box.

use serde::{Deserialize, Serialize};

use crate::memory::layered::{MemoryEntry, MemoryType, now_ms};
use crate::memory::causal::CausalGraph;

/// Per-signal scores computed for a candidate retrieval result.
#[derive(Debug, Clone)]
pub struct RetrievalSignals {
    pub semantic_score: f32,
    pub causal_proximity: f32,
    pub access_affinity: f32,
    pub tag_overlap: f32,
    pub temporal_recency: f32,
    pub type_match: f32,
    pub bm25_keyword: f32,
}

/// Tunable weights for each signal dimension. Defaults sum to 1.0.
///
/// Serializable for persistence and runtime configuration.
/// Agents can self-derive optimal weights via EMA learning (AgentProfile).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionWeights {
    pub semantic: f32,
    pub causal: f32,
    pub access: f32,
    pub tag: f32,
    pub temporal: f32,
    pub type_match: f32,
    pub bm25_keyword: f32,
}

impl Default for FusionWeights {
    fn default() -> Self {
        Self {
            semantic: 0.35,
            causal: 0.12,
            access: 0.08,
            tag: 0.12,
            temporal: 0.08,
            type_match: 0.10,
            bm25_keyword: 0.15,
        }
    }
}

impl FusionWeights {
    pub fn fuse(&self, signals: &RetrievalSignals) -> f32 {
        self.semantic * signals.semantic_score
            + self.causal * signals.causal_proximity
            + self.access * signals.access_affinity
            + self.tag * signals.tag_overlap
            + self.temporal * signals.temporal_recency
            + self.type_match * signals.type_match
            + self.bm25_keyword * signals.bm25_keyword
    }

    /// Sum of all weights (should be ~1.0 after normalization).
    pub fn total(&self) -> f32 {
        self.semantic + self.causal + self.access + self.tag
            + self.temporal + self.type_match + self.bm25_keyword
    }

    /// Normalize weights so they sum to 1.0, preserving ratios.
    pub fn normalize(&mut self) {
        let t = self.total();
        if t > 0.0 {
            self.semantic /= t;
            self.causal /= t;
            self.access /= t;
            self.tag /= t;
            self.temporal /= t;
            self.type_match /= t;
            self.bm25_keyword /= t;
        }
    }
}

/// A scored retrieval result with per-signal breakdown.
#[derive(Debug, Clone)]
pub struct FusedResult {
    pub entry: MemoryEntry,
    pub fused_score: f32,
    pub signals: RetrievalSignals,
}

/// Query context for computing signals.
pub struct RetrievalQuery<'a> {
    pub query_embedding: &'a [f32],
    pub query_tags: &'a [String],
    pub query_memory_type: Option<MemoryType>,
    pub context_entry_id: Option<&'a str>,
    /// Pre-computed BM25 scores keyed by entry ID (from Bm25Index::search).
    pub bm25_scores: Option<&'a std::collections::HashMap<String, f32>>,
}

/// The Retrieval Fusion Engine.
pub struct RetrievalFusionEngine {
    weights: FusionWeights,
    temporal_half_life_ms: u64,
}

impl RetrievalFusionEngine {
    pub fn new(weights: FusionWeights) -> Self {
        Self {
            weights,
            temporal_half_life_ms: 7 * 24 * 60 * 60 * 1000, // 7 days
        }
    }

    pub fn with_temporal_half_life(mut self, half_life_ms: u64) -> Self {
        self.temporal_half_life_ms = half_life_ms;
        self
    }

    pub fn weights(&self) -> &FusionWeights {
        &self.weights
    }

    pub fn set_weights(&mut self, weights: FusionWeights) {
        self.weights = weights;
    }

    /// Compute all signals for a candidate entry and fuse them into a single score.
    pub fn score(
        &self,
        candidate: &MemoryEntry,
        query: &RetrievalQuery,
        graph: Option<&CausalGraph>,
    ) -> RetrievalSignals {
        let semantic_score = if let Some(ref emb) = candidate.embedding {
            cosine_sim(query.query_embedding, emb)
        } else {
            0.0
        };

        let causal_proximity = if let (Some(ctx_id), Some(g)) = (query.context_entry_id, graph) {
            match g.shortest_path_len(ctx_id, &candidate.id) {
                Some(0) => 1.0,
                Some(d) => 1.0 / (1.0 + d as f32),
                None => 0.0,
            }
        } else {
            0.0
        };

        let access_affinity = {
            let count = candidate.access_count as f32;
            (count.ln_1p() / 5.0).min(1.0)
        };

        let tag_overlap = {
            if query.query_tags.is_empty() && candidate.tags.is_empty() {
                0.0
            } else {
                let q_set: std::collections::HashSet<&str> =
                    query.query_tags.iter().map(|s| s.as_str()).collect();
                let c_set: std::collections::HashSet<&str> =
                    candidate.tags.iter().map(|s| s.as_str()).collect();
                let inter = q_set.intersection(&c_set).count();
                let union = q_set.union(&c_set).count();
                if union == 0 { 0.0 } else { inter as f32 / union as f32 }
            }
        };

        let temporal_recency = {
            let now = now_ms();
            let age_ms = now.saturating_sub(candidate.last_accessed);
            let half = self.temporal_half_life_ms as f64;
            if half < 1.0 { 0.0 } else { (-(age_ms as f64) / half * std::f64::consts::LN_2).exp() as f32 }
        };

        let type_match = match query.query_memory_type {
            Some(qt) if qt == candidate.memory_type => 1.0,
            Some(_) => 0.2,
            None => 0.5,
        };

        let bm25_keyword = query.bm25_scores
            .and_then(|scores| scores.get(&candidate.id).copied())
            .unwrap_or(0.0);

        RetrievalSignals {
            semantic_score,
            causal_proximity,
            access_affinity,
            tag_overlap,
            temporal_recency,
            type_match,
            bm25_keyword,
        }
    }

    /// Score and rank a list of candidates. Returns top-k by fused score.
    pub fn rank(
        &self,
        candidates: &[MemoryEntry],
        query: &RetrievalQuery,
        graph: Option<&CausalGraph>,
        top_k: usize,
    ) -> Vec<FusedResult> {
        let mut scored: Vec<FusedResult> = candidates
            .iter()
            .map(|entry| {
                let signals = self.score(entry, query, graph);
                let fused_score = self.weights.fuse(&signals);
                FusedResult {
                    entry: entry.clone(),
                    fused_score,
                    signals,
                }
            })
            .collect();

        scored.sort_by(|a, b| b.fused_score.partial_cmp(&a.fused_score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }
}

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-9 || norm_b < 1e-9 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::{MemoryContent, MemoryScope, MemoryTier};

    fn make_entry(id: &str, tags: Vec<&str>, access_count: u32, embedding: Option<Vec<f32>>) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            agent_id: "test".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(format!("content of {}", id)),
            importance: 50,
            access_count,
            last_accessed: now_ms(),
            created_at: now_ms(),
            tags: tags.into_iter().map(String::from).collect(),
            embedding,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Semantic,
            causal_parent: None,
            supersedes: None,
        }
    }

    #[test]
    fn test_default_weights_sum_to_one() {
        let w = FusionWeights::default();
        assert!((w.total() - 1.0).abs() < 0.01, "weights sum to {}", w.total());
    }

    #[test]
    fn test_fuse_score() {
        let w = FusionWeights::default();
        let signals = RetrievalSignals {
            semantic_score: 0.9,
            causal_proximity: 0.5,
            access_affinity: 0.3,
            tag_overlap: 0.8,
            temporal_recency: 0.7,
            type_match: 1.0,
            bm25_keyword: 0.6,
        };
        let score = w.fuse(&signals);
        assert!(score > 0.0 && score <= 1.0, "fused score = {}", score);
    }

    #[test]
    fn test_rank_orders_by_fused_score() {
        let engine = RetrievalFusionEngine::new(FusionWeights::default());
        let emb = vec![1.0, 0.0, 0.0];
        let e1 = make_entry("a", vec!["rust"], 10, Some(vec![0.9, 0.1, 0.0]));
        let e2 = make_entry("b", vec!["python"], 0, Some(vec![0.0, 1.0, 0.0]));

        let query = RetrievalQuery {
            query_embedding: &emb,
            query_tags: &["rust".to_string()],
            query_memory_type: Some(MemoryType::Semantic),
            context_entry_id: None,
            bm25_scores: None,
        };

        let results = engine.rank(&[e1.clone(), e2.clone()], &query, None, 10);
        assert_eq!(results[0].entry.id, "a");
        assert!(results[0].fused_score > results[1].fused_score);
    }

    #[test]
    fn test_bm25_signal_boosts_ranking() {
        let engine = RetrievalFusionEngine::new(FusionWeights::default());
        let emb = vec![1.0, 0.0, 0.0];
        let e1 = make_entry("a", vec![], 0, Some(vec![0.5, 0.5, 0.0]));
        let e2 = make_entry("b", vec![], 0, Some(vec![0.5, 0.5, 0.0]));

        let mut bm25 = std::collections::HashMap::new();
        bm25.insert("b".to_string(), 1.0_f32);

        let query = RetrievalQuery {
            query_embedding: &emb,
            query_tags: &[],
            query_memory_type: None,
            context_entry_id: None,
            bm25_scores: Some(&bm25),
        };

        let results = engine.rank(&[e1, e2], &query, None, 10);
        assert_eq!(results[0].entry.id, "b", "BM25 boost should promote entry b");
    }

    #[test]
    fn test_tag_overlap_jaccard() {
        let engine = RetrievalFusionEngine::new(FusionWeights::default());
        let e = make_entry("x", vec!["a", "b", "c"], 0, Some(vec![0.5; 3]));
        let query = RetrievalQuery {
            query_embedding: &[0.5; 3],
            query_tags: &["a".to_string(), "b".to_string(), "d".to_string()],
            query_memory_type: None,
            context_entry_id: None,
            bm25_scores: None,
        };
        let signals = engine.score(&e, &query, None);
        let expected_jaccard = 2.0 / 4.0; // {a,b} / {a,b,c,d}
        assert!((signals.tag_overlap - expected_jaccard).abs() < 0.01);
    }

    #[test]
    fn test_weights_serialize_deserialize() {
        let w = FusionWeights::default();
        let json = serde_json::to_string(&w).unwrap();
        let w2: FusionWeights = serde_json::from_str(&json).unwrap();
        assert!((w.semantic - w2.semantic).abs() < 1e-6);
        assert!((w.bm25_keyword - w2.bm25_keyword).abs() < 1e-6);
    }

    #[test]
    fn test_normalize_preserves_ratios() {
        let mut w = FusionWeights {
            semantic: 2.0,
            causal: 1.0,
            access: 1.0,
            tag: 1.0,
            temporal: 1.0,
            type_match: 1.0,
            bm25_keyword: 1.0,
        };
        w.normalize();
        assert!((w.total() - 1.0).abs() < 0.01);
        assert!(w.semantic > w.causal);
    }
}
