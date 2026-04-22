//! AIKernel unit tests
//!
//! Tests cover: kernel creation, object CRUD, semantic operations,
//! agent registration, memory operations, permission enforcement,
//! and KernelExecutor integration.

use plico::kernel::AIKernel;
use plico::intent::{ChainRouter, IntentRouter};
use plico::intent::execution;
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
        .get_object(&cid, "TestAgent", "default")
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
        .semantic_read(&plico::fs::Query::ByCid(cid.clone()), "DevAgent", "default")
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
        .semantic_read(&plico::fs::Query::ByTags(vec!["a".to_string()]), "x", "default")
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
        .semantic_update(&old_cid, b"updated".to_vec(), None, "x", "default")
        .expect("update failed");

    // Content changed → new CID
    assert_ne!(new_cid, old_cid);

    // Old object still exists (immutable CAS)
    let old_obj = kernel.get_object(&old_cid, "x", "default").expect("old should exist");
    assert_eq!(old_obj.data, b"original");

    // New object has new content
    let new_obj = kernel.get_object(&new_cid, "x", "default").expect("new should exist");
    assert_eq!(new_obj.data, b"updated");
}

#[test]
fn test_kernel_delete_requires_permission() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel
        .semantic_create(b"secret".to_vec(), vec!["private".to_string()], "x", None)
        .expect("create failed");

    // 'cli' agent has no Delete grant by default
    let result = kernel.semantic_delete(&cid, "cli", "default");
    assert!(result.is_err(), "delete should fail without permission");

    // Object still readable by owner (isolation prevents 'cli' from reading 'x' data)
    let obj = kernel.get_object(&cid, "x", "default").expect("should still exist");
    assert_eq!(obj.data, b"secret");

    // 'cli' cannot read 'x' data (ownership isolation)
    let result = kernel.get_object(&cid, "cli", "default");
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
    let obj_a = kernel.get_object(&cid_a, "agent-a", "default").expect("A reads own");
    assert_eq!(obj_a.data, b"agent A data");

    // A cannot read B's object
    let result = kernel.get_object(&cid_b, "agent-a", "default");
    assert!(result.is_err(), "A should not read B's object");

    // B can read own object
    let obj_b = kernel.get_object(&cid_b, "agent-b", "default").expect("B reads own");
    assert_eq!(obj_b.data, b"agent B data");

    // Trusted "kernel" can read both
    let obj_a2 = kernel.get_object(&cid_a, "kernel", "default").expect("kernel reads A");
    assert_eq!(obj_a2.data, b"agent A data");

    // A search only returns own objects
    let results = kernel.semantic_read(
        &plico::fs::Query::ByTags(vec!["shared-tag".to_string()]),
        "agent-a",
        "default",
    ).expect("read by tags");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].meta.created_by, "agent-a");

    // kernel search returns both
    let results = kernel.semantic_read(
        &plico::fs::Query::ByTags(vec!["shared-tag".to_string()]),
        "kernel",
        "default",
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

    kernel.remember("agent1", "default", "Remember to check the logs".to_string()).unwrap();
    let memories = kernel.recall("agent1", "default");

    assert!(!memories.is_empty());
    assert!(memories.iter().any(|m| m.content.display().contains("logs")));
}

#[test]
fn test_kernel_forget_ephemeral() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "default", "Temporary note".to_string()).unwrap();
    assert!(!kernel.recall("agent1", "default").is_empty());

    kernel.forget_ephemeral("agent1");
    let memories = kernel.recall("agent1", "default");

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

// Node 15 F-4: Memory tier unit tests

#[test]
fn test_kernel_remember_working_tier() {
    let (kernel, _dir) = make_kernel();

    kernel.remember_working("agent1", "default", "working memory content".to_string(), vec!["test".to_string()]).unwrap();
    let memories = kernel.recall("agent1", "default");

    let working: Vec<_> = memories.iter().filter(|m| matches!(m.tier, plico::memory::MemoryTier::Working)).collect();
    assert!(!working.is_empty(), "should have at least one working tier memory");
    assert!(working.iter().any(|m| m.content.display().contains("working memory")));
}

#[test]
fn test_kernel_remember_longterm_tier() {
    let (kernel, _dir) = make_kernel();

    kernel.remember_long_term("agent1", "default", "important fact".to_string(), vec!["fact".to_string()], 50).unwrap();
    let memories = kernel.recall("agent1", "default");

    let longterm: Vec<_> = memories.iter().filter(|m| matches!(m.tier, plico::memory::MemoryTier::LongTerm)).collect();
    assert!(!longterm.is_empty(), "should have at least one longterm tier memory");
    assert!(longterm.iter().any(|m| m.content.display().contains("important fact")));
}

#[test]
fn test_kernel_remember_procedural_tier() {
    let (kernel, _dir) = make_kernel();

    let steps = vec![plico::memory::layered::ProcedureStep {
        step_number: 0,
        description: "test procedure".to_string(),
        action: "do the test".to_string(),
        expected_outcome: "test passes".to_string(),
    }];
    kernel.remember_procedural(
        "agent1", "default",
        "test-procedure".to_string(),
        "A test procedure".to_string(),
        steps,
        "unit-test".to_string(),
        vec!["test".to_string()],
    ).unwrap();

    let entries = kernel.recall_procedural("agent1", "default", Some("test-procedure"));
    assert!(!entries.is_empty(), "should recall procedural memory by name");
    assert!(entries.iter().all(|e| matches!(&e.content, plico::memory::MemoryContent::Procedure(_))));
}

#[test]
fn test_kernel_recall_procedural_no_filter() {
    let (kernel, _dir) = make_kernel();

    // Store two different procedures
    let steps1 = vec![plico::memory::layered::ProcedureStep {
        step_number: 0, description: "proc1".to_string(), action: "action1".to_string(), expected_outcome: String::new(),
    }];
    kernel.remember_procedural("agent1", "default", "proc1".to_string(), "description1".to_string(), steps1, "test".to_string(), vec![]).unwrap();

    let steps2 = vec![plico::memory::layered::ProcedureStep {
        step_number: 0, description: "proc2".to_string(), action: "action2".to_string(), expected_outcome: String::new(),
    }];
    kernel.remember_procedural("agent1", "default", "proc2".to_string(), "description2".to_string(), steps2, "test".to_string(), vec![]).unwrap();

    // recall_procedural with None returns ALL procedural memories
    let all = kernel.recall_procedural("agent1", "default", None);
    assert_eq!(all.len(), 2, "should recall all two procedural memories");
}

#[test]
fn test_kernel_memory_move() {
    let (kernel, _dir) = make_kernel();

    // Store in working
    kernel.remember_working("agent1", "default", "movable content".to_string(), vec![]).unwrap();
    let memories_before = kernel.recall("agent1", "default");
    let entry_id = memories_before.iter().find(|m| m.content.display().contains("movable content")).map(|m| m.id.clone());

    if let Some(eid) = entry_id {
        // Move to longterm
        let moved = kernel.memory_move("agent1", "default", &eid, plico::memory::MemoryTier::LongTerm);
        assert!(moved, "memory_move should succeed");

        let memories_after = kernel.recall("agent1", "default");
        let moved_entry = memories_after.iter().find(|m| m.id == eid);
        assert!(moved_entry.map(|m| matches!(m.tier, plico::memory::MemoryTier::LongTerm)).unwrap_or(false));
    }
}

#[test]
fn test_kernel_memory_stats() {
    let (kernel, _dir) = make_kernel();

    kernel.remember("agent1", "default", "ephemeral".to_string()).unwrap();
    kernel.remember_working("agent1", "default", "working".to_string(), vec![]).unwrap();
    kernel.remember_long_term("agent1", "default", "longterm".to_string(), vec![], 50).unwrap();

    let stats = kernel.memory_stats("agent1", None);
    assert!(stats.total_entries >= 3, "should have at least 3 entries across tiers");
}

// Node 15 F-4: Agent ops unit tests

#[test]
fn test_kernel_resolve_agent_by_name() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("resolver-test-agent".to_string());
    assert!(!id.is_empty());

    // Resolve by name
    let resolved = kernel.resolve_agent("resolver-test-agent");
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap(), id);
}

#[test]
fn test_kernel_resolve_agent_by_uuid() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("uuid-test-agent".to_string());

    // Resolve by UUID directly
    let resolved = kernel.resolve_agent(&id);
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap(), id);
}

#[test]
fn test_kernel_resolve_agent_not_found() {
    let (kernel, _dir) = make_kernel();

    let resolved = kernel.resolve_agent("nonexistent-agent-xyz");
    assert!(resolved.is_none());
}

#[test]
fn test_kernel_agent_status() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("status-test".to_string());
    let status = kernel.agent_status(&id);

    assert!(status.is_some());
    let (ret_id, _state, pending) = status.unwrap();
    assert_eq!(ret_id, id);
    assert_eq!(pending, 0);
}

#[test]
fn test_kernel_agent_suspend_resume() {
    let (kernel, _dir) = make_kernel();

    let id = kernel.register_agent("suspend-test".to_string());

    kernel.agent_suspend(&id).expect("suspend should succeed");
    let status = kernel.agent_status(&id).unwrap();
    assert!(status.1.contains("Suspended"), "agent should be Suspended");

    kernel.agent_resume(&id).expect("resume should succeed");
    let status = kernel.agent_status(&id).unwrap();
    assert!(status.1.contains("Waiting"), "agent should be Waiting after resume");
}

#[test]
fn test_kernel_checkpoint_and_restore() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("checkpoint-agent".to_string());
    kernel.remember_working(&agent_id, "default", "before checkpoint".to_string(), vec![]).unwrap();

    let cid = kernel.checkpoint_agent(&agent_id).expect("checkpoint should succeed");
    assert!(!cid.is_empty());

    // Verify checkpoint CID is a valid CAS object
    let obj = kernel.get_object(&cid, &agent_id, "default");
    assert!(obj.is_ok(), "checkpoint CID should be retrievable");
}

#[test]
fn test_kernel_register_skill() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("skill-host".to_string());
    let node_id = kernel.register_skill(&agent_id, "test-skill", "A test skill", vec!["test".to_string()]);

    assert!(node_id.is_ok());
    let node_id = node_id.unwrap();
    assert!(!node_id.is_empty());
}

