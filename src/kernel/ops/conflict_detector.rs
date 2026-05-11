//! Cognitive Conflict Detector — post-hoc analysis of KG for contradictions.
//!
//! Detects:
//! - Temporal inconsistencies: conflicting facts that are both still valid
//! - Duplicate entities: high embedding similarity without IsAliasOf edge
//!
//! Runs asynchronously via the CognitivePipeline or periodic checks.

use std::sync::Arc;

use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::{KnowledgeGraph, KGEdgeType, KGNodeType};

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

/// Severity of a detected conflict.
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictSeverity {
    Low,
    Medium,
    High,
}

impl std::fmt::Display for ConflictSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConflictSeverity::Low => write!(f, "low"),
            ConflictSeverity::Medium => write!(f, "medium"),
            ConflictSeverity::High => write!(f, "high"),
        }
    }
}

/// A detected conflict.
#[derive(Debug, Clone)]
pub struct DetectedConflict {
    pub conflict_id: String,
    pub conflict_type: String,
    pub description: String,
    pub involved_cids: Vec<String>,
    pub agent_id: String,
    pub severity: ConflictSeverity,
}

pub struct ConflictDetector {
    kg: Arc<dyn KnowledgeGraph>,
    embedder: Option<Arc<dyn EmbeddingProvider>>,
    entity_similarity_threshold: f32,
}

impl ConflictDetector {
    pub fn new(
        kg: Arc<dyn KnowledgeGraph>,
        embedder: Option<Arc<dyn EmbeddingProvider>>,
    ) -> Self {
        Self {
            kg,
            embedder,
            entity_similarity_threshold: 0.90,
        }
    }

    /// Run all conflict detection passes. Returns detected conflicts.
    pub fn detect_all(&self, agent_id: &str) -> Vec<DetectedConflict> {
        let mut conflicts = Vec::new();
        conflicts.extend(self.detect_temporal_conflicts(agent_id));
        conflicts.extend(self.detect_duplicate_entities(agent_id));
        conflicts
    }

    /// Detect and auto-repair high-severity conflicts.
    /// For temporal inconsistencies: invalidates the older edge.
    /// Returns (conflicts, repair_count).
    pub fn detect_and_repair(&self, agent_id: &str) -> (Vec<DetectedConflict>, usize) {
        let conflicts = self.detect_all(agent_id);
        let mut repairs = 0;

        let has_temporal = conflicts.iter().any(|c| c.conflict_type == "temporal_inconsistency");
        if has_temporal {
            // Find and invalidate the older edge in conflicting groups
            if let Ok(edges) = self.kg.list_edges(agent_id) {
                let active: Vec<_> = edges.iter()
                    .filter(|e| e.invalid_at.is_none() && e.expired_at.is_none())
                    .collect();

                // Group by (src, type) and find conflicting groups
                let mut groups: std::collections::HashMap<String, Vec<&crate::fs::graph::KGEdge>> =
                    std::collections::HashMap::new();
                for edge in &active {
                    let key = format!("{}|{:?}", edge.src, edge.edge_type);
                    groups.entry(key).or_default().push(edge);
                }

                for group in groups.values() {
                    if group.len() > 1 {
                        // Keep the newest, invalidate the rest
                        let mut sorted = group.clone();
                        sorted.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                        for older in &sorted[1..] {
                            if self.kg.invalidate_edge(&older.src, &older.dst, older.edge_type.clone()).unwrap_or(false) {
                                repairs += 1;
                            }
                        }
                    }
                }
            }
        }

        (conflicts, repairs)
    }

    /// Detect temporal inconsistencies: edges with same (src, edge_type) but different dst
    /// where both are still valid (no invalid_at).
    fn detect_temporal_conflicts(&self, agent_id: &str) -> Vec<DetectedConflict> {
        let edges = match self.kg.list_edges(agent_id) {
            Ok(e) => e,
            Err(_) => return vec![],
        };

        let mut conflicts = Vec::new();
        let now = crate::util::now_ms();

        // Group active edges by (src, edge_type)
        let mut groups: std::collections::HashMap<String, Vec<&crate::fs::graph::KGEdge>> =
            std::collections::HashMap::new();
        for edge in &edges {
            if edge.invalid_at.is_some() || edge.expired_at.is_some() {
                continue;
            }
            let key = format!("{}|{:?}", edge.src, edge.edge_type);
            groups.entry(key).or_default().push(edge);
        }

        for (key, group) in &groups {
            if group.len() > 1 {
                // Multiple valid edges with same (src, type) but different dst
                let dsts: Vec<&str> = group.iter().map(|e| e.dst.as_str()).collect();
                let conflict_id = format!("tc_{}_{}", key.replace('|', "_"), now);
                conflicts.push(DetectedConflict {
                    conflict_id,
                    conflict_type: "temporal_inconsistency".to_string(),
                    description: format!(
                        "Multiple valid edges from {} type {:?} to: {:?}",
                        group[0].src, group[0].edge_type, dsts
                    ),
                    involved_cids: group.iter().filter_map(|e| e.evidence_cid.clone()).collect(),
                    agent_id: agent_id.to_string(),
                    severity: ConflictSeverity::Medium,
                });
            }
        }

        conflicts
    }

