//! Hybrid Retrieval — Graph-RAG primitive combining vector search and KG traversal.
//!
//! Processing flow:
//! 1. Vector search: query_text → embedding → search_backend.search()
//! 2. KG seed expansion: for each vector result, find corresponding KG node
//! 3. Graph traversal: from seed nodes, traverse specified edge_types for graph_depth hops
//! 4. Merge + deduplicate: combined_score = α × vector_score + (1-α) × graph_score, α = 0.6
//! 5. Token budget pruning: accumulate by combined_score descending until budget reached
//! 6. Return results with provenance (causal path from query to result)
//!
//! Design: F-11 in docs/design-node4-collaborative-ecosystem.md

use std::collections::{HashMap, HashSet};

use crate::api::semantic::{HybridHit, HybridResult, ProvenanceStep};
use crate::fs::embedding::types::EmbeddingProvider;
use crate::fs::{KGEdgeType, SearchFilter, SearchHit};
use crate::kernel::AIKernel;

/// Default combination weight: 60% vector, 40% graph.
const DEFAULT_ALPHA: f32 = 0.6;

/// Maximum content preview length in characters.
const MAX_PREVIEW_LEN: usize = 200;

/// Result of graph traversal: node with its score and provenance chain.
struct GraphTraversalResult {
    node: crate::fs::KGNode,
    graph_score: f32,
    provenance: Vec<ProvenanceStep>,
}

