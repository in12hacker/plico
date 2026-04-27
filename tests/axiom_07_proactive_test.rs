//! Axiom #7 Benchmark: "Proactive before reactive"
//!
//! Validates that the event bus delivers proactive notifications and that
//! the context assembly mechanism works for pre-warming agent context.

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
fn axiom7_event_subscription_delivers_notifications() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("proactive-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let sub_resp = kernel.handle_api_request(ApiRequest::EventSubscribe {
        agent_id: agent_id.clone(),
        event_types: None,
        agent_ids: None,
    });
    assert!(sub_resp.ok);
    let sub_id = sub_resp.subscription_id.expect("should return subscription_id");

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Proactive test data".to_string(),
        content_encoding: Default::default(),
        tags: vec!["proactive".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let poll_resp = kernel.handle_api_request(ApiRequest::EventPoll {
        subscription_id: sub_id,
    });
    assert!(poll_resp.ok);
    let events = poll_resp.kernel_events.unwrap_or_default();
    assert!(!events.is_empty(), "Should receive events after object creation");
}

#[test]
fn axiom7_context_assembly_prewarming() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("assembly-agent".into());
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

    for i in 0..3 {
        kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: format!("Document about Rust async pattern #{i}"),
            content_encoding: Default::default(),
            tags: vec!["rust".to_string(), "async".to_string()],
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
    }

    let assemble_resp = kernel.handle_api_request(ApiRequest::ContextAssemble {
        agent_id: agent_id.clone(),
        cids: vec![],
        budget_tokens: 4000,
    });
    assert!(assemble_resp.ok, "ContextAssemble failed: {:?}", assemble_resp.error);
}

#[test]
fn axiom7_declare_intent_triggers_prefetch() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("declare-agent".into());
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
        content: "Database optimization guide for PostgreSQL".to_string(),
        content_encoding: Default::default(),
        tags: vec!["database".to_string(), "optimization".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let declare_resp = kernel.handle_api_request(ApiRequest::DeclareIntent {
        agent_id: agent_id.clone(),
        intent: "optimize database queries".to_string(),
        related_cids: vec![],
        budget_tokens: 4000,
    });
    assert!(declare_resp.ok, "DeclareIntent failed: {:?}", declare_resp.error);
    assert!(declare_resp.assembly_id.is_some(), "DeclareIntent should return an assembly_id for prefetch");
}

#[test]
fn axiom7_proactive_latency_within_budget() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("latency-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let start = std::time::Instant::now();
    let sub_resp = kernel.handle_api_request(ApiRequest::EventSubscribe {
        agent_id: agent_id.clone(),
        event_types: None,
        agent_ids: None,
    });
    let latency = start.elapsed();
    assert!(sub_resp.ok);
    assert!(latency.as_millis() < 50, "EventSubscribe should be <50ms, got {}ms", latency.as_millis());
}
