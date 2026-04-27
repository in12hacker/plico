//! Axiom #9 Benchmark: "Learning never stops"
//!
//! Validates the learning loop: skill discovery works, procedural memory stores
//! reusable operations, and growth tracking captures agent evolution.

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
fn axiom9_skill_registration_and_discovery() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("skill-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let register_resp = kernel.handle_api_request(ApiRequest::RegisterSkill {
        agent_id: agent_id.clone(),
        name: "code_review".to_string(),
        description: "Performs thorough code reviews".to_string(),
        tags: vec!["code".to_string(), "review".to_string()],
    });
    assert!(register_resp.ok, "RegisterSkill failed: {:?}", register_resp.error);

    let discover_resp = kernel.handle_api_request(ApiRequest::DiscoverSkills {
        query: Some("code review".to_string()),
        agent_id_filter: None,
        tag_filter: None,
    });
    assert!(discover_resp.ok, "DiscoverSkills failed: {:?}", discover_resp.error);
    let skills = discover_resp.discovered_skills.unwrap_or_default();
    assert!(!skills.is_empty(), "Should find the registered skill");
}

#[test]
fn axiom9_procedural_memory_store_and_recall() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("proc-memory-agent".into());
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

    let store_resp = kernel.handle_api_request(ApiRequest::RememberProcedural {
        agent_id: agent_id.clone(),
        name: "deploy_service".to_string(),
        description: "Steps to deploy the production service".to_string(),
        steps: vec![
            plico::api::semantic::ProcedureStepDto {
                action: "build".to_string(),
                description: "Run cargo build --release".to_string(),
                expected_outcome: Some("Binary at target/release/".to_string()),
            },
            plico::api::semantic::ProcedureStepDto {
                action: "deploy".to_string(),
                description: "SCP binary to server".to_string(),
                expected_outcome: None,
            },
        ],
        learned_from: Some("manual observation".to_string()),
        tags: vec!["deploy".to_string()],
        scope: None,
    });
    assert!(store_resp.ok, "RememberProcedural failed: {:?}", store_resp.error);

    let recall_resp = kernel.handle_api_request(ApiRequest::RecallProcedural {
        agent_id: agent_id.clone(),
        name: Some("deploy_service".to_string()),
    });
    assert!(recall_resp.ok, "RecallProcedural failed: {:?}", recall_resp.error);
}

#[test]
fn axiom9_growth_report_after_session() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("growth-agent".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    let start_resp = kernel.handle_api_request(ApiRequest::StartSession {
        agent_id: agent_id.clone(),
        agent_token: None,
        intent_hint: Some("test learning growth".to_string()),
        load_tiers: vec![],
        last_seen_seq: None,
    });
    assert!(start_resp.ok, "StartSession failed: {:?}", start_resp.error);
    let session_id = start_resp.session_started
        .as_ref()
        .map(|s| s.session_id.clone())
        .expect("should have session_started");

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content: "Session learning artifact".to_string(),
        content_encoding: Default::default(),
        tags: vec!["learning".to_string()],
        agent_id: agent_id.clone(),
        tenant_id: None,
        agent_token: None,
        intent: None,
    });

    let end_resp = kernel.handle_api_request(ApiRequest::EndSession {
        agent_id: agent_id.clone(),
        session_id,
        auto_checkpoint: true,
    });
    assert!(end_resp.ok, "EndSession failed: {:?}", end_resp.error);

    let growth_resp = kernel.handle_api_request(ApiRequest::QueryGrowthReport {
        agent_id: agent_id.clone(),
        period: plico::api::semantic::GrowthPeriod::AllTime,
    });
    assert!(growth_resp.ok, "GrowthReport failed: {:?}", growth_resp.error);
}

#[test]
fn axiom9_intent_feedback_for_learning() {
    let (kernel, _dir) = make_kernel();
    let agent_id = kernel.register_agent("feedback-learner".into());
    kernel.permission_grant(
        &agent_id,
        plico::api::permission::PermissionAction::Write,
        None, None,
    );

    // First declare an intent to get an intent_id
    let declare_resp = kernel.handle_api_request(ApiRequest::DeclareIntent {
        agent_id: agent_id.clone(),
        intent: "fix async bug in handler".to_string(),
        related_cids: vec![],
        budget_tokens: 4000,
    });
    assert!(declare_resp.ok);
    let intent_id = declare_resp.assembly_id.unwrap_or_else(|| "test-intent".to_string());

    let feedback_resp = kernel.handle_api_request(ApiRequest::IntentFeedback {
        intent_id,
        used_cids: vec!["cid-1".to_string(), "cid-2".to_string()],
        unused_cids: vec!["cid-3".to_string()],
        agent_id: agent_id.clone(),
    });
    assert!(feedback_resp.ok, "IntentFeedback failed: {:?}", feedback_resp.error);
}
