//! Graph backend tests.

#[allow(unused_imports)]
use crate::fs::graph::{KGNode, KGEdge, KGNodeType, KGEdgeType, PetgraphBackend, KnowledgeGraph};

#[allow(dead_code)]
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn make_node(id: &str, node_type: KGNodeType, tags: Vec<String>, agent: &str) -> KGNode {
    KGNode {
        id: id.to_string(),
        label: id.to_string(),
        node_type,
        content_cid: None,
        properties: serde_json::json!({ "tags": tags }),
        agent_id: agent.to_string(),
        tenant_id: "default".to_string(),
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

// ── Temporal query tests ────────────────────────────────────────────────────────

#[test]
fn test_get_valid_edges_at_filters_by_time() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("a", KGNodeType::Entity, vec![], "agent1")).unwrap();
    kg.add_node(make_node("b", KGNodeType::Entity, vec![], "agent1")).unwrap();

    let mut edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
    edge.valid_at = Some(1000);
    edge.invalid_at = Some(2000);
    kg.add_edge(edge).unwrap();

    assert!(kg.get_valid_edges_at(500).unwrap().is_empty(), "before valid_at");
    assert_eq!(kg.get_valid_edges_at(1000).unwrap().len(), 1, "at valid_at");
    assert_eq!(kg.get_valid_edges_at(1500).unwrap().len(), 1, "between valid_at and invalid_at");
    assert!(kg.get_valid_edges_at(2000).unwrap().is_empty(), "at invalid_at boundary");
    assert!(kg.get_valid_edges_at(3000).unwrap().is_empty(), "after invalid_at");
}

#[test]
fn test_get_valid_edges_at_current_time() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("x", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("y", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("x", "y", KGEdgeType::RelatedTo, 1.0)).unwrap();

    let now = now_ms();
    let valid = kg.get_valid_edges_at(now).unwrap();
    assert_eq!(valid.len(), 1);
}

#[test]
fn test_get_valid_edges_at_no_edges() {
    let kg = PetgraphBackend::new();
    assert!(kg.get_valid_edges_at(now_ms()).unwrap().is_empty());
}

#[test]
fn test_get_valid_edge_between_returns_most_recent() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("p", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("w1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("w2", KGNodeType::Entity, vec![], "a")).unwrap();

    // Two different fact edges: p→w1 (valid_at=1000) and p→w2 (valid_at=2000)
    let e1 = {
        let mut e = make_edge("p", "w1", KGEdgeType::HasFact, 0.8);
        e.valid_at = Some(1000);
        e
    };
    kg.add_edge(e1).unwrap();

    let e2 = {
        let mut e = make_edge("p", "w2", KGEdgeType::HasFact, 0.9);
        e.valid_at = Some(2000);
        e
    };
    kg.add_edge(e2).unwrap();

    // At time 1500, only e1 is valid
    let found = kg.get_valid_edge_between("p", "w1", None, 1500).unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().valid_at, Some(1000));

    // At time 2500, e2 is valid
    let found = kg.get_valid_edge_between("p", "w2", None, 2500).unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().valid_at, Some(2000));
}

#[test]
fn test_get_valid_nodes_at_filters_by_time() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("entity1", KGNodeType::Entity, vec![], "agent1")).unwrap();
    kg.add_node(make_node("entity2", KGNodeType::Entity, vec![], "agent1")).unwrap();

    // entity1 becomes valid at t=1000, invalid at t=2000
    let mut node1 = make_node("entity1", KGNodeType::Entity, vec![], "agent1");
    node1.valid_at = Some(1000);
    node1.invalid_at = Some(2000);
    kg.add_node(node1).unwrap();

    // entity2 is always valid (default)
    let node2 = make_node("entity2", KGNodeType::Entity, vec![], "agent1");
    kg.add_node(node2).unwrap();

    let at_500 = kg.get_valid_nodes_at("agent1", None, 500).unwrap();
    assert_eq!(at_500.len(), 1, "entity2 always valid");
    assert_eq!(at_500[0].id, "entity2");

    let at_1000 = kg.get_valid_nodes_at("agent1", None, 1000).unwrap();
    assert_eq!(at_1000.len(), 2, "both valid at valid_at");

    // At exact invalid_at boundary, entity1 is NOT valid
    let at_2000 = kg.get_valid_nodes_at("agent1", None, 2000).unwrap();
    assert_eq!(at_2000.len(), 1, "entity1 invalid at exact invalid_at boundary");

    let at_2500 = kg.get_valid_nodes_at("agent1", None, 2500).unwrap();
    assert_eq!(at_2500.len(), 1, "only entity2 valid after 2000");
}

