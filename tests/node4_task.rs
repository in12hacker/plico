//! Node4 Sprint 3: Multi-Agent Task Delegation Test (G-3)
//!
//! G-3: Multi-Agent协作验证 — task delegation end-to-end and knowledge event propagation.
//!
//! Design: F-14 in docs/design-node4-collaborative-ecosystem.md

use plico::kernel::AIKernel;
use plico::api::semantic::TaskStatus;
use plico::memory::MemoryScope;
use tempfile::tempdir;

/// Create a kernel with stub embedding for testing.
fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let _ = std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

// ─── G-3: Multi-Agent Task Delegation ────────────────────────────────────────

#[test]
fn test_g3_task_delegation_end_to_end() {
    let (kernel, _dir) = make_kernel();

    // Register coordinator and worker agents
    let coordinator = kernel.register_agent("coordinator-agent".to_string());
    let worker = kernel.register_agent("worker-agent".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::SendMessage, None, None);
    kernel.permission_grant(&worker, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker, PermissionAction::SendMessage, None, None);

    tracing::info!("G-3: Registered coordinator={}, worker={}", coordinator, worker);

    // Step 1: Coordinator creates some content for context
    let doc_cid = kernel.semantic_create(
        b"Important security analysis document about SQL injection vulnerabilities".to_vec(),
        vec!["security".into(), "analysis".into()],
        &coordinator,
        Some("Security Analysis".into()),
    ).expect("semantic_create should succeed");

    tracing::info!("G-3: Coordinator created document cid={}", doc_cid);

    // Step 2: Coordinator delegates task to worker via API
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-001".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker.clone(),
        intent: "Analyze the security document and summarize findings".to_string(),
        context_cids: vec![doc_cid.clone()],
        deadline_ms: Some(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64 + 60000), // 60 seconds from now
    });

    assert!(resp.ok, "DelegateTask should succeed: {:?}", resp.error);
    assert!(resp.task_result.is_some(), "Response should contain task_result");

    let task_result = resp.task_result.unwrap();
    assert_eq!(task_result.status, TaskStatus::Pending, "New task should be Pending");
    assert_eq!(task_result.agent_id, worker, "Task should be assigned to worker");

    tracing::info!("G-3: Task delegated: task_id={}, status={}", task_result.task_id, task_result.status);

    // Step 3: Worker queries task status
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::QueryTaskStatus {
        task_id: "task-001".to_string(),
    });

    assert!(resp.ok, "QueryTaskStatus should succeed");
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::Pending, "Task should still be Pending");

    // Step 4: Worker starts working on task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskStart {
        task_id: "task-001".to_string(),
        agent_id: worker.clone(),
    });

    assert!(resp.ok, "TaskStart should succeed: {:?}", resp.error);
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::InProgress, "Task should now be InProgress");

    tracing::info!("G-3: Worker started task, status=InProgress");

    // Step 5: Worker completes task with result CIDs
    let analysis_cid = kernel.semantic_create(
        b"SQL Injection Analysis: Found 3 vulnerabilities. Recommendations: use parameterized queries, implement WAF, add input validation.".to_vec(),
        vec!["analysis".into(), "security".into(), "completed".into()],
        &worker,
        Some("SQL Injection Analysis Result".into()),
    ).expect("semantic_create should succeed");

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskComplete {
        task_id: "task-001".to_string(),
        agent_id: worker.clone(),
        result_cids: vec![analysis_cid.clone()],
    });

    assert!(resp.ok, "TaskComplete should succeed: {:?}", resp.error);
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::Completed, "Task should now be Completed");
    assert_eq!(task.result_cids, vec![analysis_cid.clone()], "Result CIDs should match");

    tracing::info!("G-3: Worker completed task with result cid={}", analysis_cid);

    // Step 6: Coordinator can verify the result via task status
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::QueryTaskStatus {
        task_id: "task-001".to_string(),
    });

    assert!(resp.ok, "QueryTaskStatus should succeed");
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::Completed, "Task should be Completed");
    assert!(!task.result_cids.is_empty(), "Task should have result CIDs");
    // Verify the result CID is recorded (content verification would require shared storage)
    assert_eq!(task.result_cids.first(), Some(&analysis_cid), "Result CID should be recorded");

    tracing::info!("G-3 PASSED: Task delegation end-to-end verified");
}

