//! AIKernel unit tests
//!
//! Tests cover: kernel creation, object CRUD, semantic operations,
//! agent registration, memory operations, permission enforcement,
//! and KernelExecutor integration.

use plico::kernel::AIKernel;
use std::sync::Arc;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

fn make_kernel_arc() -> (Arc<AIKernel>, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = Arc::new(AIKernel::new(dir.path().to_path_buf()).expect("kernel init"));
    (kernel, dir)
}

#[test]
fn test_kernel_create_and_get() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(
            b"Agent task output: embedding batch result".to_vec(),
            vec!["embedding".to_string(), "batch-result".to_string()],
            "TestAgent",
            Some("Embedding computation output".to_string()),
        )
        .expect("create failed");

    let obj = kernel
        .get_object(&cid, "TestAgent")
        .expect("get failed");

    assert_eq!(obj.data, b"Agent task output: embedding batch result");
    assert_eq!(obj.meta.tags, vec!["embedding", "batch-result"]);
    assert_eq!(obj.meta.created_by, "TestAgent");
}

#[test]
fn test_kernel_semantic_create_and_read() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(
            b"Rust async programming discussion".to_vec(),
            vec!["rust".to_string(), "async".to_string()],
            "DevAgent",
            None,
        )
        .expect("create failed");

    // Read by CID
    let objs = kernel
        .semantic_read(&plico::fs::Query::ByCid(cid.clone()), "DevAgent")
        .expect("read failed");
    assert_eq!(objs.len(), 1);
    assert_eq!(objs[0].data, b"Rust async programming discussion");
}

#[test]
fn test_kernel_read_by_tags() {
    let (kernel, _dir) = make_kernel();

    kernel.semantic_create(b"doc1".to_vec(), vec!["a".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc2".to_vec(), vec!["a".to_string(), "b".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc3".to_vec(), vec!["b".to_string()], "x", None).ok();

    let objs = kernel
        .semantic_read(&plico::fs::Query::ByTags(vec!["a".to_string()]), "x")
        .expect("read by tags failed");

    assert_eq!(objs.len(), 2);
}

#[test]
fn test_kernel_update_changes_cid() {
    let (kernel, _dir) = make_kernel();

    let old_cid = kernel
        .semantic_create(b"original".to_vec(), vec!["t".to_string()], "x", None)
        .expect("create failed");

    let new_cid = kernel
        .semantic_update(&old_cid, b"updated".to_vec(), None, "x")
        .expect("update failed");

    // Content changed → new CID
    assert_ne!(new_cid, old_cid);

    // Old object still exists (immutable CAS)
    let old_obj = kernel.get_object(&old_cid, "x").expect("old should exist");
    assert_eq!(old_obj.data, b"original");

    // New object has new content
    let new_obj = kernel.get_object(&new_cid, "x").expect("new should exist");
    assert_eq!(new_obj.data, b"updated");
}

#[test]
fn test_kernel_delete_requires_permission() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(b"secret".to_vec(), vec!["private".to_string()], "x", None)
        .expect("create failed");

    // 'cli' agent has no Delete grant by default
    let result = kernel.semantic_delete(&cid, "cli");
    assert!(result.is_err(), "delete should fail without permission");

    // Object still readable by owner (isolation prevents 'cli' from reading 'x' data)
    let obj = kernel.get_object(&cid, "x").expect("should still exist");
    assert_eq!(obj.data, b"secret");

    // 'cli' cannot read 'x' data (ownership isolation)
    let result = kernel.get_object(&cid, "cli");
    assert!(result.is_err(), "cli should not read x's object");
}

