//! Node4 Sprint 3: Knowledge Event Propagation Tests
//!
//! Tests for F-12: KnowledgeShared and KnowledgeSuperseded events.
//! Verifies event propagation between agents, scope isolation, and summary generation.
//!
//! Design: F-12 in docs/design-node4-collaborative-ecosystem.md

use plico::kernel::AIKernel;
use plico::api::semantic::GrowthPeriod;
use plico::memory::MemoryScope;
use tempfile::tempdir;

/// Create a kernel with stub embedding for testing.
fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

// ─── KnowledgeShared Event Tests ─────────────────────────────────────────────

#[test]
fn test_knowledge_shared_scope_shared_emits_event() {
    let (kernel, _dir) = make_kernel();

    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Write, None, None);

    // Subscribe agent_b to KnowledgeShared events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_b.clone(),
        event_types: Some(vec!["KnowledgeShared".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Agent A stores Shared memory
    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "Key finding: XSS vulnerability in search field".to_string(),
        vec!["security".into(), "xss".into(), "finding".into()],
        80,
        MemoryScope::Shared,
    ).expect("remember_long_term_scoped should succeed");

    // Agent B polls and should receive the event
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok, "EventPoll should succeed");
    let events = resp.kernel_events.unwrap();

    let shared_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeShared { .. }))
        .collect();

    assert_eq!(shared_events.len(), 1, "Should receive exactly one KnowledgeShared event");

    if let plico::kernel::event_bus::KernelEvent::KnowledgeShared { scope, tags, summary, .. } = &shared_events[0] {
        assert_eq!(scope, "shared", "Scope should be 'shared'");
        assert!(!tags.is_empty(), "Tags should be non-empty");
        assert!(!summary.is_empty(), "Summary should be non-empty (metadata concatenation)");
        assert!(summary.contains("agent-a") || summary.contains("security") || summary.contains("xss"),
            "Summary should contain meaningful metadata");
    }

    tracing::info!("test_knowledge_shared_scope_shared_emits_event PASSED");
}

#[test]
fn test_knowledge_shared_scope_private_does_not_emit() {
    let (kernel, _dir) = make_kernel();

    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);

    // Subscribe agent_b to all KnowledgeShared events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_b.clone(),
        event_types: Some(vec!["KnowledgeShared".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Agent A stores Private memory
    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "My private notes about security".to_string(),
        vec!["private".into()],
        50,
        MemoryScope::Private,
    ).expect("remember_long_term_scoped should succeed");

    // Agent B polls - should NOT receive any KnowledgeShared event
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();

    let shared_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeShared { .. }))
        .collect();

    assert!(shared_events.is_empty(), "Private memory should NOT emit KnowledgeShared event");

    tracing::info!("test_knowledge_shared_scope_private_does_not_emit PASSED");
}

#[test]
fn test_knowledge_shared_scope_group_emits_event() {
    let (kernel, _dir) = make_kernel();

    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Write, None, None);

    // Subscribe agent_b to KnowledgeShared events for group:security-team
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_b.clone(),
        event_types: Some(vec!["KnowledgeShared".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Agent A stores Group-scoped memory
    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "Security team finding: CSRF in logout endpoint".to_string(),
        vec!["security".into(), "csrf".into(), "security-team".into()],
        85,
        MemoryScope::Group("security-team".into()),
    ).expect("remember_long_term_scoped should succeed");

    // Agent B polls
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();

    let group_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeShared { scope, .. }
            if scope.contains("security-team")))
        .collect();

    assert!(!group_events.is_empty(), "Group memory should emit KnowledgeShared with group scope");

    tracing::info!("test_knowledge_shared_scope_group_emits_event PASSED");
}

#[test]
fn test_knowledge_shared_event_summary_metadata_concat() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("test-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store shared memory with multiple tags
    kernel.remember_long_term_scoped(
        &agent_id,
        "default",
        "CVE-2024-1234 affects Apache Struts 2".to_string(),
        vec!["cve".into(), "apache".into(), "rce".into(), "critical".into()],
        95,
        MemoryScope::Shared,
    ).expect("remember_long_term_scoped should succeed");

    // Check event was emitted via EventHistory API
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None,
        agent_id_filter: Some(agent_id.clone()),
        limit: Some(100),
    });

    assert!(resp.ok, "EventHistory should succeed");
    let events = resp.event_history.unwrap();
    let shared_events: Vec<_> = events.iter()
        .filter(|e| matches!(e.event, plico::kernel::event_bus::KernelEvent::KnowledgeShared { .. }))
        .collect();

    assert!(!shared_events.is_empty(), "Should have KnowledgeShared event");

    if let plico::kernel::event_bus::KernelEvent::KnowledgeShared { summary, tags, .. } = &shared_events.last().unwrap().event {
        // Summary should be metadata concatenation (not LLM-generated)
        assert!(!summary.is_empty(), "Summary should be non-empty");
        assert!(!tags.is_empty(), "Tags should be non-empty");
        // Summary should include tag info or agent info (not raw content)
        assert!(summary.len() < 200, "Summary should be short (metadata concat, not full content)");
    }

    tracing::info!("test_knowledge_shared_event_summary_metadata_concat PASSED");
}

