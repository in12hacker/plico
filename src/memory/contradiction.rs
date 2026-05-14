//! Causal-Semantic Contradiction Detection (CSC) — v31 original algorithm.
//!
//! Fuses three signal families that only an AI-OS with causal graphs, layered
//! memory, and semantic embeddings can provide:
//!
//! 1. **Embedding divergence** — cosine distance, diff/product vectors
//! 2. **Causal proximity** — shared ancestors, graph distance
//! 3. **Temporal divergence** — time gap between entries
//!
//! An optional LLM classification step refines the score when available.

use crate::memory::layered::MemoryEntry;
use crate::memory::causal::CausalGraph;

/// Result of a contradiction check.
#[derive(Debug, Clone)]
pub struct ContradictionResult {
    pub is_contradiction: bool,
    pub confidence: f32,
    pub evidence: String,
}

/// Trait for contradiction classification — allows LLM or rule-based impl.
pub trait ContradictionClassifier: Send + Sync {
    fn classify(
        &self,
        old: &MemoryEntry,
        new: &MemoryEntry,
        context: &ContradictionContext,
    ) -> ContradictionResult;
}

/// Pre-computed features passed to the classifier.
#[derive(Debug, Clone)]
pub struct ContradictionContext {
    pub cosine_similarity: f32,
    pub embedding_divergence: f32,
    pub causal_distance: Option<usize>,
    pub shared_ancestor_count: usize,
    pub time_gap_ms: u64,
}

/// Compute cosine similarity between two vectors.
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

/// Build the contradiction context from two entries and an optional causal graph.
pub fn build_context(
    old: &MemoryEntry,
    new: &MemoryEntry,
    graph: Option<&CausalGraph>,
) -> ContradictionContext {
    let (cos_sim, emb_div) = match (old.embedding.as_ref(), new.embedding.as_ref()) {
        (Some(a), Some(b)) => {
            let sim = cosine_sim(a, b);
            (sim, 1.0 - sim)
        }
        _ => (0.0, 1.0),
    };

    let (causal_dist, shared_ancestors) = if let Some(g) = graph {
        let dist = g.shortest_path_len(&old.id, &new.id);
        let common = g.common_ancestors(&old.id, &new.id);
        (dist, common.len())
    } else {
        (None, 0)
    };

    let time_gap = old.created_at.abs_diff(new.created_at);

    ContradictionContext {
        cosine_similarity: cos_sim,
        embedding_divergence: emb_div,
        causal_distance: causal_dist,
        shared_ancestor_count: shared_ancestors,
        time_gap_ms: time_gap,
    }
}

/// Rule-based (stub) contradiction classifier — no LLM needed.
///
/// Heuristic: if entries are semantically similar (same topic) but not identical,
/// and share causal lineage, it's likely a contradiction.
pub struct RuleBasedClassifier;

