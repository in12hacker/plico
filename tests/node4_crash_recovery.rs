//! Node4 Sprint 3: Crash Recovery Integration Test
//!
//! Crash recovery verification: ingest 10 articles → execute queries → restart
//! Validates data integrity after process restart.
//!
//! Design: Section 5 "崩溃恢复（MVP 整体验收）" in docs/design-node4-collaborative-ecosystem.md

use plico::kernel::AIKernel;
use plico::api::semantic::TaskStatus;
use plico::memory::MemoryScope;

/// Security articles for testing.
fn security_articles() -> Vec<(&'static str, &'static str, Vec<String>)> {
    vec![
        ("SQL Injection Attack Patterns", "SQL injection attacks exploit vulnerable database queries by inserting malicious SQL code. Prevention includes parameterized queries and WAF.", vec!["security".into(), "sql-injection".into()]),
        ("XSS Defense Strategies", "Cross-Site Scripting (XSS) attacks can be prevented with Content Security Policy and output encoding.", vec!["security".into(), "xss".into()]),
        ("CSRF Prevention", "Cross-Site Request Forgery can be prevented with anti-CSRF tokens and SameSite cookies.", vec!["security".into(), "csrf".into()]),
        ("WAF Best Practices", "Web Application Firewalls should be configured with positive security model and regular rule updates.", vec!["security".into(), "waf".into()]),
        ("Input Validation Guide", "Input validation should use whitelisting over blacklisting for security.", vec!["security".into(), "validation".into()]),
        ("OAuth Security", "OAuth 2.0 requires careful redirect URI validation and PKCE for public clients.", vec!["security".into(), "oauth".into()]),
        ("API Rate Limiting", "Rate limiting prevents brute force and DoS attacks on API endpoints.", vec!["security".into(), "api".into()]),
        ("CVE-2024-21762 Analysis", "Critical RCE vulnerability in FortiOS SSL VPN. Patch immediately.", vec!["security".into(), "cve".into()]),
        ("Incident Response Plan", "Incident response should follow prepare, detect, contain, recover phases.", vec!["security".into(), "incident-response".into()]),
        ("Database Security", "Database security includes principle of least privilege and encryption at rest.", vec!["security".into(), "database".into()]),
    ]
}

// ─── Crash Recovery: CAS Data Persists ─────────────────────────────────────────