impl AIKernel {
    /// Perform hybrid retrieval combining vector search and knowledge graph traversal.
    ///
    /// Returns a `HybridResult` with items ordered by combined score (descending),
    /// each item carrying provenance showing the causal path from query to result.
    pub fn hybrid_retrieve(
        &self,
        query_text: &str,
        _seed_tags: &[String],
        graph_depth: u8,
        edge_types: &[String],
        max_results: usize,
        token_budget: Option<usize>,
    ) -> HybridResult {
        let span = tracing::info_span!(
            "hybrid_retrieve",
            operation = "hybrid_retrieve",
            query_text = %query_text,
            graph_depth = graph_depth,
            edge_types = ?edge_types,
            max_results = max_results,
        );
        let _guard = span.enter();

        // Step 1: Vector search (may return 0 under stub embedding)
        let vector_results = self.vector_search(query_text, max_results * 2);
        tracing::debug!(vector_hits = vector_results.len(), "Vector search completed");

        // Step 1b: BM25 search (always available, even with stub embedding) — F-44
        let bm25_results: Vec<(String, f32)> = self.fs.bm25_search(query_text, max_results * 2);

        // Step 2: KG seed expansion — from vector OR bm25 results (F-44 fallback)
        let mut graph_seeds: Vec<(String, f32)> = Vec::new();
        if let Some(ref kg) = self.knowledge_graph {
            // Primary: vector results
            for hit in &vector_results {
                if let Ok(Some(node)) = kg.get_node(&hit.cid) {
                    graph_seeds.push((node.id.clone(), hit.score));
                }
            }
            // Fallback: BM25 results (when vector yields nothing or sparse)
            if graph_seeds.len() < 2 && !bm25_results.is_empty() {
                tracing::debug!("F-44: vector seeds sparse ({}), using BM25 fallback", graph_seeds.len());
                for (cid, score) in &bm25_results {
                    if let Ok(Some(node)) = kg.get_node(cid) {
                        graph_seeds.push((node.id.clone(), *score));
                    }
                }
            }
        }
        tracing::debug!(seed_nodes = graph_seeds.len(), "KG seeds populated");

        // Step 3: Graph traversal from seeds
        let (graph_hits, path_count) = self.graph_traverse(&graph_seeds, edge_types, graph_depth);
        tracing::debug!(
            graph_hits = graph_hits.len(),
            paths = path_count,
            "Graph traversal completed"
        );

        // Build CID → graph_score and provenance maps
        let graph_score_map: HashMap<String, f32> = graph_hits
            .iter()
            .map(|r| (r.node.id.clone(), r.graph_score))
            .collect();

        let provenance_map: HashMap<String, Vec<ProvenanceStep>> = graph_hits
            .iter()
            .map(|r| (r.node.id.clone(), r.provenance.clone()))
            .collect();

        // Step 4: RRF fusion — proper Reciprocal Rank Fusion combining vector and BM25
        // Both retrieval methods get rank-based scoring for fair fusion (not score-based)
        // This ensures BM25 contributes even when vector search is strong
        const RRF_K: usize = 60;
        let mut rrf_scores: HashMap<String, (f32, f32)> = HashMap::new(); // cid -> (rrf_score, bm25_contribution)

        // Add vector hits with rank-based scoring
        let mut sorted_vector: Vec<_> = vector_results.iter().collect();
        sorted_vector.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        for (rank, hit) in sorted_vector.iter().enumerate() {
            let rrf = 1.0f32 / (RRF_K as f32 + rank as f32);
            rrf_scores.insert(hit.cid.clone(), (rrf, 0.0));
        }

        // Add BM25 hits with rank-based scoring — ALWAYS fuse, not just as fallback
        for (rank, (cid, _score)) in bm25_results.iter().enumerate() {
            let rrf = 1.0f32 / (RRF_K as f32 + rank as f32);
            if let Some((existing_rrf, _)) = rrf_scores.get_mut(cid) {
                // Already in vector results — add BM25 RRF bonus
                *existing_rrf += rrf;
            } else {
                // BM25-only result
                rrf_scores.insert(cid.clone(), (rrf, rrf));
            }
        }

        // Build HybridHit list with proper fusion
        let mut hits: Vec<HybridHit> = Vec::new();
        for (cid, (rrf_score, _bm25_contribution)) in rrf_scores {
            let vector_score = vector_results.iter().find(|h| h.cid == cid).map(|h| h.score).unwrap_or(0.0);
            let graph_score = graph_score_map.get(&cid).copied().unwrap_or(0.0);
            let provenance = provenance_map.get(&cid).cloned().unwrap_or_default();

            // Combine RRF fusion score with graph score
            let combined_score = rrf_score + (1.0 - DEFAULT_ALPHA) * graph_score;

            let content_preview = if vector_score > 0.0 {
                vector_results.iter().find(|h| h.cid == cid).map(|h| h.meta.snippet.clone()).unwrap_or_default()
            } else {
                self.get_content_preview(&cid)
            };

            hits.push(HybridHit {
                cid,
                content_preview,
                vector_score,
                graph_score,
                combined_score,
                provenance,
            });
        }

        // Then add graph-only results (skip CIDs already in fused results)
        let fused_cids: HashSet<String> = hits.iter().map(|h| h.cid.clone()).collect();
        for result in &graph_hits {
            if fused_cids.contains(&result.node.id) {
                continue;
            }

            // Graph-only hits have no vector score
            let combined_score = (1.0 - DEFAULT_ALPHA) * result.graph_score;

            // Get content preview from label or content_cid
            let content_preview = if let Some(ref cid) = result.node.content_cid {
                // Try to fetch content preview from CAS
                self.get_content_preview(cid)
            } else {
                result.node.label.clone()
            };

            hits.push(HybridHit {
                cid: result.node.id.clone(),
                content_preview,
                vector_score: 0.0,
                graph_score: result.graph_score,
                combined_score,
                provenance: result.provenance.clone(),
            });
        }

        // Sort by combined_score descending
        hits.sort_by(|a, b| b.combined_score.partial_cmp(&a.combined_score).unwrap_or(std::cmp::Ordering::Equal));

        // Step 5: Token budget pruning
        let mut token_estimate_total = 0usize;
        let mut vector_hits_count = 0usize;
        let mut graph_hits_count = 0usize;

        if let Some(budget) = token_budget {
            hits.retain(|hit| {
                let item_tokens = estimate_tokens_for_hit(hit);
                if token_estimate_total + item_tokens <= budget {
                    token_estimate_total += item_tokens;
                    true
                } else {
                    false
                }
            });
        } else {
            token_estimate_total = hits.iter().map(estimate_tokens_for_hit).sum();
        }

        // Count vector vs graph hits
        for hit in &hits {
            if hit.vector_score > 0.0 {
                vector_hits_count += 1;
            }
            if hit.graph_score > 0.0 {
                graph_hits_count += 1;
            }
        }

        // Limit to max_results
        hits.truncate(max_results);

        HybridResult {
            items: hits,
            token_estimate: token_estimate_total,
            vector_hits: vector_hits_count,
            graph_hits: graph_hits_count,
            paths_found: path_count,
        }
    }

    /// Vector search: embed query and search the semantic index.
    fn vector_search(&self, query_text: &str, limit: usize) -> Vec<SearchHit> {
        // Get embedding for the query
        let embedding: Vec<f32> = match self.embedding.embed(query_text) {
            Ok(e) => e.embedding,
            Err(e) => {
                tracing::warn!("embedding failed: {}", e);
                return Vec::new();
            }
        };

        // Search the semantic index
        let filter = SearchFilter::default();
        self.search_backend.search(&embedding, limit, &filter)
    }