#[test]
fn test_get_valid_nodes_at_filters_by_agent_and_type() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("e1", KGNodeType::Entity, vec![], "agent1")).unwrap();
    kg.add_node(make_node("e2", KGNodeType::Entity, vec![], "agent2")).unwrap();
    kg.add_node(make_node("d1", KGNodeType::Document, vec![], "agent1")).unwrap();

    let now = now_ms();
    let all_agent1 = kg.get_valid_nodes_at("agent1", None, now).unwrap();
    assert_eq!(all_agent1.len(), 2);

    let entities_only = kg.get_valid_nodes_at("agent1", Some(KGNodeType::Entity), now).unwrap();
    assert_eq!(entities_only.len(), 1);
    assert_eq!(entities_only[0].id, "e1");

    let other_agent = kg.get_valid_nodes_at("agent2", None, now).unwrap();
    assert_eq!(other_agent.len(), 1);
    assert_eq!(other_agent[0].id, "e2");
}

#[test]
fn test_remove_edge() {
    let kg = PetgraphBackend::new();
    let n1 = make_node("a", KGNodeType::Entity, vec![], "agent1");
    let n2 = make_node("b", KGNodeType::Entity, vec![], "agent1");
    kg.add_node(n1).unwrap();
    kg.add_node(n2).unwrap();

    let edge = make_edge("a", "b", KGEdgeType::RelatedTo, 1.0);
    kg.add_edge(edge).unwrap();

    assert_eq!(kg.list_edges("agent1").unwrap().len(), 1);
    kg.remove_edge("a", "b", Some(KGEdgeType::RelatedTo)).unwrap();
    assert_eq!(kg.list_edges("agent1").unwrap().len(), 0);
}

#[test]
fn test_remove_edge_no_match_returns_error() {
    let kg = PetgraphBackend::new();
    let result = kg.remove_edge("nonexistent", "also-no", None);
    assert!(result.is_err());
}

#[test]
fn test_update_node_merges_properties() {
    let kg = PetgraphBackend::new();
    let node = KGNode {
        id: "n1".into(),
        label: "Old".into(),
        node_type: KGNodeType::Entity,
        content_cid: None,
        properties: serde_json::json!({"key1": "val1", "key2": "val2"}),
        agent_id: "agent1".into(),
        created_at: now_ms(),
        valid_at: Some(now_ms()),
        invalid_at: None,
        expired_at: None,
        tenant_id: "default".to_string(),
    };
    kg.add_node(node).unwrap();

    kg.update_node("n1", Some("New"), Some(serde_json::json!({"key2": "updated", "key3": "new"}))).unwrap();

    let updated = kg.get_node("n1").unwrap().unwrap();
    assert_eq!(updated.label, "New");
    assert_eq!(updated.properties["key1"], "val1");
    assert_eq!(updated.properties["key2"], "updated");
    assert_eq!(updated.properties["key3"], "new");
}

#[test]
fn test_update_node_label_only() {
    let kg = PetgraphBackend::new();
    let node = make_node("n1", KGNodeType::Fact, vec![], "agent1");
    kg.add_node(node).unwrap();

    kg.update_node("n1", Some("RenamedFact"), None).unwrap();
    let updated = kg.get_node("n1").unwrap().unwrap();
    assert_eq!(updated.label, "RenamedFact");
}

// ── Persistence roundtrip tests ─────────────────────────────────────────────────

#[test]
fn test_save_load_roundtrip() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("a", KGNodeType::Entity, vec!["tag1".into()], "agent1")).unwrap();
    kg.add_node(make_node("b", KGNodeType::Document, vec![], "agent1")).unwrap();
    kg.add_edge(make_edge("a", "b", KGEdgeType::RelatedTo, 0.85)).unwrap();

    let dir = tempfile::tempdir().unwrap();
    kg.save_to_disk(dir.path()).unwrap();

    let kg2 = PetgraphBackend::new();
    kg2.load_from_disk(dir.path()).unwrap();

    assert_eq!(kg2.node_count().unwrap(), 2);
    assert_eq!(kg2.edge_count().unwrap(), 1);
    let node_a = kg2.get_node("a").unwrap().unwrap();
    assert_eq!(node_a.label, "a");
    assert_eq!(node_a.node_type, KGNodeType::Entity);
    let edges = kg2.list_edges("agent1").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].weight, 0.85);
}

