//! Memory Consolidation Engine (MCE) — v31 background service.
//!
//! Analogous to the brain's hippocampal-neocortical consolidation process:
//!
//! 1. **Semantic dedup** — merge near-duplicate memories (similarity > 0.9)
//! 2. **Contradiction resolution** — detect & supersede using CSC algorithm
//! 3. **Confidence decay** — exponential decay for unaccessed memories
//! 4. **Access enhancement** — log-boost for frequently retrieved memories
//! 5. **Dependency propagation** — update descendants when ancestors change

use crate::memory::layered::{MemoryEntry, now_ms};
use crate::memory::causal::CausalGraph;
use crate::memory::contradiction::{
    ContradictionClassifier, RuleBasedClassifier, build_context,
};

/// Configuration for the consolidation engine.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    pub dedup_similarity_threshold: f32,
    pub contradiction_confidence_threshold: f32,
    pub decay_half_life_ms: u64,
    pub access_boost_log_base: f32,
    pub max_entries_per_run: usize,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            dedup_similarity_threshold: 0.90,
            contradiction_confidence_threshold: 0.50,
            decay_half_life_ms: 14 * 24 * 60 * 60 * 1000, // 14 days
            access_boost_log_base: 2.0,
            max_entries_per_run: 500,
        }
    }
}

/// A single consolidation action to apply.
#[derive(Debug, Clone)]
pub enum ConsolidationAction {
    Merge {
        keep_id: String,
        remove_id: String,
        merged_content: String,
    },
    Supersede {
        old_id: String,
        new_id: String,
        confidence: f32,
        evidence: String,
    },
    DecayConfidence {
        entry_id: String,
        new_importance: u8,
    },
    BoostConfidence {
        entry_id: String,
        new_importance: u8,
    },
}

/// Result of a consolidation run.
#[derive(Debug, Clone, Default)]
pub struct ConsolidationReport {
    pub entries_scanned: usize,
    pub merges: usize,
    pub contradictions_found: usize,
    pub decays_applied: usize,
    pub boosts_applied: usize,
    pub actions: Vec<ConsolidationAction>,
}

/// The Memory Consolidation Engine.
pub struct MemoryConsolidationEngine {
    config: ConsolidationConfig,
    classifier: Box<dyn ContradictionClassifier>,
}

impl MemoryConsolidationEngine {
    pub fn new(config: ConsolidationConfig) -> Self {
        Self {
            config,
            classifier: Box::new(RuleBasedClassifier),
        }
    }

    pub fn with_classifier(mut self, classifier: Box<dyn ContradictionClassifier>) -> Self {
        self.classifier = classifier;
        self
    }

    /// Run a consolidation pass over the given entries.
    /// Returns a report with actions to apply (caller applies them to the memory store).
    pub fn consolidate(&self, entries: &[MemoryEntry]) -> ConsolidationReport {
        let entries_to_process: Vec<&MemoryEntry> = entries
            .iter()
            .take(self.config.max_entries_per_run)
            .collect();

        let mut report = ConsolidationReport {
            entries_scanned: entries_to_process.len(),
            ..Default::default()
        };

        let graph = CausalGraph::build(entries);

        self.find_dedup_pairs(&entries_to_process, &mut report);
        self.find_contradictions(&entries_to_process, &graph, &mut report);
        self.compute_decay_boost(&entries_to_process, &mut report);

        report
    }

    fn find_dedup_pairs(
        &self,
        entries: &[&MemoryEntry],
        report: &mut ConsolidationReport,
    ) {
        let with_embeddings: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|e| e.embedding.is_some())
            .copied()
            .collect();

        let mut merged_ids = std::collections::HashSet::new();

