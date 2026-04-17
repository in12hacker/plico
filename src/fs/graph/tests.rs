//! Graph backend tests.

#[allow(unused_imports)]
use crate::fs::graph::{KGNode, KGEdge, KGNodeType, KGEdgeType, PetgraphBackend, KnowledgeGraph};

#[allow(dead_code)]
fn make_node(id: &str, node_type: KGNodeType, tags: Vec<String>, agent: &str) -> KGNode {
    KGNode {
        id: id.to_string(),
        label: id.to_string(),
        node_type,
        content_cid: None,
        properties: serde_json::json!({ "tags": tags }),
        agent_id: agent.to_string(),
        created_at: 0,
        valid_at: None,
        invalid_at: None,
        expired_at: None,
    }
}

#[allow(dead_code)]
fn make_edge(src: &str, dst: &str, edge_type: KGEdgeType, weight: f32) -> KGEdge {
    KGEdge {
        src: src.to_string(),
        dst: dst.to_string(),
        edge_type,
        weight,
        evidence_cid: None,
        created_at: 0,
        valid_at: None,
        invalid_at: None,
        expired_at: None,
        episode: None,
    }
}

#[test]
fn test_find_weighted_path_basic() {
    // Simple path: x -> y -> z with equal weights
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();
    kg.add_edge(make_edge("y", "z", KGEdgeType::RelatedTo, 1.0)).unwrap();

    let path = kg.find_weighted_path("x", "z", 5).unwrap();
    assert!(path.is_some());
    let nodes = path.unwrap();
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0].id, "x");
    assert_eq!(nodes[2].id, "z");
}

#[test]
fn test_find_weighted_path_prefers_high_weight() {
    // Two paths to z:
    //   x -> a → z (weight 0.9 + 0.9 = 1.8)
    //   x → b → z (weight 1.0 + 0.5 = 1.5)
    // Should prefer the first (higher total weight 1.8)
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("a", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("b", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
    // Path 1: x -> a -> z (high weight 0.9 each, total 1.8)
    kg.add_edge(make_edge("x", "a", KGEdgeType::RelatedTo, 0.9)).unwrap();
    kg.add_edge(make_edge("a", "z", KGEdgeType::RelatedTo, 0.9)).unwrap();
    // Path 2: x -> b -> z (1.0 + 0.5 = 1.5, starts with higher but ends lower)
    kg.add_edge(make_edge("x", "b", KGEdgeType::RelatedTo, 1.0)).unwrap();
    kg.add_edge(make_edge("b", "z", KGEdgeType::RelatedTo, 0.5)).unwrap();

    let path = kg.find_weighted_path("x", "z", 5).unwrap();
    assert!(path.is_some());
    let nodes = path.unwrap();
    // Should take the x->a->z path (1.8 > 1.5)
    assert_eq!(nodes[1].id, "a");
}

#[test]
fn test_find_weighted_path_no_path() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();

    // No edge between x and y
    let path = kg.find_weighted_path("x", "y", 5).unwrap();
    assert!(path.is_none());
}

#[test]
fn test_find_weighted_path_respects_max_depth() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();
    kg.add_edge(make_edge("y", "z", KGEdgeType::RelatedTo, 1.0)).unwrap();

    // max_depth = 1 should not find z (needs 2 hops)
    let path = kg.find_weighted_path("x", "z", 1).unwrap();
    assert!(path.is_none());
}

#[test]
fn test_find_weighted_path_acyclic() {
    // Triangle: x->y, y->z, z->x (cycle)
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("z", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();
    kg.add_edge(make_edge("y", "z", KGEdgeType::RelatedTo, 1.0)).unwrap();
    kg.add_edge(make_edge("z", "x", KGEdgeType::RelatedTo, 1.0)).unwrap();

    // Should find x->y->z (2 hops) without infinite loop
    let path = kg.find_weighted_path("x", "z", 5).unwrap();
    assert!(path.is_some());
    assert_eq!(path.unwrap().len(), 3);
}