#[test]
fn test_load_from_disk_replaces_state() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("old", KGNodeType::Entity, vec![], "a")).unwrap();
    assert_eq!(kg.node_count().unwrap(), 1);

    let kg2 = PetgraphBackend::new();
    kg2.add_node(make_node("new1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg2.add_node(make_node("new2", KGNodeType::Fact, vec![], "a")).unwrap();

    let dir = tempfile::tempdir().unwrap();
    kg2.save_to_disk(dir.path()).unwrap();

    kg.load_from_disk(dir.path()).unwrap();
    assert_eq!(kg.node_count().unwrap(), 2);
    assert!(kg.get_node("old").unwrap().is_none());
    assert!(kg.get_node("new1").unwrap().is_some());
    assert!(kg.get_node("new2").unwrap().is_some());
}

// ── v0.9 Temporal Invalidation Tests ─────────────────────────────────────────

#[test]
fn test_invalidate_conflicts_supersedes() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "test")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "test")).unwrap();

    kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.5)).unwrap();
    kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.9)).unwrap();

    let history = kg.edge_history("n1", "n2", Some(KGEdgeType::RelatedTo)).unwrap();
    assert_eq!(history.len(), 2, "should have 2 edges in history");

    let invalidated = history.iter().filter(|e| e.invalid_at.is_some()).count();
    assert_eq!(invalidated, 1, "first edge should be invalidated");

    let active = history.iter().filter(|e| e.invalid_at.is_none()).count();
    assert_eq!(active, 1, "second edge should be active");
}

#[test]
fn test_different_type_edges_coexist() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "test")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "test")).unwrap();

    kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.5)).unwrap();
    kg.add_edge(make_edge("n1", "n2", KGEdgeType::HasFact, 0.8)).unwrap();

    let history = kg.edge_history("n1", "n2", None).unwrap();
    assert_eq!(history.len(), 2, "both edges should exist");

    let active = history.iter().filter(|e| e.invalid_at.is_none()).count();
    assert_eq!(active, 2, "both should be active (different types)");
}

#[test]
fn test_edge_history_returns_all() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "test")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "test")).unwrap();

    for i in 0..3 {
        kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, i as f32 * 0.3)).unwrap();
    }

    let history = kg.edge_history("n1", "n2", Some(KGEdgeType::RelatedTo)).unwrap();
    assert_eq!(history.len(), 3, "all 3 edges should be in history");

    let active = history.iter().filter(|e| e.invalid_at.is_none()).count();
    assert_eq!(active, 1, "only the last edge should be active");
}

#[test]
fn test_add_edge_auto_invalidates() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "test")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "test")).unwrap();

    kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.5)).unwrap();

    let now = now_ms();
    let valid_before = kg.get_valid_edges_at(now).unwrap();
    let matching = valid_before.iter().filter(|e| e.src == "n1" && e.dst == "n2").count();
    assert_eq!(matching, 1);

    kg.add_edge(make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.9)).unwrap();

    let valid_after = kg.get_valid_edges_at(now + 1).unwrap();
    let matching_after = valid_after.iter().filter(|e| e.src == "n1" && e.dst == "n2").count();
    assert_eq!(matching_after, 1, "should still have only 1 active edge after supersession");
}

#[test]
fn test_invalidate_preserves_data() {
    let kg = PetgraphBackend::new();
    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "test")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "test")).unwrap();

    let original = make_edge("n1", "n2", KGEdgeType::HasFact, 0.7);
    let original_weight = original.weight;
    kg.add_edge(original).unwrap();
    kg.add_edge(make_edge("n1", "n2", KGEdgeType::HasFact, 0.95)).unwrap();

    let history = kg.edge_history("n1", "n2", Some(KGEdgeType::HasFact)).unwrap();
    let old_edge = history.iter().find(|e| e.invalid_at.is_some()).unwrap();
    assert_eq!(old_edge.weight, original_weight, "old edge data should be preserved");
    assert_eq!(old_edge.edge_type, KGEdgeType::HasFact);
    assert!(old_edge.invalid_at.is_some(), "old edge should have invalid_at set");
}

// ── v0.9 Jaccard Auto-Correlation Tests ─────────────────────────────────────