#[test]
fn test_kernel_agent_isolation() {
    let (kernel, _dir) = make_kernel();

    let cid_a = kernel
        .semantic_create(b"agent A data".to_vec(), vec!["shared-tag".to_string()], "agent-a", None)
        .expect("create by A failed");
    let cid_b = kernel
        .semantic_create(b"agent B data".to_vec(), vec!["shared-tag".to_string()], "agent-b", None)
        .expect("create by B failed");

    // A can read own object
    let obj_a = kernel.get_object(&cid_a, "agent-a").expect("A reads own");
    assert_eq!(obj_a.data, b"agent A data");

    // A cannot read B's object
    let result = kernel.get_object(&cid_b, "agent-a");
    assert!(result.is_err(), "A should not read B's object");

    // B can read own object
    let obj_b = kernel.get_object(&cid_b, "agent-b").expect("B reads own");
    assert_eq!(obj_b.data, b"agent B data");

    // Trusted "kernel" can read both
    let obj_a2 = kernel.get_object(&cid_a, "kernel").expect("kernel reads A");
    assert_eq!(obj_a2.data, b"agent A data");

    // A search only returns own objects
    let results = kernel.semantic_read(
        &plico::fs::Query::ByTags(vec!["shared-tag".to_string()]),
        "agent-a",
    ).expect("read by tags");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].meta.created_by, "agent-a");

    // kernel search returns both
    let results = kernel.semantic_read(
        &plico::fs::Query::ByTags(vec!["shared-tag".to_string()]),
        "kernel",
    ).expect("kernel read by tags");
    assert_eq!(results.len(), 2);
}

#[test]
fn test_kernel_agent_registration() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("MyAgent".to_string());
    assert!(!id.is_empty());

    let agents = kernel.list_agents();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "MyAgent");
}

#[test]
fn test_kernel_remember_and_recall() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "Remember to check the logs".to_string());
    let memories = kernel.recall("agent1");

    assert!(!memories.is_empty());
    assert!(memories.iter().any(|m| m.content.display().contains("logs")));
}

#[test]
fn test_kernel_forget_ephemeral() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "Temporary note".to_string());
    assert!(!kernel.recall("agent1").is_empty());

    kernel.forget_ephemeral("agent1");
    let memories = kernel.recall("agent1");

    // Ephemeral tier entries should be gone
    let ephemeral: Vec<_> = memories
        .iter()
        .filter(|m| matches!(m.tier, plico::memory::MemoryTier::Ephemeral))
        .collect();
    assert!(ephemeral.is_empty(), "ephemeral memories should be cleared");
}

#[test]
fn test_kernel_list_tags() {
    let (kernel, _dir) = make_kernel();

    kernel.semantic_create(b"doc1".to_vec(), vec!["a".to_string(), "b".to_string()], "x", None).ok();
    kernel.semantic_create(b"doc2".to_vec(), vec!["b".to_string(), "c".to_string()], "x", None).ok();

    let tags = kernel.list_tags();
    assert!(tags.contains(&"a".to_string()));
    assert!(tags.contains(&"b".to_string()));
    assert!(tags.contains(&"c".to_string()));
}

#[test]
fn test_kernel_list_deleted_after_delete() {
    let (kernel, _dir) = make_kernel();

    // No deleted objects initially
    assert!(kernel.list_deleted("kernel").is_empty());

    let cid = kernel
        .semantic_create(b"to be deleted".to_vec(), vec!["temp".to_string()], "kernel", None)
        .expect("create failed");

    // "kernel" has all permissions granted by default
    kernel.semantic_delete(&cid, "kernel").expect("delete failed");

    let deleted = kernel.list_deleted("kernel");
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].cid, cid);
    assert_eq!(deleted[0].original_meta.tags, vec!["temp"]);
}

#[test]
fn test_kernel_restore_deleted() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(b"restore me".to_vec(), vec!["restore-test".to_string()], "kernel", None)
        .expect("create failed");

    kernel.semantic_delete(&cid, "kernel").expect("delete failed");
    assert_eq!(kernel.list_deleted("kernel").len(), 1);

    kernel.restore_deleted(&cid, "kernel").expect("restore failed");

    // Should no longer appear in recycle bin
    assert!(kernel.list_deleted("kernel").is_empty());

    // Object should be searchable by tag again
    let results = kernel
        .semantic_read(&plico::fs::Query::ByTags(vec!["restore-test".to_string()]), "kernel")
        .expect("read after restore failed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].data, b"restore me");
}