#[test]
fn test_kernel_discover_skills() {
    let (kernel, _dir) = make_kernel();

    let agent_id = kernel.register_agent("skill-discoverer".to_string());
    kernel.register_skill(&agent_id, "skill-a", "Description A", vec!["tag1".to_string()]).unwrap();
    kernel.register_skill(&agent_id, "skill-b", "Description B", vec!["tag2".to_string()]).unwrap();

    let skills = kernel.discover_skills(None, Some(&agent_id), None);
    assert!(skills.len() >= 2, "should discover at least 2 skills");

    // Filter by query
    let filtered = kernel.discover_skills(Some("skill-a"), Some(&agent_id), None);
    assert!(!filtered.is_empty());
    assert!(filtered.iter().any(|s| s.name == "skill-a"));
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
    kernel.semantic_delete(&cid, "kernel", "default").expect("delete failed");

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

    kernel.semantic_delete(&cid, "kernel", "default").expect("delete failed");
    assert_eq!(kernel.list_deleted("kernel").len(), 1);

    kernel.restore_deleted(&cid, "kernel").expect("restore failed");

    // Should no longer appear in recycle bin
    assert!(kernel.list_deleted("kernel").is_empty());

    // Object should be searchable by tag again
    let results = kernel
        .semantic_read(&plico::fs::Query::ByTags(vec!["restore-test".to_string()]), "kernel", "default")
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
    ).unwrap();
    let (_, _, pending) = kernel.agent_status(&id).expect("status");
    assert_eq!(pending, 1);

    // submit_intent transitions Created→Waiting via assign_intent
    let (_, state, _) = kernel.agent_status(&id).expect("status");
    assert_eq!(state, "Waiting");

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
            api_version: None,
            content: "persisted intent test".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["intent-persist".to_string()],
            agent_id: "test".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        }).unwrap();
        kernel.submit_intent(
            IntentPriority::High,
            "test persistent intent".to_string(),
            Some(action),
            Some("test-agent".to_string()),
        ).unwrap();
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
        api_version: None,
        content: "Hello from KernelExecutor".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["test".to_string(), "executor".to_string()],
        agent_id: "test-agent".to_string(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    };
    let resp = kernel.handle_api_request(create_req);
    assert!(resp.ok);
    let cid = resp.cid.expect("should have cid");

    let read_req = ApiRequest::Read {
        cid: cid.clone(),
        agent_id: "test-agent".to_string(),
        tenant_id: None,
        agent_token: None,
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
        api_version: None,
        content: "Created via intent execution".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["intent-created".to_string()],
        agent_id: "executor-test-agent".to_string(),
        tenant_id: None,
        agent_token: None,
        intent: Some("test intent".to_string()),
    }).unwrap();

    let kernel_ref = Arc::clone(&kernel);
    let executor = KernelExecutor::new(move |json: &str, _agent_id: Option<&str>| {
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

    let obj = kernel.get_object(&cid, "executor-test-agent", "default").unwrap();
    assert_eq!(obj.data, b"Created via intent execution");
}

#[test]
fn test_kernel_executor_no_action_acknowledged() {
    use plico::scheduler::dispatch::{KernelExecutor, AgentExecutor};
    use plico::scheduler::agent::{Intent, IntentPriority};

    let executor = KernelExecutor::new(|_, _| "should not be called".to_string());
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
    let executor = KernelExecutor::new(move |json: &str, _agent_id: Option<&str>| {
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
        api_version: None,
        content: "Dispatch loop integration test".to_string(),
        content_encoding: ContentEncoding::Utf8,
        tags: vec!["dispatch-test".to_string()],
        agent_id: "dispatch-agent".to_string(),
        tenant_id: None,
        agent_token: None,
        intent: Some("test dispatch".to_string()),
    }).unwrap();

    kernel.submit_intent(
        IntentPriority::High,
        "dispatch test".to_string(),
        Some(action),
        Some("dispatch-agent".to_string()),
    ).unwrap();

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
            api_version: None,
            content: "E2E: agent-created data via intent execution".to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["e2e-test".to_string(), "agent-created".to_string()],
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            intent: Some("e2e autonomy test".to_string()),
        }).unwrap();

        let intent_id = kernel.submit_intent(
            IntentPriority::Critical,
            "Create test object".to_string(),
            Some(action),
            Some(agent_id.clone()),
        ).unwrap();
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

        let obj = kernel.get_object(&created_cid, &agent_id, "default").expect("get object");
        assert_eq!(obj.data, b"E2E: agent-created data via intent execution");
        assert_eq!(obj.meta.created_by, agent_id);

        let iso_result = kernel.get_object(&created_cid, "other-agent", "default");
        assert!(iso_result.is_err(), "other agent should not read this");

        kernel.remember_working(
            &agent_id,
            "default",
            "E2E test completed successfully".to_string(),
            vec!["e2e".to_string()],
        ).unwrap();

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

        let obj = kernel2.get_object(&created_cid, &agent_id, "default").expect("restored read");
        assert_eq!(obj.data, b"E2E: agent-created data via intent execution");

        let memories = kernel2.recall(&agent_id, "default");
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
        kernel.remember(&agent_id, "default", "frequently accessed memory".to_string()).unwrap();
    }
    // Use recall() which just reads; then use recall_relevant to trigger tracking
    for _ in 0..4 {
        let _ = kernel.recall_relevant(&agent_id, "default", 10000);
    }
    // After enough accesses, promotion should occur
    kernel.promote_check(&agent_id);
    let all_memories = kernel.recall(&agent_id, "default");
    let promoted = all_memories.iter().any(|m| {
        m.content.display().contains("frequently accessed memory")
            && m.tier == plico::memory::MemoryTier::Working
    });
    assert!(promoted, "expected ephemeral memory to be promoted to working, tiers: {:?}",
        all_memories.iter().map(|m| (m.tier, m.access_count, m.content.display())).collect::<Vec<_>>());

    // 8. Test recall_relevant with budget
    let relevant = kernel.recall_relevant(&agent_id, "default", 1000);
    assert!(!relevant.is_empty(), "recall_relevant should return memories");

    // 9. Transition agent to Waiting (required for suspend)
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "placeholder".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();

    // 9b. Suspend agent → verify context snapshot
    kernel.agent_suspend(&agent_id).expect("suspend failed");
    let all_after_suspend = kernel.recall(&agent_id, "default");
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
    let all_after_resume = kernel.recall(&agent_id, "default");
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
    let memories2 = kernel2.recall(&agent_id, "default");
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
        kernel.remember(&agent_b, "default", format!("memory-{}", i)).unwrap();
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

    // 6. Use intent router to resolve NL query (application-layer)
    let router = plico::intent::ChainRouter::new(None);
    let resolved = router.resolve("search for agent scheduling documents", &agent_b).unwrap();
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
    let (_kernel, _dir) = make_kernel();
    let router = ChainRouter::new(None);
    let resolved = router.resolve("find reports from last week", "test-agent").unwrap();
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

// ── v0.7 Tests: Graph CRUD, Scheduler Enforcement ──────────────────────

#[test]
fn test_kg_remove_edge_via_kernel() {
    let (kernel, _dir) = make_kernel();
    let agent = "kernel";
    let n1 = kernel.kg_add_node("Alice", plico::fs::KGNodeType::Entity, serde_json::Value::Null, agent, "default").unwrap();
    let n2 = kernel.kg_add_node("Bob", plico::fs::KGNodeType::Entity, serde_json::Value::Null, agent, "default").unwrap();
    kernel.kg_add_edge(&n1, &n2, plico::fs::KGEdgeType::RelatedTo, None, agent, "default").unwrap();

    let edges = kernel.kg_list_edges(agent, "default", Some(&n1)).unwrap();
    assert_eq!(edges.len(), 1);

    kernel.kg_remove_edge(&n1, &n2, Some(plico::fs::KGEdgeType::RelatedTo), agent, "default").unwrap();
    let edges_after = kernel.kg_list_edges(agent, "default", Some(&n1)).unwrap();
    assert_eq!(edges_after.len(), 0);
}

#[test]
fn test_kg_update_node_via_kernel() {
    let (kernel, _dir) = make_kernel();
    let agent = "kernel";
    let nid = kernel.kg_add_node("OldLabel", plico::fs::KGNodeType::Entity, serde_json::json!({"key":"v1"}), agent, "default").unwrap();

    kernel.kg_update_node(&nid, Some("NewLabel"), Some(serde_json::json!({"key":"v2","extra":true})), agent, "default").unwrap();

    let node = kernel.kg_get_node(&nid, agent, "default").unwrap().unwrap();
    assert_eq!(node.label, "NewLabel");
    assert_eq!(node.properties["key"], "v2");
    assert_eq!(node.properties["extra"], true);
}

#[test]
fn test_kg_remove_node_cascades_edges() {
    let (kernel, _dir) = make_kernel();
    let agent = "kernel";
    let n1 = kernel.kg_add_node("Center", plico::fs::KGNodeType::Entity, serde_json::Value::Null, agent, "default").unwrap();
    let n2 = kernel.kg_add_node("Leaf", plico::fs::KGNodeType::Entity, serde_json::Value::Null, agent, "default").unwrap();
    kernel.kg_add_edge(&n1, &n2, plico::fs::KGEdgeType::Mentions, None, agent, "default").unwrap();

    kernel.kg_remove_node(&n1, agent, "default").unwrap();

    let node = kernel.kg_get_node(&n1, agent, "default").unwrap();
    assert!(node.is_none());
    let edges = kernel.kg_list_edges(agent, "default", Some(&n1)).unwrap();
    assert_eq!(edges.len(), 0);
}

#[test]
fn test_agent_complete_sets_state() {
    let (kernel, _dir) = make_kernel();
    let id = kernel.register_agent("CompletableAgent".to_string());

    // Transition Created→Waiting via intent submission
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup".to_string(),
        None,
        Some(id.clone()),
    ).unwrap();

    kernel.agent_complete(&id).unwrap();
    let (_, state, _) = kernel.agent_status(&id).unwrap();
    assert_eq!(state, "Completed");
}

#[test]
fn test_completed_agent_rejects_intents() {
    let (kernel, _dir) = make_kernel();
    let id = kernel.register_agent("DoneAgent".to_string());

    // Transition Created→Waiting→Completed
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup".to_string(),
        None,
        Some(id.clone()),
    ).unwrap();
    kernel.agent_complete(&id).unwrap();

    let result = kernel.submit_intent(
        plico::scheduler::IntentPriority::High,
        "should fail".to_string(),
        None,
        Some(id.clone()),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("terminal state"));
}

#[test]
fn test_terminated_agent_rejects_intents() {
    let (kernel, _dir) = make_kernel();
    let id = kernel.register_agent("KilledAgent".to_string());
    kernel.agent_terminate(&id).unwrap();

    let result = kernel.submit_intent(
        plico::scheduler::IntentPriority::High,
        "should fail".to_string(),
        None,
        Some(id.clone()),
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("terminal state"));
}

#[test]
fn test_remember_respects_memory_quota() {
    let (kernel, _dir) = make_kernel();
    let id = kernel.register_agent("QuotaAgent".to_string());
    kernel.agent_set_resources(&id, Some(2), None, None).unwrap();

    kernel.remember(&id, "default", "first".to_string()).unwrap();
    kernel.remember(&id, "default", "second".to_string()).unwrap();
    let result = kernel.remember(&id, "default", "third".to_string());
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("quota") || err_msg.contains("Quota"));
}

#[test]
fn test_memory_tier_api_via_handle_request() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let id = kernel.register_agent("MemTierAgent".to_string());
    kernel.remember(&id, "default", "test entry".to_string()).unwrap();

    let memories = kernel.recall(&id, "default");
    let entry_id = memories[0].id.clone();

    let resp = kernel.handle_api_request(ApiRequest::MemoryMove {
        agent_id: id.clone(),
        entry_id: entry_id.clone(),
        target_tier: "working".to_string(),
        tenant_id: None,
    });
    assert!(resp.ok);

    let resp = kernel.handle_api_request(ApiRequest::MemoryDeleteEntry {
        agent_id: id.clone(),
        entry_id,
        tenant_id: None,
    });
    assert!(resp.ok);

    let resp = kernel.handle_api_request(ApiRequest::EvictExpired {
        agent_id: id.clone(),
        tenant_id: None,
    });
    assert!(resp.ok);
}