#[test]
fn test_jaccard_weight_scoring() {
    let kg = PetgraphBackend::new();
    // doc_a has 5 tags, doc_b shares 2 of them → Jaccard = 2/5 = 0.4
    kg.upsert_document("doc_a", &["t1","t2","t3","t4","t5"].map(String::from), "a").unwrap();
    kg.upsert_document("doc_b", &["t1","t2","t6","t7","t8"].map(String::from), "a").unwrap();

    let edges = kg.list_edges("a").unwrap();
    let assoc: Vec<_> = edges.iter()
        .filter(|e| e.edge_type == KGEdgeType::AssociatesWith && e.src == "doc_b" && e.dst == "doc_a")
        .collect();
    assert_eq!(assoc.len(), 1, "should have association edge");
    let w = assoc[0].weight;
    assert!((w - 0.4).abs() < 0.01, "Jaccard should be 2/5=0.4, got {}", w);
}

#[test]
fn test_single_tag_overlap_creates_edge() {
    let kg = PetgraphBackend::new();
    kg.upsert_document("d1", &["shared".into(), "unique1".into()], "a").unwrap();
    kg.upsert_document("d2", &["shared".into(), "unique2".into()], "a").unwrap();

    let edges = kg.list_edges("a").unwrap();
    let assoc: Vec<_> = edges.iter()
        .filter(|e| e.edge_type == KGEdgeType::AssociatesWith)
        .collect();
    assert!(assoc.len() >= 2, "1 shared tag should create edges (threshold=1)");
}

#[test]
fn test_no_overlap_no_edge() {
    let kg = PetgraphBackend::new();
    kg.upsert_document("d1", &["a1".into(), "a2".into()], "a").unwrap();
    kg.upsert_document("d2", &["b1".into(), "b2".into()], "a").unwrap();

    let edges = kg.list_edges("a").unwrap();
    let assoc: Vec<_> = edges.iter()
        .filter(|e| e.edge_type == KGEdgeType::AssociatesWith)
        .collect();
    assert!(assoc.is_empty(), "0 shared tags should create no edges");
}

#[test]
fn test_weight_not_capped_at_one() {
    let kg = PetgraphBackend::new();
    // 3 shared out of 4 total → Jaccard = 3/4 = 0.75 (not 1.0)
    kg.upsert_document("d1", &["t1","t2","t3","t4"].map(String::from), "a").unwrap();
    kg.upsert_document("d2", &["t1","t2","t3","t5"].map(String::from), "a").unwrap();

    let edges = kg.list_edges("a").unwrap();
    let assoc: Vec<_> = edges.iter()
        .filter(|e| e.edge_type == KGEdgeType::AssociatesWith && e.src == "d2" && e.dst == "d1")
        .collect();
    assert_eq!(assoc.len(), 1);
    let w = assoc[0].weight;
    assert!(w < 1.0, "weight should be proportional, not capped at 1.0, got {}", w);
    assert!((w - 0.75).abs() < 0.01, "Jaccard should be 3/4=0.75, got {}", w);
}

// ── redb 4.0 Storage Tests ─────────────────────────────────────────────────────

#[test]
fn test_redb_node_crud() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    let node = make_node("test_node", KGNodeType::Entity, vec!["tag1".into()], "agent1");
    kg.add_node(node).unwrap();

    let retrieved = kg.get_node("test_node").unwrap().unwrap();
    assert_eq!(retrieved.id, "test_node");
    assert_eq!(retrieved.label, "test_node");

    // Reopen and verify persistence
    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    let retrieved2 = kg2.get_node("test_node").unwrap().unwrap();
    assert_eq!(retrieved2.id, "test_node");
}

#[test]
fn test_redb_edge_crud() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("n1", KGNodeType::Entity, vec![], "agent1")).unwrap();
    kg.add_node(make_node("n2", KGNodeType::Entity, vec![], "agent1")).unwrap();

    let edge = make_edge("n1", "n2", KGEdgeType::RelatedTo, 0.8);
    kg.add_edge(edge).unwrap();

    let edges = kg.list_edges("agent1").unwrap();
    assert_eq!(edges.len(), 1);

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    let edges2 = kg2.list_edges("agent1").unwrap();
    assert_eq!(edges2.len(), 1);
    assert_eq!(edges2[0].weight, 0.8);
}