#[test]
fn test_kernel_agent_lifecycle() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("LifecycleAgent".to_string());

    let (_, state, pending) = kernel.agent_status(&id).expect("status");
    assert_eq!(state, "Created");
    assert_eq!(pending, 0);

    kernel.submit_intent(
        plico::scheduler::IntentPriority::Medium,
        "test task".to_string(),
        None,
        Some(id.clone()),
    );
    let (_, _, pending) = kernel.agent_status(&id).expect("status");
    assert_eq!(pending, 1);

    kernel.agent_suspend(&id).expect("suspend");
    let (_, state, _) = kernel.agent_status(&id).expect("status");
    assert_eq!(state, "Suspended");

    kernel.agent_resume(&id).expect("resume");
    let (_, state, _) = kernel.agent_status(&id).expect("status");
    assert_eq!(state, "Waiting");

    kernel.agent_terminate(&id).expect("terminate");
    let (_, state, _) = kernel.agent_status(&id).expect("status");
    assert_eq!(state, "Terminated");

    let result = kernel.agent_suspend(&id);
    assert!(result.is_err(), "cannot suspend terminated agent");
}

#[test]
fn test_kernel_agent_persists_across_restart() {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let id;
    {
        let kernel = AIKernel::new(root.clone()).expect("kernel init");
        id = kernel.register_agent("PersistentAgent".to_string());
        let agents = kernel.list_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "PersistentAgent");
    }

    {
        let kernel2 = AIKernel::new(root).expect("kernel2 init");
        let agents = kernel2.list_agents();
        assert_eq!(agents.len(), 1, "agent should survive restart");
        assert_eq!(agents[0].id, id);
        assert_eq!(agents[0].name, "PersistentAgent");
    }
}

#[test]
fn test_kernel_intent_persists_across_restart() {
    use plico::scheduler::agent::IntentPriority;
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    {
        let kernel = AIKernel::new(root.clone()).expect("kernel init");
        let action = serde_json::to_string(&ApiRequest::Create {
            content: "persisted intent test".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["intent-persist".to_string()],
            agent_id: "test".to_string(),
            intent: None,
        }).unwrap();
        kernel.submit_intent(
            IntentPriority::High,
            "test persistent intent".to_string(),
            Some(action),
            Some("test-agent".to_string()),
        );
    }

    {
        let kernel2 = AIKernel::new(root).expect("kernel2 init");
        assert_eq!(kernel2.pending_intent_count(), 1, "intent should survive restart");
    }
}

#[test]
fn test_kernel_graph_explore_raw_empty() {
    let (kernel, _dir) = make_kernel();
    // A CID with no graph edges returns an empty slice
    let hits = kernel.graph_explore_raw("nonexistent-cid", None, 1);
    assert!(hits.is_empty());
}

#[test]
fn test_kernel_handle_api_request_create_and_read() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let (kernel, _dir) = make_kernel();
    let create_req = ApiRequest::Create {
        content: "Hello from KernelExecutor".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["test".to_string(), "executor".to_string()],
        agent_id: "test-agent".to_string(),
        intent: None,
    };
    let resp = kernel.handle_api_request(create_req);
    assert!(resp.ok);
    let cid = resp.cid.expect("should have cid");

    let read_req = ApiRequest::Read {
        cid: cid.clone(),
        agent_id: "test-agent".to_string(),
    };
    let resp = kernel.handle_api_request(read_req);
    assert!(resp.ok);
    assert_eq!(resp.data.unwrap(), "Hello from KernelExecutor");
}