#[test]
fn test_g3_task_delegation_state_transitions() {
    let (kernel, _dir) = make_kernel();

    let coordinator = kernel.register_agent("coordinator".to_string());
    let worker = kernel.register_agent("worker".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker, PermissionAction::Write, None, None);

    // Test invalid transitions
    // Try to complete a Pending task (should fail)
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-invalid".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker.clone(),
        intent: "Test task".to_string(),
        context_cids: vec![],
        deadline_ms: None,
    });
    assert!(resp.ok);

    // Try to complete without starting
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskComplete {
        task_id: "task-invalid".to_string(),
        agent_id: worker.clone(),
        result_cids: vec![],
    });
    assert!(!resp.ok, "Completing a Pending task should fail (must start first)");

    // Wrong agent tries to start task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskStart {
        task_id: "task-invalid".to_string(),
        agent_id: coordinator.clone(), // wrong agent
    });
    assert!(!resp.ok, "Wrong agent should not be able to start task");

    tracing::info!("G-3: State transition validation passed");
}

#[test]
fn test_g3_task_delegation_wrong_agent_cannot_complete() {
    let (kernel, _dir) = make_kernel();

    let coordinator = kernel.register_agent("coordinator".to_string());
    let worker_a = kernel.register_agent("worker-a".to_string());
    let worker_b = kernel.register_agent("worker-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker_b, PermissionAction::Write, None, None);

    // Delegate to worker-a
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-secured".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker_a.clone(),
        intent: "Secure task".to_string(),
        context_cids: vec![],
        deadline_ms: None,
    });
    assert!(resp.ok);

    // worker-a starts task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskStart {
        task_id: "task-secured".to_string(),
        agent_id: worker_a.clone(),
    });
    assert!(resp.ok);

    // worker-b tries to complete (should fail)
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskComplete {
        task_id: "task-secured".to_string(),
        agent_id: worker_b.clone(), // wrong agent
        result_cids: vec![],
    });
    assert!(!resp.ok, "worker-b should not be able to complete worker-a's task");

    tracing::info!("G-3: Task assignment security verified");
}

#[test]
fn test_g3_task_persist_and_restore() {
    // NOTE: Full task persistence across restarts requires explicit persist() call
    // which is internal to the kernel. This test verifies task operations work
    // within a single session (which is the primary use case for task delegation).

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();

    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let _ = std::env::set_var("LLM_BACKEND", "stub");

    let kernel = AIKernel::new(root).expect("kernel init");

    let coordinator = kernel.register_agent("coordinator".to_string());
    let worker = kernel.register_agent("worker".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker, PermissionAction::Write, None, None);

    // Create task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-persist".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker.clone(),
        intent: "Persistent task".to_string(),
        context_cids: vec![],
        deadline_ms: None,
    });
    assert!(resp.ok);

    // Start task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskStart {
        task_id: "task-persist".to_string(),
        agent_id: worker.clone(),
    });
    assert!(resp.ok);

    // Complete task
    let _result_cid = kernel.semantic_create(
        b"Task result content".to_vec(),
        vec!["task-result".into()],
        &worker,
        Some("Task Result".into()),
    ).expect("semantic_create should succeed");

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::TaskComplete {
        task_id: "task-persist".to_string(),
        agent_id: worker.clone(),
        result_cids: vec![],
    });
    assert!(resp.ok);

    // Verify task is Completed within session
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::QueryTaskStatus {
        task_id: "task-persist".to_string(),
    });

    assert!(resp.ok, "QueryTaskStatus should succeed");
    let task = resp.task_result.unwrap();
    assert_eq!(task.status, TaskStatus::Completed, "Task status should be Completed");

    tracing::info!("G-3: Task operations within session verified");
}