#[test]
fn test_v07_graph_scheduler_e2e_roundtrip() {
    let (kernel, _dir) = make_kernel();

    // 1. Register agent with memory_quota=5
    let agent_id = kernel.register_agent("v07-e2e-agent".to_string());
    kernel.agent_set_resources(&agent_id, Some(5), None, None).unwrap();

    // Use "kernel" (trusted) for operations requiring Delete permission
    let admin = "kernel";

    // 2. Add nodes + edges
    let n1 = kernel.kg_add_node("Project-X", plico::fs::KGNodeType::Entity, serde_json::json!({"status":"active"}), &agent_id, "default").unwrap();
    let n2 = kernel.kg_add_node("Meeting-2026-04-18", plico::fs::KGNodeType::Entity, serde_json::Value::Null, &agent_id, "default").unwrap();
    let n3 = kernel.kg_add_node("Decision-A", plico::fs::KGNodeType::Fact, serde_json::Value::Null, &agent_id, "default").unwrap();
    kernel.kg_add_edge(&n1, &n2, plico::fs::KGEdgeType::RelatedTo, Some(0.9), &agent_id, "default").unwrap();
    kernel.kg_add_edge(&n2, &n3, plico::fs::KGEdgeType::HasFact, Some(0.8), &agent_id, "default").unwrap();

    // 3. Update node
    kernel.kg_update_node(&n1, Some("Project-X-Updated"), Some(serde_json::json!({"status":"active","priority":"high"})), &agent_id, "default").unwrap();
    let updated = kernel.kg_get_node(&n1, &agent_id, "default").unwrap().unwrap();
    assert_eq!(updated.label, "Project-X-Updated");
    assert_eq!(updated.properties["priority"], "high");

    // 4. Verify edges (use agent_id who owns the nodes)
    let edges = kernel.kg_list_edges(&agent_id, "default", Some(&n1)).unwrap();
    assert_eq!(edges.len(), 1);

    // 5. Remove edge → verify (use admin for Delete permission)
    kernel.kg_remove_edge(&n1, &n2, Some(plico::fs::KGEdgeType::RelatedTo), admin, "default").unwrap();
    let edges_after = kernel.kg_list_edges(&agent_id, "default", Some(&n1)).unwrap();
    assert_eq!(edges_after.len(), 0);

    // 6. Remove node → verify cascade (use admin for Delete)
    kernel.kg_remove_node(&n2, admin, "default").unwrap();
    assert!(kernel.kg_get_node(&n2, &agent_id, "default").unwrap().is_none());

    // 7. Store memories up to quota → verify 6th rejected
    for i in 0..5 {
        kernel.remember(&agent_id, "default", format!("memory-{}", i)).unwrap();
    }
    let overflow = kernel.remember(&agent_id, "default", "overflow".to_string());
    assert!(overflow.is_err(), "6th memory should exceed quota=5");

    // 8. Complete agent → verify no more intents accepted
    // Need to transition Created→Waiting first (submit an intent)
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup for completion".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();
    kernel.agent_complete(&agent_id).unwrap();
    let (_, state, _) = kernel.agent_status(&agent_id).unwrap();
    assert_eq!(state, "Completed");

    let intent_result = kernel.submit_intent(
        plico::scheduler::IntentPriority::High,
        "post-completion intent".to_string(),
        None,
        Some(agent_id.clone()),
    );
    assert!(intent_result.is_err(), "completed agent should reject intents");
}

// ── v0.8 Pagination Tests ─────────────────────────────────────────────────────

#[test]
fn test_list_nodes_pagination() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("paginator".to_string());

    for i in 0..10 {
        let _ = kernel.kg_add_node(&format!("n{}", i), plico::fs::KGNodeType::Entity, serde_json::Value::Null, &agent_id, "default");
    }

    let resp = kernel.handle_api_request(ApiRequest::ListNodes {
        node_type: None, agent_id: agent_id.clone(), tenant_id: None, limit: Some(3), offset: Some(2),
    });
    assert!(resp.ok);
    assert_eq!(resp.nodes.as_ref().unwrap().len(), 3);
    assert_eq!(resp.total_count, Some(11));
    assert_eq!(resp.has_more, Some(true));

    let resp2 = kernel.handle_api_request(ApiRequest::ListNodes {
        node_type: None, agent_id: agent_id.clone(), tenant_id: None, limit: Some(3), offset: Some(9),
    });
    assert_eq!(resp2.nodes.as_ref().unwrap().len(), 2);
    assert_eq!(resp2.has_more, Some(false));
}

#[test]
fn test_list_edges_pagination() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("edge-pager".to_string());

    let mut src_ids = Vec::new();
    let mut dst_ids = Vec::new();
    for i in 0..5 {
        let sid = kernel.kg_add_node(&format!("s{}", i), plico::fs::KGNodeType::Entity, serde_json::Value::Null, &agent_id, "default").unwrap();
        let did = kernel.kg_add_node(&format!("d{}", i), plico::fs::KGNodeType::Entity, serde_json::Value::Null, &agent_id, "default").unwrap();
        kernel.kg_add_edge(&sid, &did, plico::fs::KGEdgeType::RelatedTo, Some(1.0), &agent_id, "default").unwrap();
        src_ids.push(sid);
        dst_ids.push(did);
    }

    let resp = kernel.handle_api_request(ApiRequest::ListEdges {
        agent_id: agent_id.clone(), tenant_id: None, node_id: None, limit: Some(2), offset: Some(1),
    });
    assert!(resp.ok);
    assert_eq!(resp.edges.as_ref().unwrap().len(), 2);
    assert_eq!(resp.total_count, Some(5));
    assert_eq!(resp.has_more, Some(true));
}

#[test]
fn test_pagination_beyond_total() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("beyond".to_string());

    for i in 0..3 {
        let _ = kernel.kg_add_node(&format!("x{}", i), plico::fs::KGNodeType::Entity, serde_json::Value::Null, &agent_id, "default");
    }

    let resp = kernel.handle_api_request(ApiRequest::ListNodes {
        node_type: None, agent_id: agent_id.clone(), tenant_id: None, limit: Some(10), offset: Some(100),
    });
    assert!(resp.ok);
    assert_eq!(resp.nodes.as_ref().unwrap().len(), 0);
    assert_eq!(resp.total_count, Some(4));
    assert_eq!(resp.has_more, Some(false));
}

// ── v0.8 Agent Lifecycle Tests ────────────────────────────────────────────────

#[test]
fn test_agent_fail_sets_state() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("fail-test".to_string());

    // Transition Created→Waiting before failing
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();

    kernel.agent_fail(&agent_id, "something went wrong").unwrap();

    let resp = kernel.handle_api_request(ApiRequest::AgentStatus { agent_id: agent_id.clone() });
    assert!(resp.ok);
    assert_eq!(resp.agent_state.as_deref(), Some("Failed"));
}

#[test]
fn test_agent_fail_via_api() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("api-fail-test".to_string());

    // Transition Created→Waiting before failing via API
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();

    let resp = kernel.handle_api_request(ApiRequest::AgentFail {
        agent_id: agent_id.clone(),
        reason: "resource exhaustion".to_string(),
    });
    assert!(resp.ok);

    let status = kernel.handle_api_request(ApiRequest::AgentStatus { agent_id: agent_id.clone() });
    assert_eq!(status.agent_state.as_deref(), Some("Failed"));
}

#[test]
fn test_agent_fail_already_terminal_rejected() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("double-fail".to_string());

    // Transition Created→Waiting before first fail
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "setup".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();

    kernel.agent_fail(&agent_id, "first failure").unwrap();
    let result = kernel.agent_fail(&agent_id, "second failure");
    assert!(result.is_err());
}

// ── v0.9 Context Loading Tests ───────────────────────────────────────────────

#[test]
fn test_context_load_l0() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("ctx-test".to_string());
    let cid = kernel.semantic_create(
        b"This is a test document with enough content to produce a meaningful L0 summary for testing purposes".to_vec(),
        vec!["test".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let loaded = kernel.context_load(&cid, plico::fs::ContextLayer::L0, &agent_id).unwrap();
    assert_eq!(loaded.cid, cid);
    assert_eq!(loaded.layer, plico::fs::ContextLayer::L0);
    assert!(loaded.tokens_estimate < 200, "L0 should be compact, got {} tokens", loaded.tokens_estimate);
}

#[test]
fn test_context_load_l2() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("ctx-l2-test".to_string());
    let content = "Full content for L2 test. This should be returned in its entirety.";
    let cid = kernel.semantic_create(
        content.as_bytes().to_vec(),
        vec!["test".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let loaded = kernel.context_load(&cid, plico::fs::ContextLayer::L2, &agent_id).unwrap();
    assert_eq!(loaded.content, content);
    assert_eq!(loaded.layer, plico::fs::ContextLayer::L2);
}

#[test]
fn test_context_load_invalid_layer() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let resp = kernel.handle_api_request(ApiRequest::LoadContext {
        cid: "nonexistent".to_string(),
        layer: "L3".to_string(),
        agent_id: "cli".to_string(),
        tenant_id: None,
    });
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("Invalid layer"));
}