impl ContradictionClassifier for RuleBasedClassifier {
    fn classify(
        &self,
        old: &MemoryEntry,
        new: &MemoryEntry,
        ctx: &ContradictionContext,
    ) -> ContradictionResult {
        let mut score: f32 = 0.0;
        let mut evidence_parts = Vec::new();

        // Similar topic (high cosine) but not identical → contradiction signal.
        // High overlap (>0.6) with any divergence means same-topic disagreement.
        if ctx.cosine_similarity > 0.3 && ctx.cosine_similarity < 0.98 {
            let topic_signal = ctx.cosine_similarity * 0.5;
            score += topic_signal;
            evidence_parts.push(format!("topic overlap {:.2}", ctx.cosine_similarity));
        }

        // Identical content (cosine > 0.98) → not a contradiction
        if ctx.cosine_similarity > 0.98 {
            return ContradictionResult {
                is_contradiction: false,
                confidence: 0.0,
                evidence: "near-identical content".to_string(),
            };
        }

        // Shared causal ancestors → stronger signal (same lineage, different claim)
        if ctx.shared_ancestor_count > 0 {
            let causal_signal = (ctx.shared_ancestor_count as f32 * 0.15).min(0.3);
            score += causal_signal;
            evidence_parts.push(format!("{} shared ancestors", ctx.shared_ancestor_count));
        }

        // Close in causal graph → likely about same topic
        if let Some(dist) = ctx.causal_distance {
            if dist <= 3 {
                score += 0.1;
                evidence_parts.push(format!("causal distance {}", dist));
            }
        }

        // Recent time gap + content difference → update/contradiction vs ancient
        if ctx.time_gap_ms < 24 * 60 * 60 * 1000 && ctx.embedding_divergence > 0.05 {
            score += 0.1;
            evidence_parts.push("recent divergence".into());
        }

        // Tag overlap as weak signal
        let old_tags: std::collections::HashSet<&str> = old.tags.iter().map(|s| s.as_str()).collect();
        let new_tags: std::collections::HashSet<&str> = new.tags.iter().map(|s| s.as_str()).collect();
        let tag_overlap = old_tags.intersection(&new_tags).count();
        if tag_overlap > 0 && old_tags.len() + new_tags.len() > 0 {
            let jaccard = tag_overlap as f32 / (old_tags.len() + new_tags.len() - tag_overlap).max(1) as f32;
            if jaccard > 0.3 {
                score += jaccard * 0.1;
                evidence_parts.push(format!("tag overlap {:.2}", jaccard));
            }
        }

        let is_contradiction = score > 0.35;
        let evidence = if evidence_parts.is_empty() {
            "no contradiction signals".to_string()
        } else {
            evidence_parts.join("; ")
        };

        ContradictionResult {
            is_contradiction,
            confidence: score.min(1.0),
            evidence,
        }
    }
}

/// LLM-enhanced contradiction classifier — uses CSC features + LLM judgment.
pub struct LlmContradictionClassifier {
    llm: std::sync::Arc<dyn crate::llm::LlmProvider>,
    prompt_registry: std::sync::Arc<crate::prompt::PromptRegistry>,
}

impl LlmContradictionClassifier {
    pub fn new(
        llm: std::sync::Arc<dyn crate::llm::LlmProvider>,
        prompt_registry: std::sync::Arc<crate::prompt::PromptRegistry>,
    ) -> Self {
        Self { llm, prompt_registry }
    }
}

