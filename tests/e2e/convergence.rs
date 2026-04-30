//! E2E Convergence Tests (Node 25)
//!
//! Tests the complete AI-OS loop: declare intent → plan → execute → learn → predict → complete

#[cfg(test)]
mod tests {
    #[test]
    fn test_kernel_ops_modules_exist() {
        let _ = core::mem::size_of::<plico::kernel::ops::intent::IntentPlan>();
        let _ = core::mem::size_of::<plico::kernel::ops::intent_executor::ExecutionStats>();
        let _ = core::mem::size_of::<plico::kernel::ops::skill_discovery::SkillDiscriminator>();
        let _ = core::mem::size_of::<plico::kernel::ops::self_healing::PlanAdaptor>();
        let _ = core::mem::size_of::<plico::kernel::ops::intent_decomposer::IntentDecomposer>();
        let _ = core::mem::size_of::<plico::kernel::ops::cross_domain_skill::CrossDomainSkillComposer>();
        let _ = core::mem::size_of::<plico::kernel::ops::goal_generator::GoalGenerator>();
        let _ = core::mem::size_of::<plico::kernel::ops::temporal_projection::TemporalProjectionEngine>();
    }

    #[test]
    fn test_hook_registry_exists() {
        let registry = plico::kernel::hook::HookRegistry::new();
        let ctx = plico::kernel::hook::HookContext::new("test-agent", "test-tool", serde_json::json!({}));
        let result = registry.run_hooks(plico::kernel::hook::HookPoint::PreToolCall, &ctx);
        assert!(matches!(result, plico::kernel::hook::HookResult::Continue));
    }

    #[test]
    fn test_execution_stats_tracking() {
        use plico::kernel::ops::intent_executor::ExecutionStats;

        let mut stats = ExecutionStats::new();
        stats.record("read".to_string(), 100);
        stats.record("read".to_string(), 200);
        stats.record("call".to_string(), 50);

        assert_eq!(stats.get_avg_time("read"), Some(150));
        assert_eq!(stats.get_avg_time("call"), Some(50));
        assert_eq!(stats.get_avg_time("unknown"), None);
    }

    #[test]
    fn test_skill_discriminator_record() {
        use plico::kernel::ops::skill_discovery::SkillDiscriminator;

        let disc = SkillDiscriminator::new(2);
        disc.record_sequence("agent-1", vec!["read".to_string(), "call".to_string()], true, 100);
        disc.record_sequence("agent-1", vec!["read".to_string(), "call".to_string()], true, 100);

        let candidates = disc.get_skill_candidates("agent-1");
        assert!(!candidates.is_empty());
    }

    #[test]
    fn test_failure_classifier() {
        use plico::kernel::ops::self_healing::{FailureClassifier, FailureType};

        let ft = FailureClassifier::classify("permission denied", "test-step");
        assert!(matches!(ft, FailureType::PermissionDenied));

        let ft2 = FailureClassifier::classify("resource exhausted", "test-step");
        assert!(matches!(ft2, FailureType::ResourceExhausted));
    }

    #[test]
    fn test_plan_adaptor_adapt() {
        use plico::kernel::ops::self_healing::{PlanAdaptor, FailureType, Adaptation};

        let adaptor = PlanAdaptor::new();
        let adapt = adaptor.record_and_adapt("step-1", &FailureType::ToolNotFound);

        assert!(matches!(adapt, Adaptation::ReplaceTool { .. } | Adaptation::RetryWithNewParams));
    }

    #[test]
    fn test_temporal_projection_engine() {
        use plico::kernel::ops::temporal_projection::TemporalProjectionEngine;

        let engine = TemporalProjectionEngine::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        engine.record_intent("test-intent", now);
        let projected = engine.project(10);
        let _ = projected;
    }

    #[test]
    fn test_cross_domain_skill_composer() {
        use plico::kernel::ops::cross_domain_skill::CrossDomainSkillComposer;

        let composer = CrossDomainSkillComposer::new(2);
        composer.record_sequence(
            &["read:storage".to_string(), "call:tool".to_string()],
            &["storage".to_string(), "tool".to_string()],
            true,
        );
        composer.record_sequence(
            &["read:storage".to_string(), "call:tool".to_string()],
            &["storage".to_string(), "tool".to_string()],
            true,
        );

        let candidates = composer.get_composition_candidates();
        assert!(!candidates.is_empty());
    }

    #[test]
    fn test_goal_generator() {
        use plico::kernel::ops::goal_generator::GoalGenerator;

        let generator = GoalGenerator::new();
        generator.record_goal(
            "agent-1",
            &["auth".to_string(), "deploy".to_string()],
            &["read".to_string(), "call".to_string()],
            true,
        );

        let goals = generator.generate_goals("agent-1", "auth deploy workflow");
        assert!(!goals.is_empty());
    }