    /// Graph traversal from seed nodes, returning results with provenance.
    fn graph_traverse(
        &self,
        seeds: &[(String, f32)],
        edge_types: &[String],
        depth: u8,
    ) -> (Vec<GraphTraversalResult>, usize) {
        let Some(ref kg) = self.knowledge_graph else {
            return (Vec::new(), 0);
        };

        let edge_type_filter: Option<Vec<KGEdgeType>> = if edge_types.is_empty() {
            None
        } else {
            Some(parse_edge_types(edge_types))
        };

        let mut visited: HashSet<String> = HashSet::new();
        let mut results: Vec<GraphTraversalResult> = Vec::new();
        let mut paths_found = 0usize;

        // BFS traversal
        let mut queue: Vec<(String, f32, u8, Vec<ProvenanceStep>)> = seeds
            .iter()
            .map(|(id, score)| (id.clone(), *score, 0u8, Vec::new()))
            .collect();

        while let Some((current_id, incoming_score, current_depth, provenance)) = queue.pop() {
            if visited.contains(&current_id) {
                continue;
            }
            if current_depth > depth {
                continue;
            }

            visited.insert(current_id.clone());

            // Get neighbors
            let edge_type_enum = edge_type_filter.as_ref().and_then(|types| types.first().copied());

            let neighbors = match kg.get_neighbors(&current_id, edge_type_enum, 1) {
                Ok(n) => n,
                Err(_) => continue,
            };

            for (neighbor, edge) in neighbors {
                let edge_type_str = format!("{:?}", edge.edge_type).to_lowercase();
                let hop = current_depth + 1;

                // Build provenance step
                let step = ProvenanceStep {
                    from_cid: current_id.clone(),
                    edge_type: edge_type_str,
                    hop,
                };
                let mut new_provenance = provenance.clone();
                new_provenance.push(step);

                // Score propagates through edges
                let propagated_score = incoming_score * edge.weight;

                // Check if this is a document node (has content_cid)
                if neighbor.content_cid.is_some() {
                    // This is a document node — add to results with provenance
                    results.push(GraphTraversalResult {
                        node: neighbor.clone(),
                        graph_score: propagated_score,
                        provenance: new_provenance.clone(),
                    });
                    paths_found += 1;
                }

                // Continue traversal
                queue.push((neighbor.id, propagated_score, hop, new_provenance));
            }
        }

        (results, paths_found)
    }

    /// Get content preview for a CID from CAS.
    fn get_content_preview(&self, cid: &str) -> String {
        match self.get_object(cid, "system", "default") {
            Ok(obj) => {
                let content = String::from_utf8_lossy(&obj.data);
                content.chars().take(MAX_PREVIEW_LEN).collect()
            }
            Err(_) => cid.chars().take(MAX_PREVIEW_LEN).collect(),
        }
    }
}

/// Estimate token count for a single HybridHit (rough approximation).
fn estimate_tokens_for_hit(hit: &HybridHit) -> usize {
    // Rough estimate: content_preview / 4 + overhead for metadata
    let preview_tokens = hit.content_preview.len().div_ceil(4);
    let provenance_tokens = hit.provenance.len() * 20; // ~20 tokens per step
    preview_tokens + provenance_tokens + 50 // base overhead
}

/// Parse edge type strings into KGEdgeType enums.
fn parse_edge_types(types: &[String]) -> Vec<KGEdgeType> {
    types
        .iter()
        .filter_map(|t| match t.to_lowercase().as_str() {
            "associates_with" => Some(KGEdgeType::AssociatesWith),
            "mentions" => Some(KGEdgeType::Mentions),
            "follows" => Some(KGEdgeType::Follows),
            "part_of" => Some(KGEdgeType::PartOf),
            "related_to" => Some(KGEdgeType::RelatedTo),
            "similar_to" => Some(KGEdgeType::SimilarTo),
            "causes" => Some(KGEdgeType::Causes),
            "has_fact" => Some(KGEdgeType::HasFact),
            "has_resolution" => Some(KGEdgeType::HasResolution),
            "reminds" => Some(KGEdgeType::Reminds),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_edge_types() {
        let types = vec!["causes".to_string(), "has_resolution".to_string()];
        let parsed = parse_edge_types(&types);
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&KGEdgeType::Causes));
        assert!(parsed.contains(&KGEdgeType::HasResolution));
    }

    #[test]
    fn test_estimate_tokens_for_hit() {
        let hit = HybridHit {
            cid: "test".to_string(),
            content_preview: "This is a test preview with some content".to_string(),
            vector_score: 0.8,
            graph_score: 0.5,
            combined_score: 0.68,
            provenance: vec![ProvenanceStep {
                from_cid: "node1".to_string(),
                edge_type: "causes".to_string(),
                hop: 1,
            }],
        };
        let tokens = estimate_tokens_for_hit(&hit);
        assert!(tokens > 0);
    }
}