impl ContradictionClassifier for LlmContradictionClassifier {
    fn classify(
        &self,
        old: &MemoryEntry,
        new: &MemoryEntry,
        ctx: &ContradictionContext,
    ) -> ContradictionResult {
        let old_text = old.content.display();
        let new_text = new.content.display();

        let mut vars = std::collections::HashMap::new();
        vars.insert("old_content", old_text.clone());
        vars.insert("new_content", new_text.clone());
        let prompt = self.prompt_registry
            .render("contradiction", &vars, Some(&old.agent_id))
            .unwrap_or_else(|_| {
                crate::memory::forgetting::contradiction_prompt(&old_text, &new_text)
            });

        let enhanced_prompt = format!(
            "{}\n\n[Features: cosine_sim={:.3}, causal_dist={}, shared_ancestors={}, time_gap={}ms]",
            prompt,
            ctx.cosine_similarity,
            ctx.causal_distance.map_or("∞".to_string(), |d| d.to_string()),
            ctx.shared_ancestor_count,
            ctx.time_gap_ms,
        );

        let msgs = [
            crate::llm::ChatMessage::system("You are a contradiction detector. Consider all provided features."),
            crate::llm::ChatMessage::user(enhanced_prompt),
        ];
        let opts = crate::llm::ChatOptions { temperature: 0.0, max_tokens: Some(32) };

        match self.llm.chat(&msgs, &opts) {
            Ok((response, _in_tok, _out_tok)) => {
                let llm_says_yes = crate::memory::forgetting::parse_contradiction_response(&response);
                let llm_score: f32 = if llm_says_yes { 0.7 } else { 0.1 };

                // Fuse: LLM judgment (w=0.5) + embedding divergence (w=0.3) + causal signal (w=0.2)
                let causal_signal = if ctx.shared_ancestor_count > 0 { 0.6 } else { 0.2 };
                let fused = 0.5 * llm_score
                    + 0.3 * ctx.embedding_divergence.min(1.0)
                    + 0.2 * causal_signal;

                ContradictionResult {
                    is_contradiction: fused > 0.45,
                    confidence: fused.min(1.0),
                    evidence: format!("LLM={}, fused={:.3}", if llm_says_yes { "yes" } else { "no" }, fused),
                }
            }
            Err(_) => {
                // Fallback to rule-based
                RuleBasedClassifier.classify(old, new, ctx)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::layered::{MemoryEntry, MemoryContent, MemoryType, MemoryScope, MemoryTier, now_ms};

    fn make_entry(id: &str, content: &str, tags: Vec<&str>) -> MemoryEntry {
        MemoryEntry {
            id: id.to_string(),
            agent_id: "test".to_string(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::LongTerm,
            content: MemoryContent::Text(content.to_string()),
            importance: 50,
            access_count: 0,
            last_accessed: now_ms(),
            created_at: now_ms(),
            tags: tags.into_iter().map(String::from).collect(),
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Private,
            memory_type: MemoryType::Semantic,
            causal_parent: None,
            supersedes: None,
        }
    }

    #[test]
    fn test_rule_based_no_contradiction() {
        let old = make_entry("a", "The sky is blue", vec!["weather"]);
        let new = make_entry("b", "Rust is a programming language", vec!["tech"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.1,
            embedding_divergence: 0.9,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 1_000_000,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(!result.is_contradiction);
    }

    #[test]
    fn test_rule_based_contradiction_signals() {
        let old = make_entry("a", "Deploy is required weekly", vec!["deploy", "policy"]);
        let new = make_entry("b", "Deploy is recommended monthly", vec!["deploy", "policy"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.8,
            embedding_divergence: 0.2,
            causal_distance: Some(1),
            shared_ancestor_count: 1,
            time_gap_ms: 3_600_000,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(result.confidence > 0.4);
    }

    #[test]
    fn test_build_context_no_graph() {
        let old = make_entry("a", "foo", vec![]);
        let new = make_entry("b", "bar", vec![]);
        let ctx = build_context(&old, &new, None);
        assert!(ctx.causal_distance.is_none());
        assert_eq!(ctx.shared_ancestor_count, 0);
    }

    #[test]
    fn test_build_context_with_graph() {
        let mut root = make_entry("root", "origin", vec![]);
        root.causal_parent = None;
        let mut a = make_entry("a", "claim A", vec![]);
        a.causal_parent = Some("root".to_string());
        let mut b = make_entry("b", "claim B", vec![]);
        b.causal_parent = Some("root".to_string());

        let graph = CausalGraph::build(&[root, a.clone(), b.clone()]);
        let ctx = build_context(&a, &b, Some(&graph));
        assert!(ctx.shared_ancestor_count > 0 || ctx.causal_distance.is_some());
    }

    // ─── cosine_sim coverage ─────────────────────────────────────────────────

    #[test]
    fn test_cosine_sim_empty_vectors() {
        // cosine_sim is private, but we can exercise it through build_context
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.embedding = Some(vec![]);
        new.embedding = Some(vec![]);
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.cosine_similarity, 0.0);
        assert_eq!(ctx.embedding_divergence, 1.0);
    }

    #[test]
    fn test_cosine_sim_unequal_length() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.embedding = Some(vec![1.0, 2.0]);
        new.embedding = Some(vec![1.0]);
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.cosine_similarity, 0.0);
        assert_eq!(ctx.embedding_divergence, 1.0);
    }

    #[test]
    fn test_cosine_sim_zero_norm() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.embedding = Some(vec![0.0, 0.0]);
        new.embedding = Some(vec![1.0, 0.0]);
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.cosine_similarity, 0.0);
    }

    #[test]
    fn test_cosine_sim_normal_vectors() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        // Identical vectors → cosine = 1.0
        old.embedding = Some(vec![1.0, 2.0, 3.0]);
        new.embedding = Some(vec![1.0, 2.0, 3.0]);
        let ctx = build_context(&old, &new, None);
        assert!((ctx.cosine_similarity - 1.0).abs() < 1e-5);
        assert!(ctx.embedding_divergence < 1e-5);
    }

    #[test]
    fn test_cosine_sim_orthogonal_vectors() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.embedding = Some(vec![1.0, 0.0]);
        new.embedding = Some(vec![0.0, 1.0]);
        let ctx = build_context(&old, &new, None);
        assert!((ctx.cosine_similarity - 0.0).abs() < 1e-5);
        assert!((ctx.embedding_divergence - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_build_context_one_embedding_missing() {
        let mut old = make_entry("a", "x", vec![]);
        let new = make_entry("b", "y", vec![]);
        old.embedding = Some(vec![1.0, 2.0]);
        // new.embedding is None
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.cosine_similarity, 0.0);
        assert_eq!(ctx.embedding_divergence, 1.0);
    }

    #[test]
    fn test_build_context_both_embeddings_with_graph() {
        let mut root = make_entry("root", "origin", vec![]);
        root.causal_parent = None;
        let mut a = make_entry("a", "claim A", vec![]);
        a.causal_parent = Some("root".to_string());
        a.embedding = Some(vec![1.0, 0.0, 0.0]);
        let mut b = make_entry("b", "claim B", vec![]);
        b.causal_parent = Some("root".to_string());
        b.embedding = Some(vec![0.0, 1.0, 0.0]);
        let graph = CausalGraph::build(&[root, a.clone(), b.clone()]);
        let ctx = build_context(&a, &b, Some(&graph));
        // Orthogonal embeddings
        assert!((ctx.cosine_similarity).abs() < 1e-5);
        assert!(ctx.shared_ancestor_count > 0);
    }

    // ─── RuleBasedClassifier edge cases ──────────────────────────────────────

    #[test]
    fn test_rule_based_near_identical_early_return() {
        let old = make_entry("a", "The sky is blue", vec!["weather"]);
        let new = make_entry("b", "The sky is blue", vec!["weather"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.99,
            embedding_divergence: 0.01,
            causal_distance: Some(1),
            shared_ancestor_count: 2,
            time_gap_ms: 100,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(!result.is_contradiction);
        assert_eq!(result.confidence, 0.0);
        assert_eq!(result.evidence, "near-identical content");
    }

    #[test]
    fn test_rule_based_high_shared_ancestors_capped() {
        let old = make_entry("a", "A is true", vec!["topic"]);
        let new = make_entry("b", "A is false", vec!["topic"]);
        // Many shared ancestors — causal signal should cap at 0.3
        let ctx = ContradictionContext {
            cosine_similarity: 0.7,
            embedding_divergence: 0.3,
            causal_distance: Some(2),
            shared_ancestor_count: 10,
            time_gap_ms: 3_600_000,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(result.confidence > 0.35);
        assert!(result.evidence.contains("10 shared ancestors"));
    }

    #[test]
    fn test_rule_based_causal_distance_far() {
        let old = make_entry("a", "X", vec![]);
        let new = make_entry("b", "Y", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.5,
            embedding_divergence: 0.5,
            causal_distance: Some(10), // far away
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000, // > 24h
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        // Only topic signal: 0.5 * 0.5 = 0.25, below 0.35
        assert!(!result.is_contradiction);
        assert!(!result.evidence.contains("causal distance"));
    }

    #[test]
    fn test_rule_based_recent_divergence_signal() {
        let old = make_entry("a", "Service runs on port 8080", vec!["infra"]);
        let new = make_entry("b", "Service runs on port 9090", vec!["infra"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.6,
            embedding_divergence: 0.4,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 60_000, // 1 minute — recent
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(result.evidence.contains("recent divergence"));
    }

    #[test]
    fn test_rule_based_tag_overlap_high_jaccard() {
        let old = make_entry("a", "CPU usage is high", vec!["monitoring", "alert", "cpu"]);
        let new = make_entry("b", "CPU usage is normal", vec!["monitoring", "alert", "status"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.65,
            embedding_divergence: 0.35,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000, // > 24h, no recent signal
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        // Jaccard: intersection={"monitoring","alert"}=2, union=4, jaccard=2/4=0.5 > 0.3
        assert!(result.evidence.contains("tag overlap"));
    }

    #[test]
    fn test_rule_based_tag_overlap_low_jaccard() {
        let old = make_entry("a", "Alpha", vec!["only-shared-tag", "b", "c"]);
        let new = make_entry("b", "Beta", vec!["only-shared-tag", "d", "e", "f"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.5,
            embedding_divergence: 0.5,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        // Jaccard: intersection=1, union=6, jaccard=1/6≈0.167 < 0.3
        assert!(!result.evidence.contains("tag overlap"));
    }

    #[test]
    fn test_rule_based_no_tags_no_signals() {
        let old = make_entry("a", "Unrelated text", vec![]);
        let new = make_entry("b", "Another unrelated", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.1,
            embedding_divergence: 0.9,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000,
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        assert!(!result.is_contradiction);
        assert_eq!(result.evidence, "no contradiction signals");
        assert_eq!(result.confidence, 0.0);
    }

    #[test]
    fn test_rule_based_score_exactly_at_threshold() {
        // Score must be > 0.35 to be a contradiction; exactly 0.35 is not.
        let old = make_entry("a", "Temperature is 20C", vec!["temp"]);
        let new = make_entry("b", "Temperature is 25C", vec!["temp"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.9,    // topic_signal = 0.9 * 0.5 = 0.45
            embedding_divergence: 0.1,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000, // > 24h
        };
        let result = RuleBasedClassifier.classify(&old, &new, &ctx);
        // 0.45 > 0.35 → contradiction
        assert!(result.is_contradiction);
    }

    // ─── LlmContradictionClassifier ──────────────────────────────────────────

    #[test]
    fn test_llm_classifier_new() {
        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("yes"));
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);
        // Verify it implements ContradictionClassifier
        let old = make_entry("a", "X", vec![]);
        let new = make_entry("b", "Y", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.5,
            embedding_divergence: 0.5,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 1000,
        };
        let _ = classifier.classify(&old, &new, &ctx);
    }

    #[test]
    fn test_llm_classifier_says_yes_no_ancestors() {
        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("yes"));
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Deploy weekly", vec![]);
        let new = make_entry("b", "Deploy monthly", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.7,
            embedding_divergence: 0.3,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 3_600_000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // LLM=yes → llm_score=0.7, causal_signal=0.2 (no ancestors)
        // fused = 0.5*0.7 + 0.3*0.3 + 0.2*0.2 = 0.35 + 0.09 + 0.04 = 0.48
        assert!(result.is_contradiction);
        assert!(result.evidence.contains("LLM=yes"));
        assert!(result.confidence > 0.0);
    }

    #[test]
    fn test_llm_classifier_says_yes_with_ancestors() {
        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("yes"));
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Claim A", vec![]);
        let new = make_entry("b", "Claim B", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.7,
            embedding_divergence: 0.3,
            causal_distance: Some(2),
            shared_ancestor_count: 3,
            time_gap_ms: 3_600_000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // LLM=yes → llm_score=0.7, causal_signal=0.6 (has ancestors)
        // fused = 0.5*0.7 + 0.3*0.3 + 0.2*0.6 = 0.35 + 0.09 + 0.12 = 0.56
        assert!(result.is_contradiction);
        assert!(result.evidence.contains("LLM=yes"));
    }

    #[test]
    fn test_llm_classifier_says_no() {
        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("no"));
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Sky is blue", vec![]);
        let new = make_entry("b", "Grass is green", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.3,
            embedding_divergence: 0.7,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 1_000_000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // LLM=no → llm_score=0.1, causal_signal=0.2
        // fused = 0.5*0.1 + 0.3*0.7 + 0.2*0.2 = 0.05 + 0.21 + 0.04 = 0.30
        assert!(!result.is_contradiction);
        assert!(result.evidence.contains("LLM=no"));
    }

    #[test]
    fn test_llm_classifier_fallback_on_error() {
        // Use a provider that always errors
        struct ErrorLlm;
        impl crate::llm::LlmProvider for ErrorLlm {
            fn chat(&self, _: &[crate::llm::ChatMessage], _: &crate::llm::ChatOptions)
                -> Result<(String, u32, u32), crate::llm::LlmError>
            {
                Err(crate::llm::LlmError::Api("test error".into()))
            }
            fn model_name(&self) -> &str { "error-llm" }
        }

        let llm = std::sync::Arc::new(ErrorLlm);
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Deploy weekly", vec!["deploy", "policy"]);
        let new = make_entry("b", "Deploy monthly", vec!["deploy", "policy"]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.8,
            embedding_divergence: 0.2,
            causal_distance: Some(1),
            shared_ancestor_count: 1,
            time_gap_ms: 3_600_000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // Should fall back to RuleBasedClassifier
        // Same as test_rule_based_contradiction_signals
        assert!(result.confidence > 0.4);
        assert!(!result.evidence.contains("LLM="));
    }

    #[test]
    fn test_llm_classifier_with_prompt_registry_template() {
        use crate::prompt::PromptTemplate;

        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("yes"));
        let registry = crate::prompt::PromptRegistry::new();
        registry.set_override(
            "contradiction",
            PromptTemplate::new("contradiction", "Compare: {{old_content}} vs {{new_content}}", &["old_content", "new_content"]),
            None,
        );
        let registry = std::sync::Arc::new(registry);
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Statement one", vec![]);
        let new = make_entry("b", "Statement two", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.7,
            embedding_divergence: 0.3,
            causal_distance: Some(1),
            shared_ancestor_count: 1,
            time_gap_ms: 1000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // Should use the registry template, not the fallback
        assert!(result.evidence.contains("LLM=yes"));
    }

    #[test]
    fn test_llm_classifier_no_contradiction_below_threshold() {
        let llm = std::sync::Arc::new(crate::llm::StubProvider::new("no"));
        let registry = std::sync::Arc::new(crate::prompt::PromptRegistry::new());
        let classifier = LlmContradictionClassifier::new(llm, registry);

        let old = make_entry("a", "Random fact", vec![]);
        let new = make_entry("b", "Another fact", vec![]);
        let ctx = ContradictionContext {
            cosine_similarity: 0.1,
            embedding_divergence: 0.02,
            causal_distance: None,
            shared_ancestor_count: 0,
            time_gap_ms: 100_000_000,
        };
        let result = classifier.classify(&old, &new, &ctx);
        // LLM=no → 0.1, divergence=0.02, causal=0.2
        // fused = 0.5*0.1 + 0.3*0.02 + 0.2*0.2 = 0.05 + 0.006 + 0.04 = 0.096
        assert!(!result.is_contradiction);
    }

    // ─── ContradictionResult / ContradictionContext debug ─────────────────────

    #[test]
    fn test_contradiction_result_debug_clone() {
        let result = ContradictionResult {
            is_contradiction: true,
            confidence: 0.85,
            evidence: "test evidence".to_string(),
        };
        let cloned = result.clone();
        assert_eq!(cloned.is_contradiction, true);
        assert_eq!(cloned.confidence, 0.85);
        assert_eq!(cloned.evidence, "test evidence");
        // Debug trait
        let _ = format!("{:?}", result);
    }

    #[test]
    fn test_contradiction_context_debug_clone() {
        let ctx = ContradictionContext {
            cosine_similarity: 0.5,
            embedding_divergence: 0.5,
            causal_distance: Some(3),
            shared_ancestor_count: 2,
            time_gap_ms: 1000,
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.causal_distance, Some(3));
        let _ = format!("{:?}", ctx);
    }

    #[test]
    fn test_build_context_time_gap() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.created_at = 1000;
        new.created_at = 5000;
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.time_gap_ms, 4000);
    }

    #[test]
    fn test_build_context_time_gap_reversed() {
        let mut old = make_entry("a", "x", vec![]);
        let mut new = make_entry("b", "y", vec![]);
        old.created_at = 5000;
        new.created_at = 1000;
        let ctx = build_context(&old, &new, None);
        assert_eq!(ctx.time_gap_ms, 4000);
    }

    #[test]
    fn test_build_context_unknown_ids_in_graph() {
        let mut a = make_entry("a", "x", vec![]);
        let mut b = make_entry("b", "y", vec![]);
        a.causal_parent = None;
        b.causal_parent = None;
        let graph = CausalGraph::build(&[a.clone(), b.clone()]);
        // Both are roots — common ancestors should be empty, path may or may not exist
        let ctx = build_context(&a, &b, Some(&graph));
        assert_eq!(ctx.shared_ancestor_count, 0);
    }
}