    /// Detect duplicate entities: entities with high embedding similarity but no IsAliasOf edge.
    fn detect_duplicate_entities(&self, agent_id: &str) -> Vec<DetectedConflict> {
        let embedder = match &self.embedder {
            Some(e) => e,
            None => return vec![],
        };

        let nodes = match self.kg.list_nodes(agent_id, Some(KGNodeType::Entity)) {
            Ok(n) => n,
            Err(_) => return vec![],
        };

        // Collect nodes with stored embeddings
        let mut with_emb: Vec<(&crate::fs::graph::KGNode, Vec<f32>)> = Vec::new();
        for node in &nodes {
            if let Some(emb_val) = node.properties.get("embedding").and_then(|v| v.as_array()) {
                let emb: Vec<f32> = emb_val
                    .iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect();
                if !emb.is_empty() {
                    with_emb.push((node, emb));
                }
            }
        }

        let mut conflicts = Vec::new();
        let now = crate::util::now_ms();

        // Check all pairs for high similarity
        for i in 0..with_emb.len() {
            for j in (i + 1)..with_emb.len() {
                let (node_a, emb_a) = &with_emb[i];
                let (node_b, emb_b) = &with_emb[j];

                if emb_a.len() != emb_b.len() {
                    continue;
                }

                let sim = cosine_similarity(emb_a, emb_b);
                if sim >= self.entity_similarity_threshold {
                    // Check if IsAliasOf edge already exists
                    let has_alias = self
                        .kg
                        .edge_history(&node_a.id, &node_b.id, Some(KGEdgeType::IsAliasOf))
                        .map(|h| h.iter().any(|e| e.expired_at.is_none()))
                        .unwrap_or(false);

                    if !has_alias {
                        let conflict_id = format!("de_{}_{}_{}", node_a.id, node_b.id, now);
                        conflicts.push(DetectedConflict {
                            conflict_id,
                            conflict_type: "duplicate_entity".to_string(),
                            description: format!(
                                "Entities '{}' and '{}' have high similarity ({:.2}) but no IsAliasOf edge",
                                node_a.label, node_b.label, sim
                            ),
                            involved_cids: vec![node_a.id.clone(), node_b.id.clone()],
                            agent_id: agent_id.to_string(),
                            severity: ConflictSeverity::Low,
                        });
                    }
                }
            }
        }

        conflicts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::graph::{KGEdge, KGEdgeType, KGNode, KGNodeType, PetgraphBackend};

    fn setup() -> (ConflictDetector, Arc<PetgraphBackend>) {
        let kg: Arc<PetgraphBackend> = Arc::new(PetgraphBackend::open(std::env::temp_dir().join(
            format!("plico_test_conflict_{}", std::process::id()),
        )));
        let detector = ConflictDetector::new(kg.clone(), None);
        (detector, kg)
    }

    fn make_node(id: &str, agent: &str) -> KGNode {
        let mut n = KGNode::new(id.into(), KGNodeType::Entity, agent.into(), "t1".into());
        n.id = id.into();
        n
    }

    #[test]
    fn test_temporal_conflict_detected() {
        let (detector, kg) = setup();

        kg.add_node(make_node("a", "agent1")).unwrap();
        kg.add_node(make_node("b", "agent1")).unwrap();
        kg.add_node(make_node("c", "agent1")).unwrap();

        // Two valid edges from a with same type but different dst
        let e1 = KGEdge::new("a".into(), "b".into(), KGEdgeType::RelatedTo, 0.8);
        kg.add_edge(e1).unwrap();

        let e2 = KGEdge::new("a".into(), "c".into(), KGEdgeType::RelatedTo, 0.7);
        kg.add_edge(e2).unwrap();

        let conflicts = detector.detect_temporal_conflicts("agent1");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].conflict_type, "temporal_inconsistency");
        assert!(conflicts[0].description.contains("b"));
        assert!(conflicts[0].description.contains("c"));
    }

    #[test]
    fn test_no_conflict_when_invalidated() {
        let (detector, kg) = setup();

        kg.add_node(make_node("a", "agent1")).unwrap();
        kg.add_node(make_node("b", "agent1")).unwrap();
        kg.add_node(make_node("c", "agent1")).unwrap();

        // First edge is invalidated
        let mut e1 = KGEdge::new("a".into(), "b".into(), KGEdgeType::RelatedTo, 0.8);
        e1.invalid_at = Some(100);
        kg.add_edge(e1).unwrap();

        // Second edge is valid
        let e2 = KGEdge::new("a".into(), "c".into(), KGEdgeType::RelatedTo, 0.7);
        kg.add_edge(e2).unwrap();

        let conflicts = detector.detect_temporal_conflicts("agent1");
        assert!(conflicts.is_empty());
    }

    #[test]
    fn test_auto_repair_invalidates_older_edge() {
        let (detector, kg) = setup();

        kg.add_node(make_node("a", "agent1")).unwrap();
        kg.add_node(make_node("b", "agent1")).unwrap();
        kg.add_node(make_node("c", "agent1")).unwrap();

        // Two valid edges from a with same type but different dst
        let e1 = KGEdge::new("a".into(), "b".into(), KGEdgeType::RelatedTo, 0.8);
        kg.add_edge(e1).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(10));

        let e2 = KGEdge::new("a".into(), "c".into(), KGEdgeType::RelatedTo, 0.7);
        kg.add_edge(e2).unwrap();

        let (conflicts, repairs) = detector.detect_and_repair("agent1");
        assert!(!conflicts.is_empty());
        assert!(repairs > 0, "Expected at least one repair, got {}", repairs);

        // After repair, only one edge should be valid
        let remaining = detector.detect_temporal_conflicts("agent1");
        assert!(remaining.is_empty(), "Expected no conflicts after repair, got {}", remaining.len());
    }
}
