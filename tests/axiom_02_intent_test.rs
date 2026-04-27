//! Axiom #2 Benchmark: "Intent drives everything"
//!
//! Validates that the intent system correctly submits intents with priorities,
//! and that intent-based operations work through the kernel API.

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
fn axiom2_submit_intent_with_priority() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("intent-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
        description: "Store a knowledge fact about Rust".to_string(),
        priority: "high".to_string(),
        action: None,
        agent_id: agent_id.clone(),
    });
    assert!(resp.ok, "SubmitIntent should succeed: {:?}", resp.error);
    assert!(resp.intent_id.is_some(), "Should return an intent_id");
}

#[test]
fn axiom2_submit_intent_default_priority() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("default-prio-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
        description: "Analyze codebase for security issues".to_string(),
        priority: "normal".to_string(),
        action: None,
        agent_id: agent_id.clone(),
    });
    assert!(resp.ok, "SubmitIntent with normal priority should succeed: {:?}", resp.error);
    assert!(resp.intent_id.is_some());
}

#[test]
fn axiom2_intent_drives_search() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("search-intent-agent".into());
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

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Rust async patterns and tokio runtime".to_string(),
        content_encoding: Default::default(),
        tags: vec!["rust".to_string(), "async".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: Some("study rust async".to_string()),
    });

    let resp = kernel.handle_api_request(ApiRequest::Search {
        query: "async".to_string(),
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        limit: Some(5),
        offset: None,
        require_tags: vec![],
        exclude_tags: vec![],
        since: None,
        until: None,
        intent_context: Some("fix async bug".to_string()),
    });
    assert!(resp.ok, "Search with intent context should succeed: {:?}", resp.error);
}

#[test]
fn axiom2_batch_submit_intents() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("batch-intent-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let resp = kernel.handle_api_request(ApiRequest::BatchSubmitIntent {
        intents: vec![
            plico::api::semantic::IntentSpec {
                description: "First intent".to_string(),
                priority: "high".to_string(),
                action: None,
            },
            plico::api::semantic::IntentSpec {
                description: "Second intent".to_string(),
                priority: "normal".to_string(),
                action: None,
            },
        ],
        agent_id: agent_id.clone(),
    });
    assert!(resp.ok, "BatchSubmitIntent should succeed: {:?}", resp.error);
}
