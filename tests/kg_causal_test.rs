//! KG Causal Reasoning tests (v16.0)
//!
//! Tests cover: causal path analysis, impact analysis,
//! temporal queries, and causal chain detection.

use plico::kernel::AIKernel;
use plico::fs::{KGNodeType, KGEdgeType};
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn test_causal_path_detection() {
    let (kernel, _dir) = make_kernel();

    // Create a simple causal chain: A -> causes -> B -> causes -> C
    let node_a = kernel
        .kg_add_node("Cause A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Effect B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    let node_c = kernel
        .kg_add_node("Effect C", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node C");

    // A causes B
    kernel
        .kg_add_edge(&node_a, &node_b, KGEdgeType::Causes, Some(0.9), "TestAgent", "default")
        .expect("failed to add edge A->B");

    // B causes C
    kernel
        .kg_add_edge(&node_b, &node_c, KGEdgeType::Causes, Some(0.8), "TestAgent", "default")
        .expect("failed to add edge B->C");

    // Find causal paths from A to C
    let paths = kernel.kg_find_causal_path(&node_a, &node_c, 5);

    // Should find at least one path A -> B -> C
    assert!(!paths.is_empty(), "Expected at least one causal path");

    // The first path should have the highest causal strength
    let best_path = &paths[0];
    assert!(best_path.causal_strength > 0.0, "Causal strength should be positive");
    assert!(best_path.nodes.len() >= 2, "Path should have at least 2 nodes");
}

#[test]
fn test_impact_analysis() {
    let (kernel, _dir) = make_kernel();

    // Create a simple propagation chain: A -> B -> C
    let node_a = kernel
        .kg_add_node("Source A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Affected B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    let node_c = kernel
        .kg_add_node("Affected C", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node C");

    // A -> B (strong causal link)
    kernel
        .kg_add_edge(&node_a, &node_b, KGEdgeType::Causes, Some(0.9), "TestAgent", "default")
        .expect("failed to add edge A->B");

    // B -> C (weaker causal link)
    kernel
        .kg_add_edge(&node_b, &node_c, KGEdgeType::RelatedTo, Some(0.5), "TestAgent", "default")
        .expect("failed to add edge B->C");

    // Analyze impact of modifying A with depth 2
    let impact = kernel.kg_impact_analysis(&node_a, 2);

    // Should affect B and potentially C
    assert!(impact.propagation_depth >= 1, "Should propagate at least 1 level");
    assert!(impact.affected_nodes.contains(&node_b), "Should include affected B");
    // C might not be included depending on edge weights and depth
    assert!(impact.severity >= 0.0, "Severity should be non-negative");
    assert!(impact.severity <= 1.0, "Severity should be at most 1.0");
}

#[test]
fn test_temporal_changes() {
    let (kernel, _dir) = make_kernel();

    let before = chrono::Utc::now().timestamp_millis() as u64;

    // Create nodes
    let node_a = kernel
        .kg_add_node("Node A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Node B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    let after = chrono::Utc::now().timestamp_millis() as u64 + 1000; // Add buffer

    // Query temporal changes
    let changes = kernel
        .kg_temporal_changes(before, after, "TestAgent", "default")
        .expect("failed to get temporal changes");

    // Should see at least the two created nodes
    assert!(changes.len() >= 2, "Should have at least 2 changes (two nodes created)");

    // All changes should be "Created" type for newly created nodes
    for change in &changes {
        assert!(change.change_type == plico::kernel::ops::graph::ChangeType::Created
            || change.change_type == plico::kernel::ops::graph::ChangeType::Modified
            || change.change_type == plico::kernel::ops::graph::ChangeType::Deleted,
            "Change type should be valid");
    }

    // Verify timestamps are within range
    for change in &changes {
        assert!(change.timestamp_ms >= before && change.timestamp_ms <= after,
            "Change timestamp should be within query range");
    }

    // Nodes A and B should appear in the changes
    let created_ids: Vec<_> = changes
        .iter()
        .filter(|c| c.change_type == plico::kernel::ops::graph::ChangeType::Created)
        .filter_map(|c| c.after.as_ref().map(|n| n.id.clone()))
        .collect();

    assert!(created_ids.contains(&node_a), "Should contain created node A");
    assert!(created_ids.contains(&node_b), "Should contain created node B");
}

#[test]
fn test_causal_path_no_path() {
    let (kernel, _dir) = make_kernel();

    // Create two disconnected nodes
    let node_a = kernel
        .kg_add_node("Isolated A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Isolated B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    // No edge between them

    // Find causal paths - should be empty
    let paths = kernel.kg_find_causal_path(&node_a, &node_b, 5);
    assert!(paths.is_empty(), "No path should exist between disconnected nodes");
}

#[test]
fn test_impact_analysis_no_outgoing() {
    let (kernel, _dir) = make_kernel();

    // Create a leaf node with no outgoing edges
    let node_a = kernel
        .kg_add_node("Leaf A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    // Analyze impact
    let impact = kernel.kg_impact_analysis(&node_a, 3);

    // Should have no affected nodes (it's a leaf)
    assert!(impact.affected_nodes.is_empty(), "Leaf node should have no affected nodes");
    assert_eq!(impact.severity, 0.0, "Severity should be 0 for leaf node");
}

#[test]
fn test_causal_chain_detection() {
    let (kernel, _dir) = make_kernel();

    // Create a causal chain: A -> causes -> B -> has_fact -> C
    let node_a = kernel
        .kg_add_node("Cause A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Fact B", KGNodeType::Fact, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    let node_c = kernel
        .kg_add_node("Related C", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node C");

    // A causes B
    kernel
        .kg_add_edge(&node_a, &node_b, KGEdgeType::Causes, Some(0.9), "TestAgent", "default")
        .expect("failed to add edge A->B");

    // B has_fact C
    kernel
        .kg_add_edge(&node_b, &node_c, KGEdgeType::HasFact, Some(0.8), "TestAgent", "default")
        .expect("failed to add edge B->C");

    // Detect causal chains starting from A
    let chains = kernel.kg_detect_causal_chains(&node_a, 3);

    // Should find at least one causal chain
    assert!(!chains.is_empty(), "Should find at least one causal chain");

    // Chains should have causal strength
    for chain in &chains {
        assert!(chain.causal_strength > 0.0, "Chain causal strength should be positive");
    }
}

#[test]
fn test_causal_path_with_associates_edge() {
    let (kernel, _dir) = make_kernel();

    // Create nodes with an AssociatesWith edge (lower causal weight)
    let node_a = kernel
        .kg_add_node("Related A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node A");

    let node_b = kernel
        .kg_add_node("Related B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default")
        .expect("failed to add node B");

    // AssociatesWith edge (lower causal weight)
    kernel
        .kg_add_edge(&node_a, &node_b, KGEdgeType::AssociatesWith, Some(0.5), "TestAgent", "default")
        .expect("failed to add edge A->B");

    // Find causal paths
    let paths = kernel.kg_find_causal_path(&node_a, &node_b, 5);

    // Should find the path
    assert!(!paths.is_empty(), "Should find path via AssociatesWith edge");
}

#[test]
fn test_impact_analysis_different_depths() {
    let (kernel, _dir) = make_kernel();

    // Create a chain: A -> B -> C -> D
    let node_a = kernel.kg_add_node("A", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default").expect("add A");
    let node_b = kernel.kg_add_node("B", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default").expect("add B");
    let node_c = kernel.kg_add_node("C", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default").expect("add C");
    let node_d = kernel.kg_add_node("D", KGNodeType::Entity, serde_json::json!({}), "TestAgent", "default").expect("add D");

    kernel.kg_add_edge(&node_a, &node_b, KGEdgeType::Causes, Some(1.0), "TestAgent", "default").expect("add A->B");
    kernel.kg_add_edge(&node_b, &node_c, KGEdgeType::Causes, Some(1.0), "TestAgent", "default").expect("add B->C");
    kernel.kg_add_edge(&node_c, &node_d, KGEdgeType::Causes, Some(1.0), "TestAgent", "default").expect("add C->D");

    // Test with depth 1
    let impact_1 = kernel.kg_impact_analysis(&node_a, 1);
    assert!(impact_1.affected_nodes.contains(&node_b), "Depth 1 should affect B");

    // Test with depth 2
    let impact_2 = kernel.kg_impact_analysis(&node_a, 2);
    assert!(impact_2.affected_nodes.contains(&node_b), "Depth 2 should affect B");
    assert!(impact_2.affected_nodes.contains(&node_c), "Depth 2 should affect C");

    // Test with depth 3
    let impact_3 = kernel.kg_impact_analysis(&node_a, 3);
    assert!(impact_3.affected_nodes.contains(&node_b), "Depth 3 should affect B");
    assert!(impact_3.affected_nodes.contains(&node_c), "Depth 3 should affect C");
    assert!(impact_3.affected_nodes.contains(&node_d), "Depth 3 should affect D");

    // Higher depth should not decrease affected nodes
    assert!(impact_3.affected_nodes.len() >= impact_2.affected_nodes.len());
}
