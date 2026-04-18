//! Integration Demo: File Q&A Agent
//!
//! Soul Phase 3 verification: "编写一个简单的Demo智能体（如'文件问答机器人'），
//! 验证整个系统的可行性"
//!
//! This test simulates a full autonomous agent lifecycle:
//!   register → store docs → NL query → execute → learn → reuse →
//!   KG explore → version/rollback — all through the Plico kernel.

use plico::kernel::AIKernel;
use plico::intent::ChainRouter;
use plico::intent::execution;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn test_file_qa_agent_full_lifecycle() {
    let (kernel, _dir) = make_kernel();

    // ── Phase 1: Agent registration ─────────────────────────────
    let agent_id = kernel.register_agent("file-qa-bot".to_string());
    // Grant full permissions for the demo agent
    use plico::api::permission::PermissionAction;
    kernel.permission_grant(&agent_id, PermissionAction::Read, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::Delete, None, None);
    kernel.permission_grant(&agent_id, PermissionAction::SendMessage, None, None);

    let status = kernel.agent_status(&agent_id);
    assert!(status.is_some(), "agent should be registered");
    let (_, state, _) = status.unwrap();
    assert!(state.contains("Created") || state.contains("Running"), "new agent should be Created or Running, got: {}", state);

    // ── Phase 2: Store documents (semantic create) ──────────────
    let doc1 = kernel.semantic_create(
        b"Quarterly revenue report Q1 2026: total revenue $4.2M, up 15% YoY".to_vec(),
        vec!["report".to_string(), "finance".to_string(), "Q1".to_string()],
        &agent_id,
        Some("financial reporting".to_string()),
    ).unwrap();

    let doc2 = kernel.semantic_create(
        b"Engineering team headcount: 42 engineers, 3 managers, hiring 8 more in Q2".to_vec(),
        vec!["report".to_string(), "engineering".to_string(), "headcount".to_string()],
        &agent_id,
        None,
    ).unwrap();

    let doc3 = kernel.semantic_create(
        b"Product roadmap Q2 2026: launch v2.0, expand to APAC, hire 8 engineers".to_vec(),
        vec!["roadmap".to_string(), "product".to_string(), "Q2".to_string()],
        &agent_id,
        None,
    ).unwrap();

    // ── Phase 3: Memory operations ──────────────────────────────
    kernel.remember_working(
        &agent_id,
        "User is interested in Q1 financial data".to_string(),
        vec!["context".to_string(), "finance".to_string()],
    ).unwrap();

    kernel.remember_long_term(
        &agent_id,
        "Company fiscal year starts in January".to_string(),
        vec!["fact".to_string(), "finance".to_string()],
        5,
    ).unwrap();

    let memories = kernel.recall(&agent_id);
    assert!(memories.len() >= 2, "should have working + long-term memories");

    // ── Phase 4: NL intent → search (single action) ─────────────
    let router = ChainRouter::new(None);
    let result = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_id,
        0.0,
        true,
    ).expect("NL search should resolve and execute");

    assert!(result.executed, "search intent should execute");
    assert!(result.success, "search should succeed");
    assert!(!result.resolved.is_empty(), "should resolve at least one intent");

    // ── Phase 5: Reuse learned workflow ─────────────────────────
    let result2 = execution::execute_sync(
        &kernel, &router,
        "search for report",
        &agent_id,
        0.0,
        false,
    ).expect("reuse should work");

    assert!(result2.executed, "reuse should execute");
    assert!(result2.success, "reuse should succeed");
    let has_reuse = result2.resolved.iter().any(|r| r.explanation.contains("[reused]"));
    assert!(has_reuse, "second execution should reuse learned action");

    // ── Phase 6: Context loading (L0/L1/L2) ────────────────────
    let l0 = kernel.context_load(&doc1, plico::fs::ContextLayer::L0, &agent_id);
    assert!(l0.is_ok(), "L0 context load should succeed");
    let l0_ctx = l0.unwrap();
    assert!(l0_ctx.tokens_estimate <= 200, "L0 should be ~100 tokens");

    let l2 = kernel.context_load(&doc1, plico::fs::ContextLayer::L2, &agent_id);
    assert!(l2.is_ok(), "L2 context load should succeed");
    let l2_ctx = l2.unwrap();
    assert!(l2_ctx.content.contains("revenue"), "L2 should contain full content");

    // ── Phase 7: Knowledge graph exploration ────────────────────
    let tags = kernel.list_tags();
    assert!(tags.contains(&"report".to_string()), "should have 'report' tag");
    assert!(tags.contains(&"finance".to_string()), "should have 'finance' tag");

    let _kg_hits = kernel.graph_explore(&doc1, None, 1);

    // ── Phase 8: Version chain + rollback ───────────────────────
    let updated_cid = kernel.semantic_update(
        &doc1,
        b"CORRECTED: Quarterly revenue report Q1 2026: total revenue $4.5M, up 18% YoY".to_vec(),
        None,
        &agent_id,
    ).unwrap();

    let history = kernel.version_history(&updated_cid, &agent_id);
    assert!(history.len() >= 2, "should have version chain after update");
    assert_eq!(history[0], updated_cid, "first in chain is newest");

    let rolled_back = kernel.rollback(&updated_cid, &agent_id)
        .expect("rollback should succeed");
    let restored = kernel.get_object(&rolled_back, &agent_id).unwrap();
    assert!(
        String::from_utf8_lossy(&restored.data).contains("$4.2M"),
        "rollback should restore original $4.2M figure"
    );

    // ── Phase 9: Soft delete + restore ──────────────────────────
    kernel.semantic_delete(&doc3, &agent_id).unwrap();
    let deleted = kernel.list_deleted(&agent_id);
    assert!(!deleted.is_empty(), "recycle bin should have deleted doc");

    kernel.restore_deleted(&doc3, &agent_id).unwrap();
    let after_restore = kernel.list_deleted(&agent_id);
    assert!(
        after_restore.iter().all(|d| d.cid != doc3),
        "restored doc should no longer be in recycle bin"
    );

    // ── Phase 10: Procedural memory verification ────────────────
    let procedures = kernel.recall_procedural(&agent_id, None);
    let has_auto_learned = procedures.iter().any(|p| {
        p.tags.iter().any(|t| t == "verified")
    });
    assert!(has_auto_learned, "should have verified procedural memory from learn cycle");

    // ── Phase 11: Permission isolation ──────────────────────────
    let other_agent = kernel.register_agent("other-agent".to_string());
    let isolated_read = kernel.get_object(&doc2, &other_agent);
    assert!(isolated_read.is_err(), "other agent should not read doc owned by file-qa-bot");

    // ── Phase 12: Agent lifecycle ───────────────────────────────
    // Use API to test lifecycle. Fresh agent + submit intent → Waiting → Suspend → Resume → Terminate
    let lifecycle_agent = kernel.register_agent("lifecycle-test".to_string());

    // Submit intent via API to move from Created → Waiting
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::SubmitIntent {
        description: "test lifecycle intent".to_string(),
        priority: "normal".to_string(),
        action: None,
        agent_id: lifecycle_agent.clone(),
    });
    assert!(resp.ok, "submit intent should succeed");

    // Suspend via API
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentSuspend {
        agent_id: lifecycle_agent.clone(),
    });
    assert!(resp.ok, "suspend should succeed: {:?}", resp.error);

    let (_, state, _) = kernel.agent_status(&lifecycle_agent).unwrap();
    assert!(state.contains("Suspended"), "should be suspended, got: {}", state);

    // Resume via API
    let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::AgentResume {
        agent_id: lifecycle_agent.clone(),
    });
    assert!(resp.ok, "resume should succeed: {:?}", resp.error);

    // Terminate
    kernel.agent_terminate(&lifecycle_agent).unwrap();
    let (_, state, _) = kernel.agent_status(&lifecycle_agent).unwrap();
    assert!(state.contains("Terminated"), "should be terminated, got: {}", state);

    // ── Phase 13: Messaging between agents ──────────────────────
    let agent_b = kernel.register_agent("analyst-bot".to_string());
    kernel.send_message(&agent_id, &agent_b, serde_json::json!("Please review Q1 report")).unwrap();

    let msgs = kernel.read_messages(&agent_b, true);
    assert_eq!(msgs.len(), 1, "analyst-bot should have 1 unread message");
    assert!(msgs[0].payload.to_string().contains("Q1"), "message should contain Q1");

    kernel.ack_message(&agent_b, &msgs[0].id);
    let unread = kernel.read_messages(&agent_b, true);
    assert!(unread.is_empty(), "after ack, no unread messages");
}