#[test]
fn test_redb_persist_incremental() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    // Add first node/edge
    kg.add_node(make_node("node1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("node2", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("node1", "node2", KGEdgeType::RelatedTo, 0.5)).unwrap();

    // Verify count immediately
    assert_eq!(kg.node_count().unwrap(), 2);
    assert_eq!(kg.edge_count().unwrap(), 1);

    // Add another node/edge incrementally
    kg.add_node(make_node("node3", KGNodeType::Document, vec![], "a")).unwrap();
    kg.add_edge(make_edge("node2", "node3", KGEdgeType::AssociatesWith, 0.7)).unwrap();

    assert_eq!(kg.node_count().unwrap(), 3);
    assert_eq!(kg.edge_count().unwrap(), 2);

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg2.node_count().unwrap(), 3);
    assert_eq!(kg2.edge_count().unwrap(), 2);
}

#[test]
fn test_redb_load_all() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    // Create multiple nodes and edges
    for i in 0..5 {
        kg.add_node(make_node(&format!("n{}", i), KGNodeType::Entity, vec![], "a")).unwrap();
    }
    for i in 0..4 {
        kg.add_edge(make_edge(&format!("n{}", i), &format!("n{}", i+1), KGEdgeType::RelatedTo, 0.5)).unwrap();
    }

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());

    assert_eq!(kg2.node_count().unwrap(), 5);
    assert_eq!(kg2.edge_count().unwrap(), 4);

    // Verify specific paths
    let path = kg2.find_weighted_path("n0", "n4", 10).unwrap();
    assert!(path.is_some());
    assert_eq!(path.unwrap().len(), 5);
}

#[test]
fn test_redb_migration_from_json() {
    let dir = tempfile::TempDir::new().unwrap();

    // Create a KG with JSON persist, then reopen with redb
    {
        let kg = PetgraphBackend::new();
        kg.add_node(make_node("mig_node1", KGNodeType::Entity, vec![], "agent1")).unwrap();
        kg.add_node(make_node("mig_node2", KGNodeType::Document, vec![], "agent1")).unwrap();
        kg.add_edge(make_edge("mig_node1", "mig_node2", KGEdgeType::HasFact, 0.9)).unwrap();

        // Save as JSON (what old code would do)
        kg.save_to_disk(dir.path()).unwrap();
    }

    // Now open with redb - should detect JSON and migrate
    let kg = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg.node_count().unwrap(), 2);
    assert_eq!(kg.edge_count().unwrap(), 1);

    let node = kg.get_node("mig_node1").unwrap().unwrap();
    assert_eq!(node.node_type, KGNodeType::Entity);
}

#[test]
fn test_redb_edge_key_format() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("src_node", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("dst_node", KGNodeType::Entity, vec![], "a")).unwrap();

    let edge = make_edge("src_node", "dst_node", KGEdgeType::AssociatesWith, 0.6);
    kg.add_edge(edge).unwrap();

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());

    // Verify we can retrieve the edge
    let edges = kg2.list_edges("a").unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].edge_type, KGEdgeType::AssociatesWith);
}

#[test]
fn test_redb_remove_node() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("remove_node", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("other", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("remove_node", "other", KGEdgeType::RelatedTo, 0.5)).unwrap();

    assert_eq!(kg.node_count().unwrap(), 2);
    kg.remove_node("remove_node").unwrap();
    assert_eq!(kg.node_count().unwrap(), 1);

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg2.node_count().unwrap(), 1);
    assert!(kg2.get_node("remove_node").unwrap().is_none());
}

#[test]
fn test_redb_remove_edge() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("e1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("e2", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(make_edge("e1", "e2", KGEdgeType::RelatedTo, 0.8)).unwrap();

    assert_eq!(kg.edge_count().unwrap(), 1);
    kg.remove_edge("e1", "e2", Some(KGEdgeType::RelatedTo)).unwrap();
    assert_eq!(kg.edge_count().unwrap(), 0);

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg2.edge_count().unwrap(), 0);
}

// ── Bug-fix regression tests (v2 edge key + atomic persistence) ─────────────

