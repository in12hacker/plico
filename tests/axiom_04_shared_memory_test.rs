//! Axiom #4 Benchmark: "Shared memory is collective intelligence"
//!
//! Validates that multiple agents can share knowledge through CAS and search,
//! and that memory stored by one agent is accessible to another.

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
fn axiom4_cross_agent_object_visibility() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("agent-alpha".into());
    let agent_b = kernel.register_agent("agent-beta".into());

    kernel.permission_grant(&agent_a, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, plico::api::permission::PermissionAction::ReadAny, None, None);

    let create_resp = kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Shared knowledge: Rust is memory safe".to_string(),
        content_encoding: Default::default(),
        tags: vec!["shared".to_string(), "rust".to_string()],
        agent_id: agent_a.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });
    assert!(create_resp.ok, "Agent A create failed: {:?}", create_resp.error);
    let cid = create_resp.cid.expect("should have CID");

    let read_resp = kernel.handle_api_request(ApiRequest::Read {
        cid: cid.clone(),
        agent_id: agent_b.clone(),
        tenant_id: None,
        agent_token: None,
    });
    assert!(read_resp.ok, "Agent B should be able to read shared object: {:?}", read_resp.error);
}

#[test]
fn axiom4_cross_agent_search_visibility() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("writer-agent".into());
    let agent_b = kernel.register_agent("reader-agent".into());

    kernel.permission_grant(&agent_a, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, plico::api::permission::PermissionAction::Read, None, None);

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Quantum computing uses qubits for parallel computation".to_string(),
        content_encoding: Default::default(),
        tags: vec!["quantum".to_string(), "shared".to_string()],
        agent_id: agent_a.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let search_resp = kernel.handle_api_request(ApiRequest::Search {
        query: "quantum".to_string(),
        agent_id: agent_b.clone(),
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
    assert!(search_resp.ok, "Agent B search failed: {:?}", search_resp.error);
    let results = search_resp.results.unwrap_or_default();
    assert!(!results.is_empty(), "Agent B should find agent A's content via search");
}

#[test]
fn axiom4_shared_memory_across_agents() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("mem-writer".into());
    let agent_b = kernel.register_agent("mem-reader".into());

    kernel.permission_grant(&agent_a, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, plico::api::permission::PermissionAction::Read, None, None);

    let remember_resp = kernel.handle_api_request(ApiRequest::Remember {
        agent_id: agent_a.clone(),
        content: "The project deadline is next Friday".to_string(),
        tenant_id: None,
    });
    assert!(remember_resp.ok, "Remember failed: {:?}", remember_resp.error);

    let recall_resp = kernel.handle_api_request(ApiRequest::Recall {
        agent_id: agent_b.clone(),
        scope: None,
        query: Some("deadline".to_string()),
        limit: Some(5),
        tier: None,
    });
    assert!(recall_resp.ok, "Recall failed: {:?}", recall_resp.error);
}

#[test]
fn axiom4_tag_based_object_sharing() {
    let (kernel, _dir) = make_kernel();
    let agent_a = kernel.register_agent("tag-writer".into());
    let agent_b = kernel.register_agent("tag-reader".into());

    kernel.permission_grant(&agent_a, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_b, plico::api::permission::PermissionAction::Read, None, None);

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Architecture decision: use microservices".to_string(),
        content_encoding: Default::default(),
        tags: vec!["adr".to_string(), "architecture".to_string()],
        agent_id: agent_a.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    // Agent B searches by tag — should find agent A's content
    let search_resp = kernel.handle_api_request(ApiRequest::Search {
        query: "architecture".to_string(),
        agent_id: agent_b.clone(),
        tenant_id: None,
        agent_token: None,
        limit: Some(5),
        offset: None,
        require_tags: vec!["adr".to_string()],
        exclude_tags: vec![],
        since: None,
        until: None,
        intent_context: None,
    });
    assert!(search_resp.ok);
    let results = search_resp.results.unwrap_or_default();
    assert!(!results.is_empty(), "Agent B should find agent A's ADR via tag search");
}