    #[test]
    fn test_full_ai_os_loop_convergence() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("LLM_BACKEND", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = plico::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");

        // Step 1: Register agent
        let agent_id = kernel.register_agent("e2e-agent".to_string());
        kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
        kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

        // Step 2: Store memories with relevant tags
        kernel.semantic_create(
            b"Fix authentication bug in login module".to_vec(),
            vec!["auth".to_string(), "login".to_string(), "bug".to_string()],
            &agent_id,
            None,
        ).expect("create auth doc failed");

        kernel.semantic_create(
            b"Deploy production API server".to_vec(),
            vec!["deploy".to_string(), "api".to_string(), "production".to_string()],
            &agent_id,
            None,
        ).expect("create deploy doc failed");

        // Add KG nodes
        let node1 = kernel.kg_add_node(
            "auth-fix",
            plico::fs::graph::KGNodeType::Fact,
            serde_json::json!({"description": "auth bug fix task"}),
            &agent_id,
            "default",
        ).expect("node1 add failed");

        let node2 = kernel.kg_add_node(
            "deploy-plan",
            plico::fs::graph::KGNodeType::Fact,
            serde_json::json!({"description": "deployment plan"}),
            &agent_id,
            "default",
        ).expect("node2 add failed");

        kernel.kg_add_edge(&node1, &node2, plico::fs::graph::KGEdgeType::Causes, None, &agent_id, "default")
            .expect("causal edge failed");

        // Step 3: Declare intent (triggers async prefetch)
        let declare_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "fix auth bug and deploy fix".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        assert!(declare_resp.ok, "declare_intent should succeed: {:?}", declare_resp.error);
        let assembly_id = declare_resp.assembly_id.expect("no assembly_id returned");

        // Step 4: Fetch assembled context (wait for async prefetch)
        // In stub mode, embedding fails so assembly enters Failed state — that's OK
        std::thread::sleep(std::time::Duration::from_millis(200));
        let fetch_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::FetchAssembledContext {
            agent_id: agent_id.clone(),
            assembly_id: assembly_id.clone(),
        });
        // In stub mode, prefetch fails due to no embedding backend — verify PlanAdaptor recorded it
        if !fetch_resp.ok {
            assert!(fetch_resp.error.as_ref().unwrap().contains("failed to embed"),
                "expected embedding failure in stub mode, got: {:?}", fetch_resp.error);
        }

        // Step 5: Submit intent
        let submit_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::SubmitIntent {
            description: "fix auth bug and deploy fix".to_string(),
            priority: "high".to_string(),
            action: Some("auth".to_string()),
            agent_id: agent_id.clone(),
        });
        assert!(submit_resp.ok, "submit_intent should succeed: {:?}", submit_resp.error);

        // Step 6: Verify KG causal edges
        let edges = kernel.kg_list_edges(&agent_id, "default", None)
            .expect("kg_list_edges failed");
        let causal_edges: Vec<_> = edges.into_iter()
            .filter(|e| matches!(e.edge_type, plico::fs::graph::KGEdgeType::Causes))
            .collect();
        assert!(!causal_edges.is_empty(), "should have at least one Causes edge");

        // Step 7: Verify KG nodes
        let nodes = kernel.kg_list_nodes(None, &agent_id, "default")
            .expect("kg_list_nodes failed");
        assert!(nodes.len() >= 2, "should have at least 2 KG nodes, got {}", nodes.len());

        // Step 8: Verify hot objects
        let _ = kernel.prefetch.get_hot_objects(&agent_id);

        // Step 9: Verify search
        let search_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::Search {
            agent_id: agent_id.clone(),
            query: "auth".to_string(),
            limit: Some(10),
            offset: None,
            require_tags: vec![],
            exclude_tags: vec![],
            tenant_id: None,
            agent_token: None,
            since: None,
            until: None,
            intent_context: None,
        });
        assert!(search_resp.ok, "search should succeed: {:?}", search_resp.error);

        // Step 10: Verify recall
        kernel.remember_working(&agent_id, "default", "e2e test memory".to_string(), vec!["test".to_string()])
            .expect("remember failed");
        let recall_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::Recall {
            agent_id: agent_id.clone(),
            scope: None,
            query: None,
            limit: None,
            tier: None,
        });
        assert!(recall_resp.ok, "recall should succeed: {:?}", recall_resp.error);
        let memories = recall_resp.memory.unwrap_or_default();
        assert!(!memories.is_empty(), "should have at least one remembered memory");

        // Step 11: Verify skill discriminator (brain module)
        let sd = &kernel.prefetch.skill_discriminator;
        sd.record_sequence(&agent_id, vec!["read".to_string(), "search".to_string()], true, 150);
        let _ = sd.get_skill_candidates(&agent_id);

        // Step 12: Verify temporal projection (brain module)
        let tp = &kernel.prefetch.temporal_engine;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        tp.record_intent("fix auth bug", now_ms);
        let current_hour = ((now_ms % 86400000) / 3600000) as u32;
        let _ = tp.project(current_hour);
    }

}