#[test]
fn test_kernel_executor_dispatches_intent_action() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};
    use plico::scheduler::dispatch::{KernelExecutor, AgentExecutor};
    use plico::scheduler::agent::{Intent, IntentPriority};

    let (kernel, _dir) = make_kernel_arc();

    let action = serde_json::to_string(&ApiRequest::Create {
        content: "Created via intent execution".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["intent-created".to_string()],
        agent_id: "executor-test-agent".to_string(),
        intent: Some("test intent".to_string()),
    }).unwrap();

    let kernel_ref = Arc::clone(&kernel);
    let executor = KernelExecutor::new(move |json: &str| {
        let req: ApiRequest = serde_json::from_str(json).unwrap();
        let resp = kernel_ref.handle_api_request(req);
        serde_json::to_string(&resp).unwrap()
    });

    let intent = Intent::new(IntentPriority::High, "Create a test object".to_string())
        .with_action(action);
    let result = executor.execute(&intent, None, 5000);
    assert!(result.is_ok());

    let response_json = result.unwrap();
    let resp: plico::api::semantic::ApiResponse = serde_json::from_str(&response_json).unwrap();
    assert!(resp.ok);
    let cid = resp.cid.expect("should have cid from execution");

    let obj = kernel.get_object(&cid, "executor-test-agent").unwrap();
    assert_eq!(obj.data, b"Created via intent execution");
}

#[test]
fn test_kernel_executor_no_action_acknowledged() {
    use plico::scheduler::dispatch::{KernelExecutor, AgentExecutor};
    use plico::scheduler::agent::{Intent, IntentPriority};

    let executor = KernelExecutor::new(|_| "should not be called".to_string());
    let intent = Intent::new(IntentPriority::Low, "descriptive only".to_string());
    let result = executor.execute(&intent, None, 5000);
    assert!(result.is_ok());
    assert!(result.unwrap().contains("No action"));
}

#[test]
fn test_kernel_executor_invalid_json_returns_error() {
    use plico::scheduler::dispatch::{KernelExecutor, AgentExecutor};
    use plico::scheduler::agent::{Intent, IntentPriority};

    let (kernel, _dir) = make_kernel_arc();
    let kernel_ref = Arc::clone(&kernel);
    let executor = KernelExecutor::new(move |json: &str| {
        let req: Result<plico::api::semantic::ApiRequest, _> = serde_json::from_str(json);
        match req {
            Ok(r) => serde_json::to_string(&kernel_ref.handle_api_request(r)).unwrap(),
            Err(e) => serde_json::to_string(
                &plico::api::semantic::ApiResponse::error(format!("Invalid JSON: {e}"))
            ).unwrap(),
        }
    });

    let intent = Intent::new(IntentPriority::Medium, "bad action".to_string())
        .with_action("{invalid json!!!".to_string());
    let result = executor.execute(&intent, None, 5000);
    assert!(result.is_ok());
    let resp: plico::api::semantic::ApiResponse =
        serde_json::from_str(&result.unwrap()).unwrap();
    assert!(!resp.ok);
}

#[tokio::test]
async fn test_dispatch_loop_with_kernel_executor() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};
    use plico::scheduler::agent::IntentPriority;

    let (kernel, _dir) = make_kernel_arc();
    let dispatch = kernel.start_dispatch_loop();

    let action = serde_json::to_string(&ApiRequest::Create {
        content: "Dispatch loop integration test".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["dispatch-test".to_string()],
        agent_id: "dispatch-agent".to_string(),
        intent: Some("test dispatch".to_string()),
    }).unwrap();

    kernel.submit_intent(
        IntentPriority::High,
        "dispatch test".to_string(),
        Some(action),
        Some("dispatch-agent".to_string()),
    );

    tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;

    let results = dispatch.drain_results().await;
    assert!(!results.is_empty(), "dispatch loop should have produced a result");
    assert!(results[0].success, "execution should succeed");

    let resp: plico::api::semantic::ApiResponse =
        serde_json::from_str(&results[0].output).unwrap();
    assert!(resp.ok);
    assert!(resp.cid.is_some());

    dispatch.shutdown();
}

