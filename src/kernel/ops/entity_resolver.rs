//! Active Entity Linking — 3-tier resolution pipeline for cross-session entity matching.
//!
//! Tier 1: Exact label match (case-insensitive)
//! Tier 2: Embedding-based semantic similarity (threshold 0.85)
//! Tier 3: Alias propagation — create IsAliasOf edge and merge aliases
//!
//! Design doc: `docs/design/plico-kg-entity-design.md` section 4.2

use std::sync::Arc;

use crate::fs::embedding::EmbeddingProvider;
use crate::fs::{KnowledgeGraph, KGEdge, KGEdgeType, KGNode, KGNodeType};

/// Cosine similarity between two f32 vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// Entity resolution result.
pub struct ResolutionResult {
    /// The node ID of the matched existing entity, or None if new.
    pub resolved_id: Option<String>,
    /// The embedding of the label (for storage in properties).
    pub embedding: Vec<f32>,
}

pub struct EntityResolver {
    kg: Arc<dyn KnowledgeGraph>,
    embedder: Arc<dyn EmbeddingProvider>,
    threshold: f32,
}

impl EntityResolver {
    pub fn new(
        kg: Arc<dyn KnowledgeGraph>,
        embedder: Arc<dyn EmbeddingProvider>,
        threshold: f32,
    ) -> Self {
        Self {
            kg,
            embedder,
            threshold,
        }
    }

    /// Resolve an entity label to an existing node ID.
    ///
    /// Tier 1: Exact label match (case-insensitive)
    /// Tier 2: Embedding similarity against stored entity embeddings
    ///
    /// Returns `ResolutionResult` with the matched node ID (if any) and the label embedding.
    pub fn resolve(
        &self,
        label: &str,
        node_type: KGNodeType,
        agent_id: &str,
    ) -> Result<ResolutionResult, String> {
        // Compute embedding for the label (needed for Tier 2 and for storage)
        let embed_result = self
            .embedder
            .embed(label)
            .map_err(|e| format!("Entity embedding failed: {}", e))?;
        let embedding = embed_result.embedding;

        // Tier 1: Exact label match
        if let Ok(nodes) = self.kg.list_nodes(agent_id, Some(node_type)) {
            // Exact match
            if let Some(existing) = nodes
                .iter()
                .find(|n| n.label.eq_ignore_ascii_case(label))
            {
                return Ok(ResolutionResult {
                    resolved_id: Some(existing.id.clone()),
                    embedding,
                });
            }

            // Alias match
            for node in &nodes {
                if let Some(aliases) = node.properties.get("aliases").and_then(|a| a.as_array()) {
                    if aliases.iter().any(|a| {
                        a.as_str()
                            .map(|s| s.eq_ignore_ascii_case(label))
                            .unwrap_or(false)
                    }) {
                        return Ok(ResolutionResult {
                            resolved_id: Some(node.id.clone()),
                            embedding,
                        });
                    }
                }
            }

            // Tier 2: Embedding-based semantic match
            let mut best_match: Option<(&KGNode, f32)> = None;
            for node in &nodes {
                if let Some(stored_emb) = node
                    .properties
                    .get("embedding")
                    .and_then(|v| v.as_array())
                {
                    let stored: Vec<f32> = stored_emb
                        .iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect();
                    if stored.len() == embedding.len() {
                        let sim = cosine_similarity(&embedding, &stored);
                        if sim >= self.threshold {
                            match best_match {
                                Some((_, best_sim)) if best_sim >= sim => {}
                                _ => best_match = Some((node, sim)),
                            }
                        }
                    }
                }
            }

            if let Some((matched, _sim)) = best_match {
                return Ok(ResolutionResult {
                    resolved_id: Some(matched.id.clone()),
                    embedding,
                });
            }
        }

        // No match found — this is a new entity
        Ok(ResolutionResult {
            resolved_id: None,
            embedding,
        })
    }