#[test]
fn test_context_load_via_api() {
    use plico::api::semantic::ApiRequest;
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("ctx-api-test".to_string());
    let cid = kernel.semantic_create(
        b"API context loading roundtrip test content".to_vec(),
        vec!["test".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let resp = kernel.handle_api_request(ApiRequest::LoadContext {
        cid: cid.clone(),
        layer: "L2".to_string(),
        agent_id: agent_id.clone(),
        tenant_id: None,
    });
    assert!(resp.ok);
    let ctx = resp.context_data.unwrap();
    assert_eq!(ctx.cid, cid);
    assert_eq!(ctx.layer, "L2");
    assert!(ctx.content.contains("roundtrip test"));
}

#[test]
fn test_context_load_tool() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("ctx-tool-test".to_string());
    let cid = kernel.semantic_create(
        b"Tool-based context loading test".to_vec(),
        vec!["test".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let result = kernel.execute_tool(
        "context.load",
        &serde_json::json!({"cid": cid, "layer": "L0"}),
        &agent_id,
    );
    assert!(result.success, "context.load tool failed: {:?}", result.error);
    assert!(result.output["layer"].as_str() == Some("L0"));
}

// ─── M6: Synchronous Intent Execution ─────────────────────────────────────

#[test]
fn test_intent_execute_sync_stores_result_in_memory() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("sync-exec-test".to_string());
    let router = ChainRouter::new(None);

    kernel.semantic_create(
        b"test document for intent".to_vec(),
        vec!["test".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let result = execution::execute_sync(
        &kernel, &router,
        "search for test documents",
        &agent_id,
        0.0,
        false,
    );

    match result {
        Ok(r) => {
            assert!(!r.resolved.is_empty(), "should have resolved intents");
            if r.executed {
                let memories = kernel.recall(&agent_id, "default");
                let has_exec_memory = memories.iter().any(|m| {
                    m.tags.iter().any(|t| t == "execution-success" || t == "execution-failure")
                });
                assert!(has_exec_memory, "execution outcome should be stored in memory");
            }
        }
        Err(e) => {
            assert!(e.contains("resolution"), "unexpected error: {}", e);
        }
    }
}

#[test]
fn test_intent_execute_sync_below_threshold_not_executed() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("threshold-test".to_string());
    let router = ChainRouter::new(None);

    let result = execution::execute_sync(
        &kernel, &router,
        "do something vague",
        &agent_id,
        0.99,
        false,
    );

    match result {
        Ok(r) => {
            assert!(!r.executed, "should not execute below threshold");
        }
        Err(_) => {}
    }
}

#[test]
fn test_intent_execute_sync_learn_creates_procedural_memory() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("learn-test".to_string());
    let router = ChainRouter::new(None);

    kernel.semantic_create(
        b"searchable content".to_vec(),
        vec!["data".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let result = execution::execute_sync(
        &kernel, &router,
        "search for data",
        &agent_id,
        0.0,
        true,
    );

    if let Ok(r) = result {
        if r.executed && r.success {
            let proc_memories = kernel.recall_procedural(&agent_id, "default", None);
            let has_auto = proc_memories.iter().any(|m| {
                m.tags.iter().any(|t| t == "auto-learned")
            });
            assert!(has_auto, "learn=true + success should create procedural memory");
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_result_consumer_captures_dispatch_outcomes() {
    use plico::scheduler::agent::IntentPriority;
    use plico::api::permission::PermissionAction;

    let (kernel, _dir) = make_kernel_arc();

    let agent_id = kernel.register_agent("consumer-test".to_string());
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    let dispatch = kernel.start_dispatch_loop();
    let _consumer = kernel.start_result_consumer(&dispatch);

    kernel.submit_intent(
        IntentPriority::Medium,
        "test dispatch result".to_string(),
        None,
        Some(agent_id.clone()),
    ).unwrap();

    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

    dispatch.shutdown();

    let events = kernel.list_events(None, None, &["dispatch".to_string()], None, None);
    assert!(!events.is_empty(), "result consumer should emit dispatch events. Found {} events", events.len());
}

// ─── M7: End-to-End Autonomous Loop ───────────────────────────────────────

#[test]
fn test_e2e_autonomous_loop_resolve_execute_learn_reuse() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("e2e-loop".to_string());
    let router = ChainRouter::new(None);

    kernel.semantic_create(
        b"quarterly sales report Q1 2026".to_vec(),
        vec!["report".to_string(), "sales".to_string()],
        &agent_id,
        None,
    ).unwrap();

    // Step 1: Execute with learn=true → stores procedural memory
    let result = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_id,
        0.0,
        true,
    ).expect("sync execution should succeed");

    assert!(!result.resolved.is_empty(), "should resolve at least one intent");
    assert!(result.executed, "should execute the action");

    // Step 2: Verify execution result stored in working memory
    let memories = kernel.recall(&agent_id, "default");
    let has_exec_result = memories.iter().any(|m| {
        m.tags.iter().any(|t| t.contains("execution"))
    });
    assert!(has_exec_result, "execution outcome should be in memory");

    // Step 3: Verify procedural memory was learned
    let proc_memories = kernel.recall_procedural(&agent_id, "default", None);
    let auto_learned = proc_memories.iter().find(|m| {
        m.tags.iter().any(|t| t == "auto-learned")
    });

    if result.success {
        assert!(auto_learned.is_some(), "successful execution with learn=true should create procedural memory");

        // Verify it has the "verified" tag (indicating execution-verified learning)
        let verified = proc_memories.iter().any(|m| {
            m.tags.iter().any(|t| t == "verified")
        });
        assert!(verified, "auto-learned procedure should have 'verified' tag");

        // Step 4: Execute same intent again → should REUSE learned action
        let result2 = execution::execute_sync(
            &kernel, &router,
            "search for report",
            &agent_id,
            0.0,
            false,
        ).expect("second execution should succeed");

        assert!(result2.executed, "reuse should execute");
        assert!(result2.success, "reused action should succeed");

        let has_reuse = result2.resolved.iter().any(|r| {
            r.explanation.contains("[reused]")
        });
        assert!(has_reuse, "second execution should reuse learned action, got: {:?}",
            result2.resolved.iter().map(|r| &r.explanation).collect::<Vec<_>>());

        // Step 5: Verify reuse is tracked in memory
        let memories_after = kernel.recall(&agent_id, "default");
        let has_reused_tag = memories_after.iter().any(|m| {
            m.tags.iter().any(|t| t == "reused")
        });
        assert!(has_reused_tag, "reused execution should be tagged in memory");
    }
}

// ─── M8: Version Chain & Rollback ──────────────────────────────────

#[test]
fn test_version_history_returns_chain() {
    let (kernel, _dir) = make_kernel();

    let cid1 = kernel.semantic_create(
        b"version one".to_vec(),
        vec!["doc".to_string()],
        "cli",
        None,
    ).unwrap();

    let cid2 = kernel.semantic_update(
        &cid1,
        b"version two".to_vec(),
        None,
        "cli",
        "default",
    ).unwrap();

    let cid3 = kernel.semantic_update(
        &cid2,
        b"version three".to_vec(),
        None,
        "cli",
        "default",
    ).unwrap();

    let history = kernel.version_history(&cid3, "cli");
    assert!(history.len() >= 2, "history should have at least 2 entries, got {}: {:?}", history.len(), history);
    assert_eq!(history[0], cid3, "first entry should be the current CID");
}

#[test]
fn test_version_history_single_version() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel.semantic_create(
        b"only version".to_vec(),
        vec!["solo".to_string()],
        "cli",
        None,
    ).unwrap();

    let history = kernel.version_history(&cid, "cli");
    assert_eq!(history, vec![cid.clone()], "single version should return just itself");
}

#[test]
fn test_rollback_restores_previous_content() {
    let (kernel, _dir) = make_kernel();

    let cid1 = kernel.semantic_create(
        b"original content".to_vec(),
        vec!["doc".to_string(), "important".to_string()],
        "cli",
        None,
    ).unwrap();

    let cid2 = kernel.semantic_update(
        &cid1,
        b"modified content".to_vec(),
        Some(vec!["doc".to_string(), "changed".to_string()]),
        "cli",
        "default",
    ).unwrap();

    let obj_before = kernel.get_object(&cid2, "cli", "default").unwrap();
    assert_eq!(String::from_utf8_lossy(&obj_before.data), "modified content");

    let restored_cid = kernel.rollback(&cid2, "cli").expect("rollback should succeed");

    let restored_obj = kernel.get_object(&restored_cid, "cli", "default").unwrap();
    assert_eq!(
        String::from_utf8_lossy(&restored_obj.data),
        "original content",
        "rollback should restore previous version's content"
    );
}

#[test]
fn test_rollback_no_previous_version_fails() {
    let (kernel, _dir) = make_kernel();

    let cid = kernel.semantic_create(
        b"first and only".to_vec(),
        vec!["doc".to_string()],
        "cli",
        None,
    ).unwrap();

    let result = kernel.rollback(&cid, "cli");
    assert!(result.is_err(), "rollback with no previous version should fail");
    assert!(result.unwrap_err().contains("No previous version"), "error should mention no previous version");
}

#[test]
fn test_history_and_rollback_via_api() {
    let (kernel, _dir) = make_kernel();

    let cid1 = kernel.semantic_create(
        b"api version one".to_vec(),
        vec!["api-test".to_string()],
        "cli",
        None,
    ).unwrap();

    let cid2 = kernel.semantic_update(
        &cid1,
        b"api version two".to_vec(),
        None,
        "cli",
        "default",
    ).unwrap();

    // Test History API
    let history_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::History {
        cid: cid2.clone(),
        agent_id: "cli".to_string(),
    });
    assert!(history_resp.ok, "history API should succeed");
    assert!(history_resp.data.is_some(), "history should return data");

    // Test Rollback API
    let rollback_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::Rollback {
        cid: cid2.clone(),
        agent_id: "cli".to_string(),
    });
    assert!(rollback_resp.ok, "rollback API should succeed");
    assert!(rollback_resp.cid.is_some(), "rollback should return new CID");
}

// ─── M9: Multi-Step Workflow Execution ─────────────────────────────

#[test]
fn test_multi_step_intent_execution() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("multi-step".to_string());
    let router = ChainRouter::new(None);

    // "create a note and then search for notes" should resolve to 2 actions
    let result = execution::execute_sync(
        &kernel, &router,
        "save 'hello world' with tags note then search for note",
        &agent_id,
        0.0,
        false,
    ).expect("multi-step should succeed");

    assert!(result.executed, "multi-step should execute");
    // Should resolve into multiple actions (create + search)
    assert!(result.resolved.len() >= 1, "should have at least one resolved intent");
}

#[test]
fn test_multi_step_learn_creates_multi_step_procedure() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("multi-learn".to_string());
    let router = ChainRouter::new(None);

    // First create some data so the search part succeeds
    kernel.semantic_create(
        b"meeting notes for Q1".to_vec(),
        vec!["meeting".to_string(), "notes".to_string()],
        &agent_id,
        None,
    ).unwrap();

    // Execute with learn — conjunctive intent
    let result = execution::execute_sync(
        &kernel, &router,
        "remember 'check Q1 notes' and search for meeting",
        &agent_id,
        0.0,
        true,
    ).expect("learn workflow should succeed");

    assert!(result.executed);

    if result.success && result.resolved.len() > 1 {
        // Verify procedural memory has multiple steps
        let procedures = kernel.recall_procedural(&agent_id, "default", None);
        let multi_proc = procedures.iter().find(|p| {
            if let plico::memory::MemoryContent::Procedure(ref proc) = p.content {
                proc.steps.len() > 1
            } else {
                false
            }
        });
        assert!(multi_proc.is_some(), "learned workflow should have multiple steps");
    }
}

#[test]
fn test_multi_step_reuse_replays_all_steps() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("multi-reuse".to_string());
    let router = ChainRouter::new(None);

    kernel.semantic_create(
        b"project alpha status report".to_vec(),
        vec!["report".to_string(), "alpha".to_string()],
        &agent_id,
        None,
    ).unwrap();

    // First execution with learn
    let result1 = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_id,
        0.0,
        true,
    ).expect("first execution should succeed");

    assert!(result1.executed);

    if result1.success {
        // Second execution — should reuse
        let result2 = execution::execute_sync(
            &kernel, &router,
            "search for report",
            &agent_id,
            0.0,
            false,
        ).expect("reuse should succeed");

        assert!(result2.executed, "reuse should execute");
        assert!(result2.success, "reused workflow should succeed");
        let has_reuse = result2.resolved.iter().any(|r| r.explanation.contains("[reused]"));
        assert!(has_reuse, "should indicate reuse in explanation");
    }
}