#[test]
fn test_kernel_graph_explore_raw_returns_tuples() {
    let (kernel, _dir) = make_kernel();
    let cid = kernel
        .semantic_create(b"graph node A".to_vec(), vec!["graph".to_string()], "x", None)
        .expect("create failed");

    // No edges added — just confirm the call succeeds and returns the expected tuple shape
    let hits = kernel.graph_explore_raw(&cid, None, 1);
    // With no edges, result should be empty
    assert!(hits.is_empty());
}

/// End-to-end agent autonomy test:
///   register → submit intent → kernel executes → verify object created →
///   remember → suspend → persist → restart kernel → restore → recall
#[test]
fn test_e2e_agent_autonomy_cycle() {
    use plico::api::semantic::{ApiRequest, ApiResponse, ContentEncoding};
    use plico::scheduler::agent::IntentPriority;

    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    let agent_id;
    let created_cid;

    // ── Phase 1: Register + Submit Intent + Execute + Remember ──────
    {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let kernel = Arc::new(AIKernel::new(root.clone()).expect("kernel init"));

        let dispatch = rt.block_on(async { kernel.start_dispatch_loop() });

        agent_id = kernel.register_agent("E2ETestAgent".to_string());
        let (_, state, _) = kernel.agent_status(&agent_id).expect("status");
        assert_eq!(state, "Created");

        let action = serde_json::to_string(&ApiRequest::Create {
            content: "E2E: agent-created data via intent execution".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["e2e-test".to_string(), "agent-created".to_string()],
            agent_id: agent_id.clone(),
            intent: Some("e2e autonomy test".to_string()),
        }).unwrap();

        let intent_id = kernel.submit_intent(
            IntentPriority::Critical,
            "Create test object".to_string(),
            Some(action),
            Some(agent_id.clone()),
        );
        assert!(!intent_id.is_empty());

        rt.block_on(async {
            tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
        });

        let results = rt.block_on(dispatch.drain_results());
        assert!(!results.is_empty(), "dispatch should have executed the intent");
        let result = &results[0];
        assert!(result.success, "execution should succeed: {}", result.output);

        let resp: ApiResponse = serde_json::from_str(&result.output).unwrap();
        assert!(resp.ok);
        created_cid = resp.cid.clone().expect("should have created object");

        let obj = kernel.get_object(&created_cid, &agent_id).expect("get object");
        assert_eq!(obj.data, b"E2E: agent-created data via intent execution");
        assert_eq!(obj.meta.created_by, agent_id);

        let iso_result = kernel.get_object(&created_cid, "other-agent");
        assert!(iso_result.is_err(), "other agent should not read this");

        kernel.remember_working(
            &agent_id,
            "E2E test completed successfully".to_string(),
            vec!["e2e".to_string()],
        );

        kernel.agent_suspend(&agent_id).expect("suspend");
        let (_, state, _) = kernel.agent_status(&agent_id).expect("status after suspend");
        assert_eq!(state, "Suspended");

        kernel.persist_memories();
        kernel.persist_agents();
        kernel.persist_intents();

        dispatch.shutdown();
    }

    // ── Phase 2: Restart Kernel → Restore → Verify ──────────────────
    {
        let kernel2 = AIKernel::new(root).expect("kernel2 init");

        let agents = kernel2.list_agents();
        assert!(!agents.is_empty(), "agents should survive restart");
        let restored = agents.iter().find(|a| a.id == agent_id);
        assert!(restored.is_some(), "E2ETestAgent should be restored");
        assert_eq!(restored.unwrap().name, "E2ETestAgent");

        let obj = kernel2.get_object(&created_cid, &agent_id).expect("restored read");
        assert_eq!(obj.data, b"E2E: agent-created data via intent execution");

        let memories = kernel2.recall(&agent_id);
        let has_e2e = memories.iter().any(|m| m.content.display().contains("E2E test completed"));
        assert!(has_e2e, "memory should survive restart: got {:?}",
            memories.iter().map(|m| m.content.display()).collect::<Vec<_>>());
    }
}

