//! Axiom #10 Benchmark: "Session mechanism — cognitive continuity"
//!
//! Validates the session lifecycle: start, active tracking, end, and that
//! session operations are fast and correct.

use plico::api::semantic::ApiRequest;
use plico::kernel::AIKernel;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let _ = std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn axiom10_session_start_and_end() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("session-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let start_resp = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: Some("code review session".to_string()),
        load_tiers: vec![],
        last_seen_seq: None,
    });
    assert!(start_resp.ok, "StartSession failed: {:?}", start_resp.error);
    let session_id = start_resp.session_started
        .as_ref()
        .map(|s| s.session_id.clone())
        .expect("should have session_started");

    let end_resp = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id,
        auto_checkpoint: true,
    });
    assert!(end_resp.ok, "EndSession failed: {:?}", end_resp.error);
    assert!(end_resp.session_ended.is_some(), "Should have session_ended result");
}

#[test]
fn axiom10_session_with_operations() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("ops-session-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Read,
        None, None,
    );

    let start_resp = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: None,
        load_tiers: vec![],
        last_seen_seq: None,
    });
    assert!(start_resp.ok);
    let session_id = start_resp.session_started
        .as_ref()
        .map(|s| s.session_id.clone())
        .expect("should have session_started");

    // Perform operations within the session
    let create_resp = kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "In-session knowledge artifact".to_string(),
        content_encoding: Default::default(),
        tags: vec!["session-scope".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });
    assert!(create_resp.ok);

    let search_resp = kernel.handle_api_request(ApiRequest::Search {
        query: "session knowledge".to_string(),
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        limit: Some(5),
        offset: None,
        require_tags: vec![],
        exclude_tags: vec![],
        since: None,
        until: None,
        intent_context: None,
    });
    assert!(search_resp.ok);

    let end_resp = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id,
        auto_checkpoint: true,
    });
    assert!(end_resp.ok);
}

#[test]
fn axiom10_session_continuity_via_seq() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("continuity-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    // Session 1
    let start1 = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: None,
        load_tiers: vec![],
        last_seen_seq: None,
    });
    assert!(start1.ok);
    let session_id_1 = start1.session_started.as_ref().unwrap().session_id.clone();

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "First session data".to_string(),
        content_encoding: Default::default(),
        tags: vec!["continuity".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let end1 = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id: session_id_1,
        auto_checkpoint: true,
    });
    assert!(end1.ok);
    let last_seq = end1.session_ended.as_ref().unwrap().last_seq;

    // Session 2 — should pick up from where session 1 left off
    let start2 = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: None,
        load_tiers: vec![],
        last_seen_seq: Some(last_seq),
    });
    assert!(start2.ok);
    let session_id_2 = start2.session_started.as_ref().unwrap().session_id.clone();

    let end2 = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id: session_id_2,
        auto_checkpoint: true,
    });
    assert!(end2.ok);
}

#[test]
fn axiom10_session_performance_within_budget() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("perf-session-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let start = std::time::Instant::now();
    let start_resp = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: None,
        load_tiers: vec![],
        last_seen_seq: None,
    });
    let start_latency = start.elapsed();
    assert!(start_resp.ok);
    assert!(start_latency.as_millis() < 200, "StartSession should be <200ms, got {}ms", start_latency.as_millis());

    let session_id = start_resp.session_started.as_ref().unwrap().session_id.clone();
    let end_start = std::time::Instant::now();
    let end_resp = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id,
        auto_checkpoint: true,
    });
    let end_latency = end_start.elapsed();
    assert!(end_resp.ok);
    assert!(end_latency.as_millis() < 200, "EndSession should be <200ms, got {}ms", end_latency.as_millis());
}