#[test]
fn test_recall_procedural_api_returns_full_structure() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("recall-proc-test".to_string());

    let store_req = plico::api::semantic::ApiRequest::RememberProcedural {
        agent_id: agent_id.clone(),
        name: "deploy-module".to_string(),
        description: "Deploy a module to production".to_string(),
        steps: vec![
            plico::api::semantic::ProcedureStepDto {
                description: "Run tests".to_string(),
                action: "cargo test".to_string(),
                expected_outcome: Some("All tests pass".to_string()),
            },
            plico::api::semantic::ProcedureStepDto {
                description: "Build release".to_string(),
                action: "cargo build --release".to_string(),
                expected_outcome: Some("Binary produced".to_string()),
            },
        ],
        learned_from: Some("manual".to_string()),
        tags: vec!["deploy".to_string()],
        scope: None,
    };
    let resp = kernel.handle_api_request(store_req);
    assert!(resp.ok, "store should succeed: {:?}", resp.error);

    let recall_req = plico::api::semantic::ApiRequest::RecallProcedural {
        agent_id: agent_id.clone(),
        name: Some("deploy-module".to_string()),
    };
    let resp = kernel.handle_api_request(recall_req);
    assert!(resp.ok);
    let data: Vec<serde_json::Value> = serde_json::from_str(resp.data.as_ref().unwrap()).unwrap();
    assert_eq!(data.len(), 1);

    let proc = &data[0];
    assert_eq!(proc["name"], "deploy-module");
    assert_eq!(proc["description"], "Deploy a module to production");
    assert_eq!(proc["learned_from"], "manual");

    let steps = proc["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["step_number"], 1);
    assert_eq!(steps[0]["action"], "cargo test");
    assert_eq!(steps[0]["expected_outcome"], "All tests pass");
    assert_eq!(steps[1]["step_number"], 2);
    assert_eq!(steps[1]["action"], "cargo build --release");
}

// ─── MemoryScope E2E Tests ──────────────────────────────────────────

#[test]
fn test_memory_scope_private_isolation() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);

    kernel.remember_working(&agent_a, "default", "secret data".to_string(), vec!["private".into()]).unwrap();

    let a_memories = kernel.recall(&agent_a, "default");
    assert_eq!(a_memories.len(), 1, "agent-a sees own private memory");

    let b_memories = kernel.recall(&agent_b, "default");
    assert_eq!(b_memories.len(), 0, "agent-b does NOT see agent-a's private memory");
}

#[test]
fn test_memory_scope_shared_cross_agent() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    use plico::memory::MemoryScope;
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);

    kernel.remember_long_term_scoped(
        &agent_a,
        "default",
        "shared company knowledge".to_string(),
        vec!["company".into()],
        80,
        MemoryScope::Shared,
    ).unwrap();

    kernel.remember_long_term(
        &agent_a,
        "default",
        "agent-a private note".to_string(),
        vec!["private".into()],
        50,
    ).unwrap();

    let visible_a = kernel.recall_visible(&agent_a, "default", &[]);
    let visible_b = kernel.recall_visible(&agent_b, "default", &[]);

    assert_eq!(visible_a.len(), 2, "agent-a sees both private + shared");
    assert_eq!(visible_b.len(), 1, "agent-b only sees shared");
    assert!(visible_b[0].content.display().contains("shared company knowledge"));
}

#[test]
fn test_memory_scope_group_visibility() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());
    let agent_c = kernel.register_agent("agent-c".to_string());

    use plico::api::permission::PermissionAction;
    use plico::memory::MemoryScope;
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_c, PermissionAction::Read, None, None);

    kernel.remember_working_scoped(
        &agent_a,
        "default",
        "engineering standup notes".to_string(),
        vec!["standup".into()],
        MemoryScope::Group("engineering".into()),
    ).unwrap();

    let visible_a = kernel.recall_visible(&agent_a, "default", &[]);
    let visible_b = kernel.recall_visible(&agent_b, "default", &["engineering".into()]);
    let visible_c = kernel.recall_visible(&agent_c, "default", &["marketing".into()]);

    assert_eq!(visible_a.len(), 1, "owner sees own group memory");
    assert_eq!(visible_b.len(), 1, "engineering member sees group memory");
    assert_eq!(visible_c.len(), 0, "marketing member does NOT see engineering group memory");
}

#[test]
fn test_shared_procedural_memory_cross_agent() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("agent-a".to_string());
    let agent_b = kernel.register_agent("agent-b".to_string());

    use plico::api::permission::PermissionAction;
    use plico::memory::MemoryScope;
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);

    kernel.remember_procedural_scoped(
        &agent_a,
        "default",
        "deploy-workflow".to_string(),
        "Standard deploy procedure".to_string(),
        vec![plico::memory::layered::ProcedureStep {
            step_number: 1,
            description: "Run tests".to_string(),
            action: "cargo test".to_string(),
            expected_outcome: "pass".to_string(),
        }],
        "learned from experience".to_string(),
        vec!["deploy".into(), "verified".into()],
        MemoryScope::Shared,
    ).unwrap();

    let a_procs = kernel.recall_procedural(&agent_a, "default", None);
    assert_eq!(a_procs.len(), 1, "agent-a sees own procedural");

    let shared = kernel.recall_shared_procedural(None);
    assert_eq!(shared.len(), 1, "shared procedural is discoverable");
    assert!(shared[0].content.display().contains("deploy"));

    let b_visible = kernel.recall_visible(&agent_b, "default", &[]);
    assert_eq!(b_visible.len(), 1, "agent-b sees shared procedural via recall_visible");
}

#[test]
fn test_memory_scope_api_round_trip() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("scope-api-test".to_string());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::RememberLongTerm {
        agent_id: agent_id.clone(),
        content: "shared via API".to_string(),
        tags: vec!["api-test".into()],
        importance: 70,
        scope: Some("shared".to_string()),
        tenant_id: None,
    });
    assert!(resp.ok, "shared remember should succeed");

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::RecallVisible {
        agent_id: "some-other-agent".to_string(),
        groups: vec![],
    });
    assert!(resp.ok);
    let memories = resp.memory.unwrap();
    assert_eq!(memories.len(), 1, "other agent sees shared memory via API");
    assert!(memories[0].contains("shared via API"));
}

// ─── v3.0-M4: Multi-Agent Knowledge Sharing ──────────────────────────

#[test]
fn test_cross_agent_workflow_reuse_via_shared_scope() {
    let (kernel, _dir) = make_kernel();
    let router = ChainRouter::new(None);

    let agent_a = kernel.register_agent("teacher-agent".to_string());
    let agent_b = kernel.register_agent("student-agent".to_string());

    use plico::api::permission::PermissionAction;
    use plico::memory::MemoryScope;
    for agent in [&agent_a, &agent_b] {
        kernel.permission_grant(agent, PermissionAction::Read, None, None);
        kernel.permission_grant(agent, PermissionAction::Write, None, None);
    }

    kernel.semantic_create(
        b"quarterly financial report for Q1 2026".to_vec(),
        vec!["report".to_string(), "finance".to_string()],
        &agent_a,
        None,
    ).unwrap();

    // Agent A learns a workflow with learn=true
    let result = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_a,
        0.0,
        true,
    ).expect("Agent A execute should succeed");
    assert!(result.executed && result.success, "Agent A should successfully learn");

    // Verify Agent A has the procedural memory (private scope by default)
    let a_procs = kernel.recall_procedural(&agent_a, "default", None);
    assert!(!a_procs.is_empty(), "Agent A should have learned procedure");

    // Agent B tries to reuse — should NOT find it (private scope)
    let result_b_private = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_b,
        0.0,
        false,
    ).expect("Agent B execute should succeed");
    let has_reuse_private = result_b_private.resolved.iter()
        .any(|r| r.explanation.contains("[reused]"));
    assert!(!has_reuse_private, "Agent B should NOT reuse private procedure");

    // Now Agent A shares the procedure
    let proc_entry = &a_procs[0];
    if let plico::memory::MemoryContent::Procedure(ref proc) = proc_entry.content {
        kernel.remember_procedural_scoped(
            &agent_a,
            "default",
            proc.name.clone(),
            proc.description.clone(),
            proc.steps.clone(),
            proc.learned_from.clone(),
            vec!["auto-learned".to_string(), "verified".to_string()],
            MemoryScope::Shared,
        ).unwrap();
    } else {
        panic!("Expected Procedure content");
    }

    // Agent B tries again — should find shared procedure
    let result_b_shared = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_b,
        0.0,
        false,
    ).expect("Agent B execute should succeed");
    let has_reuse_shared = result_b_shared.resolved.iter()
        .any(|r| r.explanation.contains("[reused]"));
    assert!(has_reuse_shared, "Agent B SHOULD reuse shared procedure from Agent A");
    assert!(result_b_shared.success, "Reused workflow should execute successfully");
}

// ─── v3.1-M1: Procedures as Tools ───────────────────────────────────

#[test]
fn test_shared_procedure_appears_as_tool() {
    let (kernel, _dir) = make_kernel_arc();
    let router = ChainRouter::new(None);

    let agent_a = kernel.register_agent("teacher".to_string());
    let agent_b = kernel.register_agent("student".to_string());

    use plico::api::permission::PermissionAction;
    use plico::memory::MemoryScope;
    kernel.permission_grant(&agent_a, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_a, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, PermissionAction::Read, None, None);

    // Store test data so search has something to find
    kernel.semantic_create(
        b"quarterly report data".to_vec(),
        vec!["report".into()],
        &agent_a,
        None,
    ).unwrap();

    // Agent A learns a workflow and shares it
    let result = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_a,
        0.0,
        true,
    ).unwrap();
    assert!(result.success);

    // Share the learned procedure
    let procs = kernel.recall_procedural(&agent_a, "default", None);
    assert!(!procs.is_empty());
    if let plico::memory::MemoryContent::Procedure(ref proc) = procs[0].content {
        kernel.remember_procedural_scoped(
            &agent_a,
            "default",
            proc.name.clone(),
            proc.description.clone(),
            proc.steps.clone(),
            proc.learned_from.clone(),
            vec!["auto-learned".into(), "verified".into()],
            MemoryScope::Shared,
        ).unwrap();
    }

    // Refresh procedure tools — shared procedures become tools
    let tool_names = kernel.refresh_procedure_tools();
    assert!(!tool_names.is_empty(), "should register at least one skill tool");

    // Agent B can discover the tool via API
    let list_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::ToolList {
        agent_id: agent_b.clone(),
    });
    assert!(list_resp.ok);
    let all_tools = list_resp.tools.unwrap();
    let skill_tools: Vec<_> = all_tools.iter()
        .filter(|t| t.name.starts_with("skill."))
        .collect();
    assert!(!skill_tools.is_empty(), "tool list should contain skill.* entries");
    assert!(
        skill_tools[0].description.contains(&agent_a),
        "tool description should mention the original agent ID"
    );

    // Agent B invokes the tool
    let tool_name = &skill_tools[0].name;
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::ToolCall {
        tool: tool_name.clone(),
        params: serde_json::json!({"agent_id": agent_b.clone()}),
        agent_id: agent_b.clone(),
    });
    assert!(resp.ok, "tool call should succeed: {:?}", resp.error);
}

// ─── v4.0-M1: Agent Checkpoint & Restore ──────────────────────────

#[test]
fn test_checkpoint_creates_cas_object() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("checkpointer".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store some memories
    kernel.remember_working(&agent_id, "default", "task in progress".into(), vec!["wip".into()]).unwrap();
    kernel.remember_long_term(&agent_id, "default", "important fact".into(), vec!["fact".into()], 80).unwrap();

    // Checkpoint
    let cid = kernel.checkpoint_agent(&agent_id).expect("checkpoint should succeed");
    assert!(!cid.is_empty(), "CID should be non-empty");

    // Verify CAS object exists
    let obj = kernel.get_object(&cid, &agent_id, "default").expect("should fetch checkpoint object");
    let entries: Vec<plico::memory::MemoryEntry> = serde_json::from_slice(&obj.data).unwrap();
    assert_eq!(entries.len(), 2, "checkpoint should contain 2 memory entries");
}