// ─── v0.4 E2E: Tool Abstraction + Cognitive Memory ────────────────────

#[test]
fn test_e2e_tool_cognitive_memory_cycle() {
    use plico::api::semantic::ApiRequest;
    use serde_json::json;

    let (kernel, dir) = make_kernel();
    let root = dir.path().to_path_buf();

    // 1. Register an agent
    let agent_id = kernel.register_agent("ToolTestAgent".to_string());

    // 2. Discover tools via tool_list
    let list_resp = kernel.handle_api_request(ApiRequest::ToolList {
        agent_id: agent_id.clone(),
    });
    assert!(list_resp.ok);
    let tools = list_resp.tools.unwrap();
    assert!(tools.len() >= 19, "expected 19+ tools, got {}", tools.len());
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(tool_names.contains(&"cas.create"));
    assert!(tool_names.contains(&"memory.store"));
    assert!(tool_names.contains(&"tools.list"));

    // 3. Create data via tool call
    let create_result = kernel.execute_tool(
        "cas.create",
        &json!({"content": "v0.4 tool-created object", "tags": ["v0.4", "e2e"]}),
        &agent_id,
    );
    assert!(create_result.success, "cas.create failed: {:?}", create_result.error);
    let created_cid = create_result.output["cid"].as_str().unwrap().to_string();

    // 4. Store memory via tool call with importance
    let mem_result = kernel.execute_tool(
        "memory.store",
        &json!({"content": "important cognitive memory", "tier": "working", "importance": 80, "tags": ["e2e"]}),
        &agent_id,
    );
    assert!(mem_result.success);

    // 5. Store ephemeral memory with TTL (1ms — will expire immediately)
    let ttl_result = kernel.execute_tool(
        "memory.store",
        &json!({"content": "ephemeral with TTL", "tier": "ephemeral", "ttl_ms": 1}),
        &agent_id,
    );
    assert!(ttl_result.success);

    std::thread::sleep(std::time::Duration::from_millis(10));

    // 6. Verify TTL eviction
    let evicted = kernel.evict_expired(&agent_id);
    assert!(evicted >= 1, "expected at least 1 expired entry evicted, got {}", evicted);

    // 7. Store ephemeral memories and access them for promotion
    for _ in 0..4 {
        kernel.remember(&agent_id, "frequently accessed memory".to_string());
    }
    // Use recall() which just reads; then use recall_relevant to trigger tracking
    for _ in 0..4 {
        let _ = kernel.recall_relevant(&agent_id, 10000);
    }
    // After enough accesses, promotion should occur
    kernel.promote_check(&agent_id);
    let all_memories = kernel.recall(&agent_id);
    let promoted = all_memories.iter().any(|m| {
        m.content.display().contains("frequently accessed memory")
            && m.tier == plico::memory::MemoryTier::Working
    });
    assert!(promoted, "expected ephemeral memory to be promoted to working, tiers: {:?}",
        all_memories.iter().map(|m| (m.tier, m.access_count, m.content.display())).collect::<Vec<_>>());

    // 8. Test recall_relevant with budget
    let relevant = kernel.recall_relevant(&agent_id, 1000);
    assert!(!relevant.is_empty(), "recall_relevant should return memories");

    // 9. Suspend agent → verify context snapshot
    kernel.agent_suspend(&agent_id).expect("suspend failed");
    let all_after_suspend = kernel.recall(&agent_id);
    let has_snapshot = all_after_suspend.iter().any(|m| {
        m.tags.contains(&"plico:internal:snapshot".to_string())
    });
    assert!(has_snapshot, "suspend should create a context snapshot");

    // 10. Persist everything
    kernel.persist_agents();
    kernel.persist_intents();
    kernel.persist_memories();
    kernel.persist_search_index();

    // 11. Resume agent → verify snapshot loaded into ephemeral
    kernel.agent_resume(&agent_id).expect("resume failed");
    let all_after_resume = kernel.recall(&agent_id);
    let has_context = all_after_resume.iter().any(|m| {
        m.content.display().contains("Context restored")
    });
    assert!(has_context, "resume should load context snapshot into ephemeral");

    // 12. Verify object created in step 3 still readable
    let read_result = kernel.execute_tool(
        "cas.read",
        &json!({"cid": created_cid}),
        &agent_id,
    );
    assert!(read_result.success);
    assert_eq!(read_result.output["data"].as_str().unwrap(), "v0.4 tool-created object");

    // 13. Tool describe works
    let desc_result = kernel.execute_tool(
        "tools.describe",
        &json!({"name": "cas.create"}),
        &agent_id,
    );
    assert!(desc_result.success);
    assert_eq!(desc_result.output["name"].as_str().unwrap(), "cas.create");

    // 14. Unknown tool returns error
    let unknown = kernel.execute_tool("nonexistent.tool", &json!({}), &agent_id);
    assert!(!unknown.success);
    assert!(unknown.error.unwrap().contains("unknown tool"));

    // 15. Restart kernel and verify persistence
    drop(kernel);
    let kernel2 = AIKernel::new(root.clone()).expect("kernel2 init");

    // Agent should be restored
    let status = kernel2.agent_status(&agent_id);
    assert!(status.is_some(), "agent should survive restart");

    // Tool registry should be repopulated
    assert!(kernel2.tool_count() >= 19, "tools should be re-registered after restart");

    // Working memories should survive restart
    let memories2 = kernel2.recall(&agent_id);
    let has_cognitive = memories2.iter().any(|m| {
        m.content.display().contains("important cognitive memory")
    });
    assert!(has_cognitive, "working memory should survive restart");
}