    /// Store entity embedding in the node's properties and create IsAliasOf edge
    /// if this entity resolved to an existing one with a different label.
    pub fn link_and_store(
        &self,
        node_id: &str,
        label: &str,
        resolved_id: &str,
        embedding: &[f32],
        agent_id: &str,
        tenant_id: &str,
        created_at: u64,
    ) -> Result<(), String> {
        // Store embedding in node properties
        if let Ok(Some(mut node)) = self.kg.get_node(node_id) {
            let emb_json: Vec<serde_json::Value> =
                embedding.iter().map(|v| serde_json::json!(*v)).collect();
            node.properties["embedding"] = serde_json::Value::Array(emb_json);

            // If resolved to a different node, propagate aliases and create IsAliasOf edge
            if node_id != resolved_id {
                // Add current label as alias on the resolved node
                if let Ok(Some(mut resolved_node)) = self.kg.get_node(resolved_id) {
                    if !resolved_node.label.eq_ignore_ascii_case(label) {
                        let mut aliases = resolved_node
                            .properties
                            .get("aliases")
                            .and_then(|a| a.as_array())
                            .cloned()
                            .unwrap_or_default();
                        if !aliases.iter().any(|a| {
                            a.as_str()
                                .map(|s| s.eq_ignore_ascii_case(label))
                                .unwrap_or(false)
                        }) {
                            aliases.push(serde_json::Value::String(label.to_string()));
                            resolved_node.properties["aliases"] =
                                serde_json::Value::Array(aliases);
                            let _ = self.kg.add_node(resolved_node);
                        }
                    }
                }

                // Create IsAliasOf edge from new node to resolved node
                let alias_edge = KGEdge::new_with_episode(
                    node_id.to_string(),
                    resolved_id.to_string(),
                    KGEdgeType::IsAliasOf,
                    0.9,
                    format!("entity_resolver:{}", agent_id),
                );
                let _ = self.kg.add_edge(alias_edge);
            }

            let _ = self.kg.add_node(node);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::embedding::{EmbedResult, EmbeddingProvider};
    use crate::fs::graph::PetgraphBackend;

    /// Stub embedder for tests: returns deterministic embeddings based on text hash.
    struct StubEmbedder {
        dim: usize,
    }

    impl EmbeddingProvider for StubEmbedder {
        fn embed(&self, text: &str) -> Result<EmbedResult, crate::fs::embedding::EmbedError> {
            // Simple hash-based deterministic embedding
            let mut emb = vec![0.0f32; self.dim];
            for (i, byte) in text.bytes().enumerate() {
                emb[i % self.dim] += (byte as f32) / 255.0;
            }
            // Normalize
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for v in &mut emb {
                    *v /= norm;
                }
            }
            Ok(EmbedResult {
                embedding: emb,
                input_tokens: text.len() as u32,
            })
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, crate::fs::embedding::EmbedError> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimension(&self) -> usize {
            self.dim
        }

        fn model_name(&self) -> &str {
            "stub-entity-resolver"
        }
    }

    fn setup() -> (EntityResolver, Arc<PetgraphBackend>) {
        let kg: Arc<PetgraphBackend> = Arc::new(PetgraphBackend::open(std::env::temp_dir().join(format!(
            "plico_test_entity_{}",
            std::process::id()
        ))));
        let embedder = Arc::new(StubEmbedder { dim: 32 });
        let resolver = EntityResolver::new(kg.clone(), embedder, 0.85);
        (resolver, kg)
    }

    #[test]
    fn test_exact_label_match() {
        let (resolver, kg) = setup();

        // Create an entity
        let mut node = KGNode::new("Leo".into(), KGNodeType::Entity, "agent1".into(), "t1".into());
        node.id = "ent:Leo".into();
        kg.add_node(node).unwrap();

        // Resolve "Leo" — should match exactly
        let result = resolver.resolve("Leo", KGNodeType::Entity, "agent1").unwrap();
        assert_eq!(result.resolved_id, Some("ent:Leo".into()));
    }

    #[test]
    fn test_alias_match() {
        let (resolver, kg) = setup();

        // Create entity with alias
        let mut node = KGNode::new("Leo".into(), KGNodeType::Entity, "agent1".into(), "t1".into());
        node.id = "ent:Leo".into();
        node.properties["aliases"] = serde_json::json!(["CEO of Plico"]);
        kg.add_node(node).unwrap();

        // Resolve "CEO of Plico" — should match via alias
        let result = resolver.resolve("CEO of Plico", KGNodeType::Entity, "agent1").unwrap();
        assert_eq!(result.resolved_id, Some("ent:Leo".into()));
    }

    #[test]
    fn test_no_match_returns_none() {
        let (resolver, _kg) = setup();

        let result = resolver.resolve("Unknown", KGNodeType::Entity, "agent1").unwrap();
        assert_eq!(result.resolved_id, None);
    }
}