#[test]
fn test_restore_checkpoint_replaces_memory() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("restorer".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store initial state and checkpoint
    kernel.remember_working(&agent_id, "default", "original note".into(), vec!["v1".into()]).unwrap();
    let cid = kernel.checkpoint_agent(&agent_id).unwrap();

    // Modify memory (add more, simulating continued work)
    kernel.remember_working(&agent_id, "default", "new note after checkpoint".into(), vec!["v2".into()]).unwrap();
    kernel.remember_long_term(&agent_id, "default", "extra long-term".into(), vec!["v2".into()], 90).unwrap();

    // Verify memory grew
    let before_restore = kernel.recall(&agent_id, "default");
    assert!(before_restore.len() >= 3, "should have original + new entries");

    // Restore to checkpoint
    let restored = kernel.restore_agent_checkpoint(&agent_id, &cid).unwrap();
    assert_eq!(restored, 1, "should restore 1 entry from checkpoint");

    // Verify memory matches checkpoint state
    let after_restore = kernel.recall(&agent_id, "default");
    assert_eq!(after_restore.len(), 1, "should have exactly the checkpointed entries");
    assert!(
        after_restore.iter().any(|e| {
            if let plico::memory::MemoryContent::Text(t) = &e.content {
                t.contains("original note")
            } else { false }
        }),
        "restored memory should contain the original note"
    );
}

#[test]
fn test_checkpoint_deduplication() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("dedup-tester".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    kernel.remember_working(&agent_id, "default", "stable state".into(), vec!["stable".into()]).unwrap();

    // Two checkpoints of the same state should produce the same CID (CAS dedup)
    let cid1 = kernel.checkpoint_agent(&agent_id).unwrap();
    let cid2 = kernel.checkpoint_agent(&agent_id).unwrap();
    assert_eq!(cid1, cid2, "same memory state should produce same CID (content-addressed)");
}

#[test]
fn test_checkpoint_unknown_agent_fails() {
    let (kernel, _dir) = make_kernel_arc();
    let result = kernel.checkpoint_agent("nonexistent");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_restore_unknown_checkpoint_fails() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("orphan".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);

    let result = kernel.restore_agent_checkpoint(&agent_id, "nonexistent-cid");
    assert!(result.is_err());
}

#[test]
fn test_checkpoint_via_api() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("api-check".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    kernel.remember_working(&agent_id, "default", "api test data".into(), vec!["api".into()]).unwrap();

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentCheckpoint {
        agent_id: agent_id.clone(),
    });
    assert!(resp.ok, "API checkpoint should succeed: {:?}", resp.error);
    let cid = resp.data.unwrap();
    assert!(!cid.is_empty());

    // Restore via API
    let resp2 = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentRestore {
        agent_id: agent_id.clone(),
        checkpoint_cid: cid,
    });
    assert!(resp2.ok, "API restore should succeed: {:?}", resp2.error);
    assert!(resp2.data.unwrap().contains("1 entries restored"));
}

#[test]
fn test_suspend_auto_checkpoints() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("auto-cp".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Move to Running state so we can suspend
    kernel.handle_api_request(plico::api::semantic::ApiRequest::SubmitIntent {
        priority: "normal".into(),
        description: "test intent".into(),
        action: None,
        agent_id: agent_id.clone(),
    });

    kernel.remember_working(&agent_id, "default", "important data".into(), vec!["pre-suspend".into()]).unwrap();

    // Suspend — should auto-checkpoint
    kernel.agent_suspend(&agent_id).unwrap();

    // Look for checkpoint tag in the context snapshot
    let memories = kernel.recall(&agent_id, "default");
    let has_checkpoint_tag = memories.iter().any(|e| {
        e.tags.iter().any(|t| t.starts_with("checkpoint:"))
    });
    assert!(has_checkpoint_tag, "suspend should create a checkpoint CID tag on the context snapshot");
}

// ─── v4.1: Auto-restore on resume ─────────────────────────────────

#[test]
fn test_full_suspend_resume_cycle_preserves_memory() {
    let (kernel, _dir) = make_kernel_arc();
    let agent_id = kernel.register_agent("cycle-test".into());

    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);

    // Store working memory
    kernel.remember_working(&agent_id, "default", "important fact 1".into(), vec!["fact".into()]).unwrap();
    kernel.remember_working(&agent_id, "default", "important fact 2".into(), vec!["fact".into()]).unwrap();
    let pre_suspend_count = kernel.recall(&agent_id, "default").len();
    assert_eq!(pre_suspend_count, 2);

    // Move to Running so we can suspend
    kernel.handle_api_request(plico::api::semantic::ApiRequest::SubmitIntent {
        priority: "normal".into(),
        description: "work".into(),
        action: None,
        agent_id: agent_id.clone(),
    });

    // Suspend — auto-checkpoints, stores snapshot
    kernel.agent_suspend(&agent_id).unwrap();

    // Verify suspended state
    let (_, state, _) = kernel.agent_status(&agent_id).unwrap();
    assert_eq!(state, "Suspended");

    // Resume — should auto-restore from checkpoint + inject context
    kernel.agent_resume(&agent_id).unwrap();

    let (_, state, _) = kernel.agent_status(&agent_id).unwrap();
    assert_eq!(state, "Waiting");

    // Memory should contain the original facts (restored from checkpoint)
    // plus an ephemeral context summary
    let post_resume = kernel.recall(&agent_id, "default");
    let has_fact1 = post_resume.iter().any(|e| {
        if let plico::memory::MemoryContent::Text(t) = &e.content { t.contains("important fact 1") } else { false }
    });
    let has_fact2 = post_resume.iter().any(|e| {
        if let plico::memory::MemoryContent::Text(t) = &e.content { t.contains("important fact 2") } else { false }
    });
    let has_context = post_resume.iter().any(|e| {
        if let plico::memory::MemoryContent::Text(t) = &e.content { t.contains("Context restored") } else { false }
    });
    assert!(has_fact1, "should restore fact 1 from checkpoint");
    assert!(has_fact2, "should restore fact 2 from checkpoint");
    assert!(has_context, "should inject context summary");
}

// ── v5.0: Kernel Event Bus ─────────────────────────────────────────

#[test]
fn test_event_subscribe_and_poll_empty() {
    let (kernel, _dir) = make_kernel();
    let sub_id = kernel.event_subscribe();
    assert!(sub_id.starts_with("sub-"));
    let events = kernel.event_poll(&sub_id).unwrap();
    assert!(events.is_empty());
}

#[test]
fn test_event_bus_object_stored_notification() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("watcher".into());

    let sub_id = kernel.event_subscribe();

    kernel.semantic_create(
        b"test data".to_vec(),
        vec!["tag-a".into()],
        &agent,
        None,
    ).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    assert!(!events.is_empty(), "should receive ObjectStored event");
    let has_obj_stored = events.iter().any(|e| {
        matches!(e, plico::kernel::event_bus::KernelEvent::ObjectStored { tags, .. }
            if tags.contains(&"tag-a".to_string()))
    });
    assert!(has_obj_stored, "should have ObjectStored with correct tags");
}

#[test]
fn test_event_bus_agent_state_change_notification() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("lifecycle".into());

    // Move agent to Waiting state first (Created → Suspended is illegal)
    kernel.submit_intent(
        plico::scheduler::IntentPriority::Low,
        "activate".into(),
        None,
        Some(agent.clone()),
    ).unwrap();

    let sub_id = kernel.event_subscribe();

    kernel.agent_suspend(&agent).unwrap();
    kernel.agent_resume(&agent).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    let state_changes: Vec<_> = events.iter().filter(|e| {
        matches!(e, plico::kernel::event_bus::KernelEvent::AgentStateChanged { .. })
    }).collect();
    assert!(state_changes.len() >= 2, "should see suspend + resume state changes");
}

#[test]
fn test_event_bus_memory_stored_notification() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("learner".into());

    let sub_id = kernel.event_subscribe();

    kernel.remember_working(&agent, "default", "test fact".into(), vec!["tag".into()]).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    let has_mem_stored = events.iter().any(|e| {
        matches!(e, plico::kernel::event_bus::KernelEvent::MemoryStored { tier, .. }
            if tier == "working")
    });
    assert!(has_mem_stored, "should receive MemoryStored event for working tier");
}

#[test]
fn test_event_bus_cross_agent_reactive_workflow() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("producer".into());
    let _agent_b = kernel.register_agent("consumer".into());

    let sub_b = kernel.event_subscribe();

    kernel.semantic_create(
        b"shared knowledge".to_vec(),
        vec!["shared".into(), "knowledge".into()],
        &agent_a,
        None,
    ).unwrap();

    kernel.remember_working(&agent_a, "default", "learned something".into(), vec![]).unwrap();

    let events = kernel.event_poll(&sub_b).unwrap();
    assert!(events.len() >= 2, "consumer should see both producer events");

    let has_object = events.iter().any(|e| matches!(e, plico::kernel::event_bus::KernelEvent::ObjectStored { .. }));
    let has_memory = events.iter().any(|e| matches!(e, plico::kernel::event_bus::KernelEvent::MemoryStored { .. }));
    assert!(has_object, "consumer should see ObjectStored from producer");
    assert!(has_memory, "consumer should see MemoryStored from producer");
}

#[test]
fn test_event_bus_unsubscribe_stops_events() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("temp".into());

    let sub_id = kernel.event_subscribe();
    assert!(kernel.event_unsubscribe(&sub_id));

    kernel.semantic_create(b"data".to_vec(), vec![], &agent, None).unwrap();

    assert!(kernel.event_poll(&sub_id).is_none(), "unsubscribed should return None");
}

#[test]
fn test_event_bus_via_api() {
    use plico::api::semantic::ApiRequest;

    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("api-user".into());

    let resp = kernel.handle_api_request(ApiRequest::EventSubscribe { agent_id: agent.clone(), event_types: None, agent_ids: None });
    assert!(resp.ok);
    let sub_id = resp.subscription_id.unwrap();

    kernel.semantic_create(b"api test".to_vec(), vec!["api".into()], &agent, None).unwrap();

    let resp = kernel.handle_api_request(ApiRequest::EventPoll { subscription_id: sub_id.clone() });
    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();
    assert!(!events.is_empty(), "API poll should return pending events");

    let resp = kernel.handle_api_request(ApiRequest::EventUnsubscribe { subscription_id: sub_id });
    assert!(resp.ok);
}

#[test]
fn test_event_bus_intent_submitted_notification() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("intender".into());

    let sub_id = kernel.event_subscribe();

    kernel.submit_intent(
        plico::scheduler::IntentPriority::Medium,
        "test intent".into(),
        None,
        Some(agent.clone()),
    ).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    let has_intent = events.iter().any(|e| {
        matches!(e, plico::kernel::event_bus::KernelEvent::IntentSubmitted { .. })
    });
    assert!(has_intent, "should receive IntentSubmitted event");
}

