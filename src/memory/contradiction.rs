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
            time_gap_ms: 3600_000,
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
}