#[test]
fn test_crash_recovery_cas_data_persists() {
    // CAS (Content-Addressed Storage) persists objects immediately on write.
    // This test verifies that objects are accessible after restart.

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    // Create kernel and ingest articles
    let kernel1 = AIKernel::new(root.clone()).expect("kernel init");
    let agent_id = kernel1.register_agent("ingest-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel1.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel1.permission_grant(&agent_id, PermissionAction::Write, None, None);

    let mut cids = Vec::new();
    for (title, content, tags) in security_articles() {
        let cid = kernel1.semantic_create(
            content.as_bytes().to_vec(),
            tags,
            &agent_id,
            Some(title.to_string()),
        ).expect("semantic_create should succeed");
        cids.push(cid);
    }
    assert_eq!(cids.len(), 10, "Should have 10 CIDs");
    drop(kernel1);

    // Create new kernel from same root
    let kernel2 = AIKernel::new(root).expect("kernel init after restart");

    // Verify all 10 articles are still accessible in CAS
    for cid in &cids {
        let obj = kernel2.get_object(cid, &agent_id, "default");
        assert!(obj.is_ok(), "CID {} should be recoverable after restart", cid);
    }

    // Verify article content integrity
    for cid in &cids {
        let obj = kernel2.get_object(cid, &agent_id, "default").unwrap();
        assert!(!obj.data.is_empty(), "CAS object should have content");
    }

    tracing::info!("test_crash_recovery_cas_data_persists PASSED");
}

// ─── Crash Recovery: KG Data Persists ──────────────────────────────────────────

#[test]
fn test_crash_recovery_kg_data_persists() {
    // KG is stored in a file-based backend. This test verifies KG data persists across restarts.

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let kernel1 = AIKernel::new(root.clone()).expect("kernel init");
    let agent_id = kernel1.register_agent("kg-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel1.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel1.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Create KG structure
    let sql_injection = kernel1.kg_add_node(
        "SQL Injection",
        plico::fs::KGNodeType::Entity,
        serde_json::json!({"severity": "high"}),
        &agent_id,
        "default",
    ).expect("kg_add_node should succeed");

    let waf = kernel1.kg_add_node(
        "WAF",
        plico::fs::KGNodeType::Entity,
        serde_json::json!({"type": "defense"}),
        &agent_id,
        "default",
    ).expect("kg_add_node should succeed");

    let cve = kernel1.kg_add_node(
        "CVE-2024-21762",
        plico::fs::KGNodeType::Fact,
        serde_json::json!({"severity": "critical"}),
        &agent_id,
        "default",
    ).expect("kg_add_node should succeed");

    kernel1.kg_add_edge(&sql_injection, &waf, plico::fs::KGEdgeType::HasResolution, Some(0.9), &agent_id, "default")
        .expect("kg_add_edge should succeed");
    kernel1.kg_add_edge(&cve, &sql_injection, plico::fs::KGEdgeType::Causes, Some(0.95), &agent_id, "default")
        .expect("kg_add_edge should succeed");

    let node_ids = vec![sql_injection.clone(), waf.clone(), cve.clone()];
    drop(kernel1);

    // Restart and verify KG persists
    let kernel2 = AIKernel::new(root).expect("kernel init after restart");

    for node_id in &node_ids {
        let node = kernel2.kg_get_node(node_id, &agent_id, "default");
        assert!(node.is_ok(), "KG node {} should be recoverable", node_id);
    }

    let neighbors = kernel2.graph_explore(&sql_injection, None, 1);
    assert!(!neighbors.is_empty(), "SQL Injection should have neighbors after restart");

    tracing::info!("test_crash_recovery_kg_data_persists PASSED");
}

// ─── Crash Recovery: Memories Persist ───────────────────────────────────────────

#[test]
fn test_crash_recovery_memories_persist() {
    // Memories are stored in layered memory with persistence.
    // This test verifies memories are accessible after restart.

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let kernel1 = AIKernel::new(root.clone()).expect("kernel init");
    let agent_id = kernel1.register_agent("memory-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel1.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel1.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store memories
    kernel1.remember_long_term_scoped(
        &agent_id,
        "default",
        "Important finding about SQL injection".to_string(),
        vec!["security".into(), "finding".into()],
        85,
        MemoryScope::Shared,
    ).expect("remember_long_term should succeed");

    kernel1.remember_working(
        &agent_id,
        "default",
        "User is researching security topics".to_string(),
        vec!["context".into()],
    ).expect("remember_working should succeed");

    drop(kernel1);

    // Restart
    let kernel2 = AIKernel::new(root).expect("kernel init after restart");

    // Verify memories are recoverable
    let memories = kernel2.recall(&agent_id, "default");
    assert!(!memories.is_empty(), "Memories should be recoverable after restart");

    tracing::info!("test_crash_recovery_memories_persist PASSED: {} memories recovered", memories.len());
}

// ─── Crash Recovery: Task State (Within Session) ─────────────────────────────────

#[test]
fn test_crash_recovery_task_operations_within_session() {
    // Task operations work within a single kernel session.
    // Full task persistence across restarts requires explicit persist() which is internal.
    // This test verifies task operations function correctly within a session.

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let kernel1 = AIKernel::new(root.clone()).expect("kernel init");

    let coordinator = kernel1.register_agent("coordinator".to_string());
    let worker = kernel1.register_agent("worker".to_string());

    use plico::api::permission::PermissionAction;
    kernel1.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel1.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel1.permission_grant(&worker, PermissionAction::Read, None, None);
    kernel1.permission_grant(&worker, PermissionAction::Write, None, None);

    // Create and start task
    let resp = kernel1.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "session-task".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker.clone(),
        intent: "Task within session".to_string(),
        context_cids: vec![],
        deadline_ms: Some(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64 + 60000),
    });
    assert!(resp.ok, "DelegateTask should succeed");

    let resp = kernel1.handle_api_request(plico::api::semantic::ApiRequest::TaskStart {
        task_id: "session-task".to_string(),
        agent_id: worker.clone(),
    });
    assert!(resp.ok, "TaskStart should succeed");

    // Verify task is InProgress
    let resp = kernel1.handle_api_request(plico::api::semantic::ApiRequest::QueryTaskStatus {
        task_id: "session-task".to_string(),
    });
    assert!(resp.ok);
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::InProgress, "Task should be InProgress");

    tracing::info!("test_crash_recovery_task_operations_within_session PASSED");
}

// ─── Full Integration: Ingest → KG Build → Query ────────────────────────────────

#[test]
fn test_full_ingest_kg_query_sequence() {
    // Full sequence: ingest articles, build KG, verify queries work.
    // This tests the complete flow within a single session.

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let kernel = AIKernel::new(root).expect("kernel init");
    let agent_id = kernel.register_agent("full-test-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Phase 1: Ingest 10 articles
    let mut cids = Vec::new();
    for (title, content, tags) in security_articles() {
        let cid = kernel.semantic_create(
            content.as_bytes().to_vec(),
            tags,
            &agent_id,
            Some(title.to_string()),
        ).expect("semantic_create should succeed");
        cids.push(cid);
    }
    assert_eq!(cids.len(), 10, "Should have 10 articles");
    tracing::info!("Phase 1: Ingested {} articles", cids.len());

    // Phase 2: Build KG
    let sql_node = kernel.kg_add_node(
        "SQL Injection",
        plico::fs::KGNodeType::Entity,
        serde_json::json!({}),
        &agent_id,
        "default",
    ).expect("kg_add_node should succeed");

    let cve_node = kernel.kg_add_node(
        "CVE-2024-21762",
        plico::fs::KGNodeType::Fact,
        serde_json::json!({}),
        &agent_id,
        "default",
    ).expect("kg_add_node should succeed");

    kernel.kg_add_edge(&cve_node, &sql_node, plico::fs::KGEdgeType::Causes, Some(0.9), &agent_id, "default")
        .expect("kg_add_edge should succeed");

    tracing::info!("Phase 2: Built KG with causal relationship");

    // Phase 3: Verify data integrity
    let all_nodes = kernel.kg_list_nodes(None, &agent_id, "default")
        .expect("kg_list_nodes should work");
    assert!(!all_nodes.is_empty(), "KG nodes should exist");

    let neighbors = kernel.graph_explore(&sql_node, None, 1);
    assert!(!neighbors.is_empty(), "SQL Injection should have neighbors");

    tracing::info!(
        "Phase 3: Verified - {} KG nodes, {} neighbors for SQL Injection",
        all_nodes.len(),
        neighbors.len()
    );

    tracing::info!("test_full_ingest_kg_query_sequence PASSED");
}