#[test]
fn test_event_bus_filtered_subscribe_by_type() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("filter-test".into());

    let filter = plico::kernel::event_bus::EventFilter {
        event_types: Some(vec!["ObjectStored".into()]),
        agent_ids: None,
    };
    let sub_id = kernel.event_subscribe_filtered(Some(filter));

    let _ = kernel.remember_working_scoped(&agent, "default", "filter-noise".into(), vec![], plico::memory::MemoryScope::Private);
    kernel.semantic_create(b"filtered data".to_vec(), vec!["ft".into()], &agent, None).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    assert_eq!(events.len(), 1, "should only receive ObjectStored, not MemoryStored");
    assert!(matches!(&events[0], plico::kernel::event_bus::KernelEvent::ObjectStored { .. }));
}

#[test]
fn test_event_bus_filtered_subscribe_by_agent() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("producer-a".into());
    let agent_b = kernel.register_agent("producer-b".into());

    let filter = plico::kernel::event_bus::EventFilter {
        event_types: None,
        agent_ids: Some(vec![agent_b.clone()]),
    };
    let sub_id = kernel.event_subscribe_filtered(Some(filter));

    kernel.semantic_create(b"from A".to_vec(), vec![], &agent_a, None).unwrap();
    kernel.semantic_create(b"from B".to_vec(), vec![], &agent_b, None).unwrap();

    let events = kernel.event_poll(&sub_id).unwrap();
    assert_eq!(events.len(), 1, "should only see events from agent_b");
    match &events[0] {
        plico::kernel::event_bus::KernelEvent::ObjectStored { agent_id, .. } => {
            assert_eq!(agent_id, &agent_b);
        }
        other => panic!("unexpected event: {:?}", other),
    }
}

#[test]
fn test_event_bus_filtered_subscribe_via_api() {
    use plico::api::semantic::ApiRequest;

    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("api-filter".into());

    let resp = kernel.handle_api_request(ApiRequest::EventSubscribe {
        agent_id: agent.clone(),
        event_types: Some(vec!["MemoryStored".into()]),
        agent_ids: None,
    });
    assert!(resp.ok);
    let sub_id = resp.subscription_id.unwrap();

    kernel.semantic_create(b"noise".to_vec(), vec![], &agent, None).unwrap();
    let _ = kernel.remember_working_scoped(&agent, "default", "signal".into(), vec![], plico::memory::MemoryScope::Private);

    let resp = kernel.handle_api_request(ApiRequest::EventPoll { subscription_id: sub_id.clone() });
    assert!(resp.ok);
    let events = resp.kernel_events.unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(&events[0], plico::kernel::event_bus::KernelEvent::MemoryStored { .. }));

    kernel.handle_api_request(ApiRequest::EventUnsubscribe { subscription_id: sub_id });
}

#[test]
fn test_system_status_via_api() {
    use plico::api::semantic::ApiRequest;

    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("status-tester".into());
    kernel.semantic_create(b"status-data".to_vec(), vec!["test".into()], &agent, None).unwrap();

    let resp = kernel.handle_api_request(ApiRequest::SystemStatus);
    assert!(resp.ok);
    let status = resp.system_status.unwrap();
    assert!(status.agent_count >= 1);
    assert!(status.timestamp_ms > 0);
}

#[test]
fn test_context_assemble_within_budget() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("ctx-test".into());

    let cid1 = kernel.semantic_create(b"First document about Rust.".to_vec(), vec!["rust".into()], &agent, None).unwrap();
    let cid2 = kernel.semantic_create(b"Second document about Python.".to_vec(), vec!["python".into()], &agent, None).unwrap();

    let candidates = vec![
        plico::fs::context_budget::ContextCandidate { cid: cid1, relevance: 0.9 },
        plico::fs::context_budget::ContextCandidate { cid: cid2, relevance: 0.7 },
    ];

    let allocation = kernel.context_assemble(&candidates, 10000, &agent).unwrap();
    assert_eq!(allocation.candidates_included, 2);
    assert!(allocation.total_tokens <= allocation.budget);
    assert_eq!(allocation.candidates_considered, 2);
}

#[test]
fn test_context_assemble_via_api() {
    use plico::api::semantic::{ApiRequest, ContextAssembleCandidate};

    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("ctx-api".into());

    let cid = kernel.semantic_create(b"API test document.".to_vec(), vec![], &agent, None).unwrap();

    let resp = kernel.handle_api_request(ApiRequest::ContextAssemble {
        agent_id: agent.clone(),
        cids: vec![ContextAssembleCandidate { cid, relevance: 1.0 }],
        budget_tokens: 5000,
    });
    assert!(resp.ok);
    let assembly = resp.context_assembly.unwrap();
    assert_eq!(assembly.candidates_included, 1);
    assert!(assembly.total_tokens <= 5000);
}

#[test]
fn test_context_assemble_tight_budget_downgrades() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("ctx-tight".into());

    let big_content = "word ".repeat(2000);
    let cid = kernel.semantic_create(big_content.as_bytes().to_vec(), vec![], &agent, None).unwrap();

    let candidates = vec![
        plico::fs::context_budget::ContextCandidate { cid, relevance: 1.0 },
    ];

    // Budget too small for L2, should downgrade
    let allocation = kernel.context_assemble(&candidates, 50, &agent).unwrap();
    if allocation.candidates_included == 1 {
        assert_eq!(allocation.items[0].layer, plico::fs::ContextLayer::L0);
    }
    assert!(allocation.total_tokens <= 50);
}

// ── v6.1: Resource Visibility tests ─────────────────────────────────

#[test]
fn test_agent_usage_returns_defaults() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("usage-test".into());

    let usage = kernel.agent_usage(&agent).unwrap();
    assert_eq!(usage.agent_id, agent);
    assert_eq!(usage.memory_entries, 0);
    assert_eq!(usage.memory_quota, 0);
    assert_eq!(usage.tool_call_count, 0);
    assert_eq!(usage.cpu_time_quota, 0);
    assert!(usage.allowed_tools.is_empty());
}

#[test]
fn test_agent_usage_tracks_memory() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("mem-track".into());

    kernel.remember(&agent, "default", "fact one".into()).unwrap();
    kernel.remember(&agent, "default", "fact two".into()).unwrap();

    let usage = kernel.agent_usage(&agent).unwrap();
    assert_eq!(usage.memory_entries, 2);
}

#[test]
fn test_agent_usage_tracks_tool_calls() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("tool-track".into());

    kernel.execute_tool("cas.create", &serde_json::json!({
        "content": "test", "tags": ["t1"]
    }), &agent);

    let usage = kernel.agent_usage(&agent).unwrap();
    assert_eq!(usage.tool_call_count, 1);
}

#[test]
fn test_agent_usage_reflects_quotas() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("quota-reflect".into());

    kernel.agent_set_resources(&agent, Some(100), Some(5000), Some(vec!["cas.read".into()])).unwrap();

    let usage = kernel.agent_usage(&agent).unwrap();
    assert_eq!(usage.memory_quota, 100);
    assert_eq!(usage.cpu_time_quota, 5000);
    assert_eq!(usage.allowed_tools, vec!["cas.read"]);
}

#[test]
fn test_agent_usage_via_api() {
    let (kernel, _dir) = make_kernel();
    let agent = kernel.register_agent("api-usage".into());

    kernel.remember(&agent, "default", "data".into()).unwrap();

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentUsage {
        agent_id: agent.clone(),
    });
    assert!(resp.ok);
    let usage = resp.agent_usage.unwrap();
    assert_eq!(usage.agent_id, agent);
    assert_eq!(usage.memory_entries, 1);
}

#[test]
fn test_agent_usage_not_found() {
    let (kernel, _dir) = make_kernel();

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentUsage {
        agent_id: "nonexistent".into(),
    });
    assert!(!resp.ok);
    assert!(resp.error.unwrap().contains("not found"));
}

// ── v6.2: Agent Discovery tests ─────────────────────────────────────

#[test]
fn test_discover_agents_returns_all() {
    let (kernel, _dir) = make_kernel();
    kernel.register_agent("alice".into());
    kernel.register_agent("bob".into());

    let cards = kernel.discover_agents(None, None);
    assert_eq!(cards.len(), 2);
    let names: Vec<&str> = cards.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"alice"));
    assert!(names.contains(&"bob"));
}

#[test]
fn test_discover_agents_filter_by_state() {
    let (kernel, _dir) = make_kernel();
    let _a = kernel.register_agent("active-agent".into());
    let b = kernel.register_agent("terminated-agent".into());

    kernel.agent_terminate(&b).unwrap();

    let cards = kernel.discover_agents(Some("Created"), None);
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].name, "active-agent");
}

#[test]
fn test_discover_agents_filter_by_tool() {
    let (kernel, _dir) = make_kernel();
    let a = kernel.register_agent("cas-agent".into());
    kernel.agent_set_resources(&a, None, None, Some(vec!["cas.create".into(), "cas.read".into()])).unwrap();

    let _b = kernel.register_agent("mem-agent".into());

    let cards = kernel.discover_agents(None, Some("cas"));
    assert!(cards.iter().any(|c| c.name == "cas-agent"));
    // mem-agent has all tools (empty = all), which includes cas tools
    // so both should match
}

#[test]
fn test_discover_agents_via_api() {
    let (kernel, _dir) = make_kernel();
    let caller = kernel.register_agent("caller".into());
    kernel.register_agent("peer".into());

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DiscoverAgents {
        state_filter: None,
        tool_filter: None,
        agent_id: caller.clone(),
    });
    assert!(resp.ok);
    let cards = resp.agent_cards.unwrap();
    assert_eq!(cards.len(), 2);
}

#[test]
fn test_agent_card_includes_usage() {
    let (kernel, _dir) = make_kernel();
    let a = kernel.register_agent("tracked".into());
    kernel.remember(&a, "default", "test memory".into()).unwrap();
    kernel.execute_tool("cas.create", &serde_json::json!({"content": "x", "tags": ["t"]}), &a);

    let cards = kernel.discover_agents(None, None);
    let card = cards.iter().find(|c| c.name == "tracked").unwrap();
    assert_eq!(card.memory_entries, 1);
    assert_eq!(card.tool_call_count, 1);
}

// ── v6.3: Agent Delegation tests ────────────────────────────────────

#[test]
fn test_delegate_task_creates_intent_and_message() {
    let (kernel, _dir) = make_kernel();
    let alice = kernel.register_agent("alice".into());
    let bob = kernel.register_agent("bob".into());

    let (intent_id, msg_id) = kernel.delegate_task(
        &alice, &bob,
        "analyze PR #42".into(),
        None,
        plico::scheduler::IntentPriority::High,
    ).unwrap();

    assert!(!intent_id.is_empty());
    assert!(!msg_id.is_empty());

    let msgs = kernel.read_messages(&bob, true);
    assert_eq!(msgs.len(), 1);
    let payload = &msgs[0].payload;
    assert_eq!(payload["type"], "delegation");
    assert_eq!(payload["from"], alice);
    assert_eq!(payload["intent_id"], intent_id);
    assert_eq!(msgs[0].from, "kernel");
}

#[test]
fn test_delegate_task_rejects_terminal_agent() {
    let (kernel, _dir) = make_kernel();
    let alice = kernel.register_agent("alice".into());
    let bob = kernel.register_agent("bob".into());
    kernel.agent_terminate(&bob).unwrap();

    let result = kernel.delegate_task(
        &alice, &bob,
        "should fail".into(),
        None,
        plico::scheduler::IntentPriority::Medium,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("terminal"));
}