// ─── KnowledgeSuperseded Event Tests ─────────────────────────────────────────

#[test]
fn test_knowledge_superseded_event_on_update() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("test-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Subscribe to KnowledgeSuperseded events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_id.clone(),
        event_types: Some(vec!["KnowledgeSuperseded".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Create original document
    let original_cid = kernel.semantic_create(
        b"SQL injection prevention using parameterized queries".to_vec(),
        vec!["security".into(), "sql-injection".into()],
        &agent_id,
        Some("Original".into()),
    ).expect("semantic_create should succeed");

    // Update the document (creates new version)
    let new_cid = kernel.semantic_update(
        &original_cid,
        b"Updated: SQL injection prevention using parameterized queries and input validation".to_vec(),
        None,
        &agent_id,
        "default",
    ).expect("semantic_update should succeed");

    assert_ne!(original_cid, new_cid, "Update should create new CID");

    // Poll for events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();

    // Should have KnowledgeSuperseded event
    let superseded_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeSuperseded { .. }))
        .collect();

    assert!(!superseded_events.is_empty(), "Update should emit KnowledgeSuperseded event");

    if let plico::kernel::event_bus::KernelEvent::KnowledgeSuperseded { old_cid, new_cid: returned_new, .. } = &superseded_events[0] {
        assert_eq!(old_cid, &original_cid, "Old CID should be the original");
        assert_eq!(returned_new, &new_cid, "New CID should be the updated one");
    }

    tracing::info!("test_knowledge_superseded_event_on_update PASSED");
}

// ─── Knowledge Event via GrowthReport Integration ─────────────────────────────

#[test]
fn test_growth_report_reflects_knowledge_shared() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("growth-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store several shared memories
    for i in 0..5 {
        kernel.remember_long_term_scoped(
            &agent_id,
            "default",
            format!("Important finding #{}", i),
            vec![format!("finding-{}", i)],
            70,
            MemoryScope::Shared,
        ).expect("remember_long_term_scoped should succeed");
    }

    // Query growth report
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::QueryGrowthReport {
        agent_id: agent_id.clone(),
        period: GrowthPeriod::AllTime,
    });

    assert!(resp.ok, "QueryGrowthReport should succeed");
    let report = resp.growth_report.unwrap();

    assert_eq!(report.agent_id, agent_id);
    assert!(report.memories_shared >= 5, "memories_shared should reflect stored Shared memories");

    tracing::info!(
        "test_growth_report_reflects_knowledge_shared: memories_stored={}, memories_shared={}",
        report.memories_stored,
        report.memories_shared
    );
}

// ─── Knowledge Event Broadcast Lag Recovery ────────────────────────────────────

#[test]
fn test_knowledge_event_broadcast_with_slow_consumer() {
    let (kernel, _dir) = make_kernel();

    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Write, None, None);

    // Agent B subscribes
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_b.clone(),
        event_types: Some(vec!["KnowledgeShared".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Agent A emits many events rapidly (simulating ingestion burst)
    for i in 0..20 {
        kernel.remember_long_term_scoped(
            &agent_a,
            "default",
            format!("Rapid knowledge entry #{}", i),
            vec![format!("entry-{}", i)],
            60,
            MemoryScope::Shared,
        ).expect("remember_long_term_scoped should succeed");
    }

    // Agent B polls - should recover via event_log even if broadcast lagged
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok, "EventPoll should succeed");
    let events = resp.kernel_events.unwrap();

    // Should receive events (possibly recovered from event_log)
    let shared_count = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeShared { .. }))
        .count();

    assert!(shared_count > 0, "Should receive KnowledgeShared events despite burst");

    tracing::info!(
        "test_knowledge_event_broadcast_with_slow_consumer: received {} events after burst",
        shared_count
    );
}