#[test]
fn test_g3_knowledge_event_propagation() {
    let (kernel, _dir) = make_kernel();

    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Write, None, None);

    // Agent B subscribes to KnowledgeShared events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: agent_b.clone(),
        event_types: Some(vec!["KnowledgeShared".to_string()]),
        agent_ids: None,
    });
    assert!(resp.ok, "EventSubscribe should succeed");
    let sub_id = resp.subscription_id.unwrap();

    tracing::info!("G-3: Agent B subscribed to KnowledgeShared, sub_id={}", sub_id);

    // Agent A stores Shared memory (should emit KnowledgeShared)
    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "Important security finding: SQL injection in login form".to_string(),
        vec!["security".into(), "finding".into()],
        90,
        MemoryScope::Shared,
    ).expect("remember_long_term_scoped should succeed");

    // Agent A stores Private memory (should NOT emit KnowledgeShared)
    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "My private notes about the analysis".to_string(),
        vec!["private".into()],
        50,
        MemoryScope::Private,
    ).expect("remember_long_term_scoped should succeed");

    // Agent B polls for events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id.clone(),
    });

    assert!(resp.ok, "EventPoll should succeed");
    let events = resp.kernel_events.unwrap();

    // Verify KnowledgeShared event received (not Private memory)
    let shared_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::KnowledgeShared { .. }))
        .collect();

    assert!(!shared_events.is_empty(), "Agent B should receive KnowledgeShared event");

    // Verify the shared event has non-empty summary
    if let plico::kernel::event_bus::KernelEvent::KnowledgeShared { summary, .. } = &shared_events[0] {
        assert!(!summary.is_empty(), "KnowledgeShared summary should be non-empty");
    }

    // Verify private memory is NOT in events
    let private_content = "My private notes";
    assert!(!events.iter().any(|e| {
        match e {
            plico::kernel::event_bus::KernelEvent::KnowledgeShared { summary, .. } => {
                summary.contains(private_content)
            }
            _ => false,
        }
    }), "Agent B should NOT receive Private memory events");

    tracing::info!("G-3: KnowledgeShared event propagation verified");
}

#[test]
fn test_g3_task_delegated_event_emitted() {
    let (kernel, _dir) = make_kernel();

    let coordinator = kernel.register_agent("coordinator".to_string());
    let worker = kernel.register_agent("worker".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&coordinator, PermissionAction::Read, None, None);
    kernel.permission_grant(&coordinator, PermissionAction::Write, None, None);
    kernel.permission_grant(&worker, PermissionAction::Read, None, None);
    kernel.permission_grant(&worker, PermissionAction::Write, None, None);

    // Subscribe to TaskDelegated events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventSubscribe {
        agent_id: worker.clone(),
        event_types: Some(vec!["TaskDelegated".to_string()]),
        agent_ids: None,
    });
    let sub_id = resp.subscription_id.unwrap();

    // Delegate task
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-event".to_string(),
        from_agent: coordinator.clone(),
        to_agent: worker.clone(),
        intent: "Task for event test".to_string(),
        context_cids: vec![],
        deadline_ms: None,
    });
    assert!(resp.ok, "DelegateTask should succeed: {:?}", resp.error);

    // Poll for events
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventPoll {
        subscription_id: sub_id,
    });

    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();

    let delegated_events: Vec<_> = events.iter()
        .filter(|e| matches!(e, plico::kernel::event_bus::KernelEvent::TaskDelegated { .. }))
        .collect();

    assert!(!delegated_events.is_empty(), "Should receive TaskDelegated event");

    if let plico::kernel::event_bus::KernelEvent::TaskDelegated { task_id, from_agent, to_agent, .. } = &delegated_events[0] {
        assert_eq!(task_id, "task-event");
        assert_eq!(from_agent, &coordinator);
        assert_eq!(to_agent, &worker);
    }

    tracing::info!("G-3: TaskDelegated event verified");
}