#[test]
fn test_delegate_task_rejects_unknown_agent() {
    let (kernel, _dir) = make_kernel();
    let alice = kernel.register_agent("alice".into());

    let result = kernel.delegate_task(
        &alice, "ghost",
        "should fail".into(),
        None,
        plico::scheduler::IntentPriority::Medium,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_delegate_task_via_api() {
    let (kernel, _dir) = make_kernel();
    let alice = kernel.register_agent("alice".into());
    let bob = kernel.register_agent("bob".into());

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DelegateTask {
        task_id: "task-1".into(),
        from_agent: alice.clone(),
        to_agent: bob.clone(),
        intent: "review code".into(),
        context_cids: vec![],
        deadline_ms: None,
    });
    assert!(resp.ok);
    let result = resp.task_result.unwrap();
    assert_eq!(result.task_id, "task-1");
    assert_eq!(result.agent_id, bob);
    assert!(matches!(result.status, plico::api::semantic::TaskStatus::Pending));
}

// ─── v7.0: Durable Event Store ──────────────────────────────────────

#[test]
fn test_event_history_records_agent_lifecycle() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("observer".into());

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None,
        agent_id_filter: Some(aid.clone()),
        limit: None,
    });
    assert!(resp.ok);
    let history = resp.event_history.unwrap();
    assert!(history.iter().any(|e| matches!(&e.event,
        plico::kernel::event_bus::KernelEvent::AgentStateChanged {
            agent_id, new_state, ..
        } if agent_id == &aid && new_state == "Waiting"
    )));
    assert!(history[0].seq > 0);
    assert!(history[0].timestamp_ms > 0);
}

#[test]
fn test_event_history_since_seq() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("a1".into());

    let baseline = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: None, limit: None,
    }).event_history.unwrap().len();

    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "hello".into(),
        content_encoding: Default::default(),
        tags: vec!["test".into()],
        agent_id: aid.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: Some(baseline as u64),
        agent_id_filter: None,
        limit: None,
    });
    let since = resp.event_history.unwrap();
    assert!(!since.is_empty());
    assert!(since.iter().any(|e| matches!(&e.event,
        plico::kernel::event_bus::KernelEvent::ObjectStored { .. }
    )));
}

#[test]
fn test_event_history_by_agent() {
    let (kernel, _dir) = make_kernel();
    let a1 = kernel.register_agent("alpha".into());
    let a2 = kernel.register_agent("beta".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "a1-data".into(),
        content_encoding: Default::default(),
        tags: vec!["t1".into()],
        agent_id: a1.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "a2-data".into(),
        content_encoding: Default::default(),
        tags: vec!["t2".into()],
        agent_id: a2.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let r1 = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: Some(a1.clone()), limit: None,
    });
    let r2 = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: Some(a2.clone()), limit: None,
    });

    let h1 = r1.event_history.unwrap();
    let h2 = r2.event_history.unwrap();
    assert!(h1.iter().all(|e| e.event.agent_id() == Some(a1.as_str())));
    assert!(h2.iter().all(|e| e.event.agent_id() == Some(a2.as_str())));
}

#[test]
fn test_event_history_via_api_full() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("api-agent".into());
    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "data".into(),
        content_encoding: Default::default(),
        tags: vec!["tag".into()],
        agent_id: aid.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: None, limit: None,
    });
    assert!(resp.ok);
    let history = resp.event_history.unwrap();
    assert!(!history.is_empty());

    let first_seq = history[0].seq;
    let resp2 = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: Some(first_seq), agent_id_filter: None, limit: None,
    });
    let h2 = resp2.event_history.unwrap();
    assert!(h2.iter().all(|e| e.seq > first_seq));
}

#[test]
fn test_event_history_api_agent_filter() {
    let (kernel, _dir) = make_kernel();
    let a1 = kernel.register_agent("filter-a".into());
    let a2 = kernel.register_agent("filter-b".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "d1".into(),
        content_encoding: Default::default(),
        tags: vec!["x".into()],
        agent_id: a1.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "d2".into(),
        content_encoding: Default::default(),
        tags: vec!["y".into()],
        agent_id: a2.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: Some(a1.clone()), limit: None,
    });
    let history = resp.event_history.unwrap();
    assert!(history.iter().all(|e| e.event.agent_id() == Some(a1.as_str())));
}

#[test]
fn test_event_history_api_limit() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("limiter".into());
    for i in 0..5 {
        kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
            api_version: None,
            content: format!("d{}", i),
            content_encoding: Default::default(),
            tags: vec!["t".into()],
            agent_id: aid.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
    }

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: None, limit: Some(3),
    });
    let history = resp.event_history.unwrap();
    assert_eq!(history.len(), 3);
}

#[test]
fn test_event_history_monotonic_sequence() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("mono".into());
    for i in 0..10 {
        kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
            api_version: None,
            content: format!("obj{}", i),
            content_encoding: Default::default(),
            tags: vec!["t".into()],
            agent_id: aid.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
    }

    let all = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
        since_seq: None, agent_id_filter: None, limit: None,
    }).event_history.unwrap();
    for window in all.windows(2) {
        assert!(window[1].seq > window[0].seq);
        assert!(window[1].timestamp_ms >= window[0].timestamp_ms);
    }
}

// ─── v7.0-M2: Event Log Persistence ─────────────────────────────────

#[test]
fn test_event_log_persists_across_restart() {
    let dir = tempdir().unwrap();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let event_count_before;
    {
        let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        let aid = kernel.register_agent("persist-test".into());
        kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
            api_version: None,
            content: "persist-data".into(),
            content_encoding: Default::default(),
            tags: vec!["persist".into()],
            agent_id: aid.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        kernel.persist_event_log();

        let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
            since_seq: None, agent_id_filter: None, limit: None,
        });
        event_count_before = resp.event_history.unwrap().len();
        assert!(event_count_before > 0);
    }

    {
        let kernel2 = AIKernel::new(dir.path().to_path_buf()).expect("kernel2 init");
        let resp = kernel2.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
            since_seq: None, agent_id_filter: None, limit: None,
        });
        let history = resp.event_history.unwrap();
        assert!(history.len() >= event_count_before,
            "restored event log should have at least {} events, got {}", event_count_before, history.len());
    }
}

#[test]
fn test_event_log_sequence_continues_after_restore() {
    let dir = tempdir().unwrap();
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");

    let max_seq_before;
    {
        let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        kernel.register_agent("seq-test".into());
        kernel.persist_event_log();

        let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
            since_seq: None, agent_id_filter: None, limit: None,
        });
        let history = resp.event_history.unwrap();
        max_seq_before = history.last().unwrap().seq;
    }

    {
        let kernel2 = AIKernel::new(dir.path().to_path_buf()).expect("kernel2 init");
        let aid = kernel2.register_agent("seq-test-2".into());
        kernel2.handle_api_request(plico::api::semantic::ApiRequest::Create {
            api_version: None,
            content: "new-data".into(),
            content_encoding: Default::default(),
            tags: vec!["new".into()],
            agent_id: aid,
            tenant_id: None,
            agent_token: None,
            intent: None,
        });

        let resp = kernel2.handle_api_request(plico::api::semantic::ApiRequest::EventHistory {
            since_seq: None, agent_id_filter: None, limit: None,
        });
        let history = resp.event_history.unwrap();
        let new_max = history.last().unwrap().seq;
        assert!(new_max > max_seq_before,
            "new events should have seq > {} but got {}", max_seq_before, new_max);
    }
}

#[test]
fn test_event_log_explicit_persist() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("explicit-persist".into());
    kernel.handle_api_request(plico::api::semantic::ApiRequest::Create {
        api_version: None,
        content: "test".into(),
        content_encoding: Default::default(),
        tags: vec!["t".into()],
        agent_id: aid,
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    kernel.persist_event_log();

    let path = _dir.path().join("event_log.jsonl");
    assert!(path.exists(), "event_log.jsonl should exist after persist");
    let json = std::fs::read_to_string(&path).unwrap();
    let events: Vec<plico::kernel::event_bus::SequencedEvent> = json.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(!events.is_empty());
}

// ─── v8.0: Agent Skill Registry ─────────────────────────────────────

#[test]
fn test_register_skill_creates_kg_node() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("skill-agent".into());

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "summarize".into(),
        description: "Summarize documents into concise bullet points".into(),
        tags: vec!["nlp".into(), "summarization".into()],
    });
    assert!(resp.ok);
    assert!(resp.node_id.is_some());
}

#[test]
fn test_register_skill_rejects_unknown_agent() {
    let (kernel, _dir) = make_kernel();

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: "nonexistent".into(),
        name: "skill".into(),
        description: "test".into(),
        tags: vec![],
    });
    assert!(!resp.ok);
}

#[test]
fn test_discover_skills_returns_registered() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("discoverable".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "translate".into(),
        description: "Translate text between languages".into(),
        tags: vec!["nlp".into(), "translation".into()],
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "summarize".into(),
        description: "Summarize documents".into(),
        tags: vec!["nlp".into()],
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DiscoverSkills {
        query: None,
        agent_id_filter: Some(aid.clone()),
        tag_filter: None,
    });
    assert!(resp.ok);
    let skills = resp.discovered_skills.unwrap();
    assert_eq!(skills.len(), 2);
    assert!(skills.iter().any(|s| s.name == "translate"));
    assert!(skills.iter().any(|s| s.name == "summarize"));
}

#[test]
fn test_discover_skills_by_query() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("query-agent".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "code-review".into(),
        description: "Review code for bugs and style issues".into(),
        tags: vec!["engineering".into()],
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "summarize".into(),
        description: "Summarize documents".into(),
        tags: vec!["nlp".into()],
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DiscoverSkills {
        query: Some("code".into()),
        agent_id_filter: None,
        tag_filter: None,
    });
    let skills = resp.discovered_skills.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "code-review");
}

#[test]
fn test_discover_skills_by_tag() {
    let (kernel, _dir) = make_kernel();
    let aid = kernel.register_agent("tag-agent".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "translate".into(),
        description: "Translate text".into(),
        tags: vec!["nlp".into()],
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: aid.clone(),
        name: "deploy".into(),
        description: "Deploy services".into(),
        tags: vec!["devops".into()],
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DiscoverSkills {
        query: None,
        agent_id_filter: None,
        tag_filter: Some("devops".into()),
    });
    let skills = resp.discovered_skills.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "deploy");
}

#[test]
fn test_discover_skills_cross_agent() {
    let (kernel, _dir) = make_kernel();
    let a1 = kernel.register_agent("agent-alpha".into());
    let a2 = kernel.register_agent("agent-beta".into());

    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: a1.clone(),
        name: "alpha-skill".into(),
        description: "Alpha capability".into(),
        tags: vec![],
    });
    kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id: a2.clone(),
        name: "beta-skill".into(),
        description: "Beta capability".into(),
        tags: vec![],
    });

    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DiscoverSkills {
        query: None,
        agent_id_filter: None,
        tag_filter: None,
    });
    let skills = resp.discovered_skills.unwrap();
    assert!(skills.len() >= 2);
    assert!(skills.iter().any(|s| s.name == "alpha-skill"));
    assert!(skills.iter().any(|s| s.name == "beta-skill"));
}