#[test]
fn test_search_index_persistence_roundtrip() {
    use plico::fs::{InMemoryBackend, SearchIndexMeta, SemanticSearch};

    let backend = InMemoryBackend::new();

    backend.upsert("cid-a", &vec![1.0, 0.0, 0.0], SearchIndexMeta {
        cid: "cid-a".into(),
        tags: vec!["tag1".into()],
        snippet: "hello world".into(),
        content_type: "text".into(),
        created_at: 1000,
    });
    backend.upsert("cid-b", &vec![0.0, 1.0, 0.0], SearchIndexMeta {
        cid: "cid-b".into(),
        tags: vec!["tag2".into()],
        snippet: "foo bar".into(),
        content_type: "text".into(),
        created_at: 2000,
    });

    let snapshot = backend.snapshot();
    assert_eq!(snapshot.len(), 2);

    // Serialize to JSONL
    let jsonl: String = snapshot.iter()
        .map(|e| serde_json::to_string(e).unwrap())
        .collect::<Vec<_>>()
        .join("\n");

    // Restore into new backend
    let backend2 = InMemoryBackend::new();
    let entries: Vec<plico::fs::SearchIndexEntry> = jsonl.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    backend2.restore(entries);

    assert_eq!(backend2.len(), 2);

    // Search should still work
    let hits = backend2.search(
        &vec![1.0, 0.0, 0.0],
        1,
        &plico::fs::SearchFilter::default(),
    );
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].cid, "cid-a");
    assert!((hits[0].score - 1.0).abs() < 1e-4);
}