#[test]
fn test_redb_edge_history_survives_restart() {
    // Regression: old 3-part key "src|dst|type" caused later edges to overwrite earlier ones.
    // With 4-part key "src|dst|type|created_at", each temporal version is distinct.
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("h1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("h2", KGNodeType::Entity, vec![], "a")).unwrap();

    // Use real timestamps via KGEdge::new to ensure unique created_at
    let e1 = KGEdge::new("h1".into(), "h2".into(), KGEdgeType::HasFact, 0.5);
    kg.add_edge(e1).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let e2 = KGEdge::new("h1".into(), "h2".into(), KGEdgeType::HasFact, 0.9);
    kg.add_edge(e2).unwrap();

    let history = kg.edge_history("h1", "h2", Some(KGEdgeType::HasFact)).unwrap();
    assert_eq!(history.len(), 2, "in-memory should have 2 edge versions");

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    let history2 = kg2.edge_history("h1", "h2", Some(KGEdgeType::HasFact)).unwrap();
    assert_eq!(history2.len(), 2, "redb should preserve both edge versions after restart");

    let active = history2.iter().filter(|e| e.invalid_at.is_none()).count();
    assert_eq!(active, 1, "only latest edge should be active");
}

#[test]
fn test_redb_invalidate_conflicts_persisted() {
    // Regression: invalidate_conflicts only set invalid_at in memory, not in redb.
    // After restart, invalidated edges appeared valid again.
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("ic1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("ic2", KGNodeType::Entity, vec![], "a")).unwrap();

    let e1 = KGEdge::new("ic1".into(), "ic2".into(), KGEdgeType::RelatedTo, 0.5);
    kg.add_edge(e1).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let e2 = KGEdge::new("ic1".into(), "ic2".into(), KGEdgeType::RelatedTo, 0.9);
    kg.add_edge(e2).unwrap();

    // Before restart: 1 active edge at current time
    let now_val = now_ms();
    let valid = kg.get_valid_edges_at(now_val + 1).unwrap();
    let matching = valid.iter().filter(|e| e.src == "ic1" && e.dst == "ic2").count();
    assert_eq!(matching, 1, "should have exactly 1 active edge before restart");

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    let valid2 = kg2.get_valid_edges_at(now_val + 2).unwrap();
    let matching2 = valid2.iter().filter(|e| e.src == "ic1" && e.dst == "ic2").count();
    assert_eq!(matching2, 1, "after restart, still exactly 1 active edge (invalidation persisted)");
}

#[test]
fn test_redb_remove_node_cleans_edges() {
    // Regression: remove_node removed edges from memory but not from redb.
    // After restart, orphaned edges reappeared.
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("rn1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("rn2", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("rn3", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_edge(KGEdge::new("rn1".into(), "rn2".into(), KGEdgeType::RelatedTo, 0.5)).unwrap();
    kg.add_edge(KGEdge::new("rn2".into(), "rn3".into(), KGEdgeType::Follows, 0.8)).unwrap();
    kg.add_edge(KGEdge::new("rn3".into(), "rn1".into(), KGEdgeType::Causes, 0.3)).unwrap();

    assert_eq!(kg.edge_count().unwrap(), 3);
    kg.remove_node("rn2").unwrap();
    assert_eq!(kg.node_count().unwrap(), 2);
    assert_eq!(kg.edge_count().unwrap(), 1, "only rn3→rn1 edge should remain");

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg2.node_count().unwrap(), 2, "node count preserved after restart");
    assert_eq!(kg2.edge_count().unwrap(), 1, "orphaned edges should not reappear after restart");
    assert!(kg2.get_node("rn2").unwrap().is_none(), "removed node stays removed");
}

#[test]
fn test_redb_remove_edge_all_history() {
    // remove_edge should remove all versions (including invalidated) from redb
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("reh1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.add_node(make_node("reh2", KGNodeType::Entity, vec![], "a")).unwrap();

    let e1 = KGEdge::new("reh1".into(), "reh2".into(), KGEdgeType::HasFact, 0.3);
    kg.add_edge(e1).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(2));
    let e2 = KGEdge::new("reh1".into(), "reh2".into(), KGEdgeType::HasFact, 0.9);
    kg.add_edge(e2).unwrap();

    assert_eq!(kg.edge_count().unwrap(), 2, "both versions stored");
    kg.remove_edge("reh1", "reh2", Some(KGEdgeType::HasFact)).unwrap();
    assert_eq!(kg.edge_count().unwrap(), 0, "all versions removed");

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    assert_eq!(kg2.edge_count().unwrap(), 0, "removal persisted across restart");
}

#[test]
fn test_redb_update_node_persisted() {
    let dir = tempfile::TempDir::new().unwrap();
    let kg = PetgraphBackend::open(dir.path().to_path_buf());

    kg.add_node(make_node("up1", KGNodeType::Entity, vec![], "a")).unwrap();
    kg.update_node("up1", Some("UpdatedLabel"), Some(serde_json::json!({"key": "value"}))).unwrap();

    drop(kg);
    let kg2 = PetgraphBackend::open(dir.path().to_path_buf());
    let node = kg2.get_node("up1").unwrap().unwrap();
    assert_eq!(node.label, "UpdatedLabel");
    assert_eq!(node.properties["key"], "value");
}