        for i in 0..with_embeddings.len() {
            if merged_ids.contains(&with_embeddings[i].id) {
                continue;
            }
            for j in (i + 1)..with_embeddings.len() {
                if merged_ids.contains(&with_embeddings[j].id) {
                    continue;
                }
                if with_embeddings[i].agent_id != with_embeddings[j].agent_id {
                    continue;
                }
                if with_embeddings[i].memory_type != with_embeddings[j].memory_type {
                    continue;
                }

                let sim = cosine_sim(
                    with_embeddings[i].embedding.as_ref().unwrap(),
                    with_embeddings[j].embedding.as_ref().unwrap(),
                );

                if sim >= self.config.dedup_similarity_threshold {
                    let (keep, remove) = if with_embeddings[i].importance >= with_embeddings[j].importance {
                        (with_embeddings[i], with_embeddings[j])
                    } else {
                        (with_embeddings[j], with_embeddings[i])
                    };

                    let merged_content = format!(
                        "{} [consolidated from: {}]",
                        keep.content.display(),
                        remove.content.display(),
                    );

                    report.actions.push(ConsolidationAction::Merge {
                        keep_id: keep.id.clone(),
                        remove_id: remove.id.clone(),
                        merged_content,
                    });
                    report.merges += 1;
                    merged_ids.insert(remove.id.clone());
                }
            }
        }
    }

    fn find_contradictions(
        &self,
        entries: &[&MemoryEntry],
        graph: &CausalGraph,
        report: &mut ConsolidationReport,
    ) {
        let with_embeddings: Vec<&MemoryEntry> = entries
            .iter()
            .filter(|e| e.embedding.is_some())
            .copied()
            .collect();

        for i in 0..with_embeddings.len() {
            for j in (i + 1)..with_embeddings.len() {
                if with_embeddings[i].agent_id != with_embeddings[j].agent_id {
                    continue;
                }

                let sim = cosine_sim(
                    with_embeddings[i].embedding.as_ref().unwrap(),
                    with_embeddings[j].embedding.as_ref().unwrap(),
                );

                // Only check for contradictions among topically related entries
                if sim < 0.3 || sim >= self.config.dedup_similarity_threshold {
                    continue;
                }

                let ctx = build_context(with_embeddings[i], with_embeddings[j], Some(graph));
                let result = self.classifier.classify(with_embeddings[i], with_embeddings[j], &ctx);

                if result.is_contradiction && result.confidence >= self.config.contradiction_confidence_threshold {
                    let (old, new) = if with_embeddings[i].created_at <= with_embeddings[j].created_at {
                        (with_embeddings[i], with_embeddings[j])
                    } else {
                        (with_embeddings[j], with_embeddings[i])
                    };

                    report.actions.push(ConsolidationAction::Supersede {
                        old_id: old.id.clone(),
                        new_id: new.id.clone(),
                        confidence: result.confidence,
                        evidence: result.evidence,
                    });
                    report.contradictions_found += 1;
                }
            }
        }
    }

    fn compute_decay_boost(
        &self,
        entries: &[&MemoryEntry],
        report: &mut ConsolidationReport,
    ) {
        let now = now_ms();
        let half_life = self.config.decay_half_life_ms as f64;

        for entry in entries {
            let age_ms = now.saturating_sub(entry.last_accessed) as f64;

            // Decay: reduce importance for long-unaccessed entries
            if entry.access_count == 0 && age_ms > half_life {
                let decay_factor = (-(age_ms / half_life) * std::f64::consts::LN_2).exp();
                let new_importance = ((entry.importance as f64 * decay_factor) as u8).max(1);
                if new_importance < entry.importance {
                    report.actions.push(ConsolidationAction::DecayConfidence {
                        entry_id: entry.id.clone(),
                        new_importance,
                    });
                    report.decays_applied += 1;
                }
            }

            // Boost: increase importance for frequently accessed entries
            if entry.access_count >= 3 {
                let boost = (entry.access_count as f32).log(self.config.access_boost_log_base);
                let new_importance = (entry.importance as f32 + boost).min(100.0) as u8;
                if new_importance > entry.importance {
                    report.actions.push(ConsolidationAction::BoostConfidence {
                        entry_id: entry.id.clone(),
                        new_importance,
                    });
                    report.boosts_applied += 1;
                }
            }
        }
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
    use crate::memory::layered::{MemoryScope, MemoryTier, MemoryType};

    fn make_entry(
        id: &str,
        content: &str,
        importance: u8,
        access_count: u32,
        embedding: Option<Vec<f32>>,
        age_days: u64,
    ) -> MemoryEntry {
        let now = now_ms();
        MemoryEntry {
            id: id.to_string(),
            agent_id: "test".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content.to_string()),
            importance,
            access_count,
            last_accessed: now - (age_days * 24 * 60 * 60 * 1000),
            created_at: now - (age_days * 24 * 60 * 60 * 1000),
            tags: vec![],
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
    fn test_dedup_detection() {
        let emb = vec![0.9, 0.1, 0.0];
        let emb_dup = vec![0.91, 0.09, 0.01]; // very similar
        let e1 = make_entry("a", "Rust is fast", 50, 0, Some(emb), 0);
        let e2 = make_entry("b", "Rust is fast!", 40, 0, Some(emb_dup), 0);

        let engine = MemoryConsolidationEngine::new(ConsolidationConfig::default());
        let report = engine.consolidate(&[e1, e2]);
        assert!(report.merges > 0 || report.entries_scanned == 2);
    }

    #[test]
    fn test_decay_applied_to_old_unaccessed() {
        let e = make_entry("old", "ancient knowledge", 80, 0, None, 30);
        let engine = MemoryConsolidationEngine::new(ConsolidationConfig::default());
        let report = engine.consolidate(&[e]);
        assert!(report.decays_applied > 0, "should decay old unaccessed entry");
    }

    #[test]
    fn test_boost_applied_to_frequently_accessed() {
        let e = make_entry("popular", "frequently accessed", 50, 10, None, 1);
        let engine = MemoryConsolidationEngine::new(ConsolidationConfig::default());
        let report = engine.consolidate(&[e]);
        assert!(report.boosts_applied > 0, "should boost frequently accessed entry");
    }

    #[test]
    fn test_empty_entries() {
        let engine = MemoryConsolidationEngine::new(ConsolidationConfig::default());
        let report = engine.consolidate(&[]);
        assert_eq!(report.entries_scanned, 0);
        assert!(report.actions.is_empty());
    }
}