/// v0.5 E2E: Intent resolution + resource enforcement + agent messaging.
#[test]
fn test_e2e_intent_resources_messaging() {
    let (kernel, _dir) = make_kernel();

    // 1. Register two agents with different resources
    let agent_a = kernel.register_agent("Agent-Alpha".to_string());
    let agent_b = kernel.register_agent("Agent-Beta".to_string());

    // 2. Set resources for Agent-Alpha: memory_quota=5, only cas.search allowed
    kernel.agent_set_resources(
        &agent_a, Some(5), None, Some(vec!["cas.search".to_string()])
    ).expect("set resources");

    // 3. Verify resource enforcement: tool allowlist blocks cas.create
    let blocked = kernel.execute_tool("cas.create", &serde_json::json!({"content":"test","tags":["t"]}), &agent_a);
    assert!(!blocked.success, "cas.create should be blocked for agent_a");
    assert!(blocked.error.as_ref().unwrap().contains("not in agent's allowed list"));

    // 4. Allowed tool works
    let allowed = kernel.execute_tool("cas.search", &serde_json::json!({"query":"test"}), &agent_a);
    assert!(allowed.success, "cas.search should work for agent_a");

    // 5. Memory quota enforcement: store 5 memories, then 6th should fail
    for i in 0..5 {
        let result = kernel.execute_tool(
            "memory.store",
            &serde_json::json!({"content": format!("mem-{}", i)}),
            &agent_a,
        );
        // memory.store is NOT in allowed_tools, so it should be blocked
        // Actually let's test quota on agent_b which has no tool restriction
        assert!(!result.success, "memory.store blocked by allowlist");
    }

    // Test quota enforcement on agent_b with memory_quota=3 via tool API
    kernel.agent_set_resources(&agent_b, Some(3), None, None).expect("set b resources");
    for i in 0..3 {
        kernel.remember(&agent_b, format!("memory-{}", i));
    }
    // 4th store should be rejected by quota (via tool call)
    let overflow_result = kernel.execute_tool(
        "memory.store",
        &serde_json::json!({"content": "overflow"}),
        &agent_b,
    );
    assert!(!overflow_result.success, "should exceed quota");
    assert!(overflow_result.error.as_ref().unwrap().contains("quota"),
        "error should mention quota: {:?}", overflow_result.error);

    // 6. Use intent router to resolve NL query
    let resolved = kernel.intent_resolve("search for agent scheduling documents", &agent_b);
    assert!(!resolved.is_empty(), "should resolve at least one intent");
    assert!(resolved[0].confidence >= 0.7, "confidence should be >= 0.7");

    // 7. Agent messaging: trusted agent "kernel" sends message to B
    let msg_id = kernel.send_message("kernel", &agent_b, serde_json::json!({"task": "summarize"}))
        .expect("send message (trusted agent)");
    assert!(!msg_id.is_empty());

    // B reads unread messages
    let msgs = kernel.read_messages(&agent_b, true);
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].from, "kernel");
    assert!(!msgs[0].read);

    // B acknowledges the message
    assert!(kernel.ack_message(&agent_b, &msg_id));
    let msgs_after = kernel.read_messages(&agent_b, true);
    assert_eq!(msgs_after.len(), 0, "no unread after ack");

    // All messages still accessible
    let all_msgs = kernel.read_messages(&agent_b, false);
    assert_eq!(all_msgs.len(), 1);
    assert!(all_msgs[0].read);
}

/// v0.5: Intent router resolves temporal phrases correctly.
#[test]
fn test_intent_router_temporal() {
    let (kernel, _dir) = make_kernel();
    let resolved = kernel.intent_resolve("find reports from last week", "test-agent");
    assert!(!resolved.is_empty());
    if let plico::api::semantic::ApiRequest::Search { since, until, .. } = &resolved[0].action {
        assert!(since.is_some(), "should have since timestamp");
        assert!(until.is_some(), "should have until timestamp");
    } else {
        panic!("Expected Search action, got {:?}", resolved[0].action);
    }
}

/// v0.5: Messaging permission enforcement — unauthorized sender blocked.
#[test]
fn test_messaging_permission_denied() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("Sender".to_string());
    let agent_b = kernel.register_agent("Receiver".to_string());

    // agent_a has no SendMessage grant — should fail
    let result = kernel.send_message(&agent_a, &agent_b, serde_json::json!("hello"));
    assert!(result.is_err(), "should be permission denied");
    assert!(result.unwrap_err().to_string().contains("permission"));
}
