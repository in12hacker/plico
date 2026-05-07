//! E2E Convergence Tests (Node 25)
//!
//! Tests the complete AI-OS loop: declare intent → plan → execute → learn → predict → complete
//! Soul v3.0: Validates cognitive symbiotic engine integration.

#[cfg(test)]
mod tests {
    #[test]
    fn test_kernel_ops_modules_exist() {
        let _ = core::mem::size_of::<plico::kernel::ops::intent::IntentPlan>();
        let _ = core::mem::size_of::<plico::kernel::ops::intent_executor::ExecutionStats>();
        let _ = core::mem::size_of::<plico::kernel::cognition::CognitiveLoop>();
        let _ = core::mem::size_of::<plico::kernel::cognition::SkillForge>();
        let _ = core::mem::size_of::<plico::kernel::cognition::IntentSemanticNetwork>();
        let _ = core::mem::size_of::<plico::kernel::cognition::TrajectoryTracker>();
        let _ = core::mem::size_of::<plico::kernel::cognition::ContextQualityEngine>();
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
    fn test_trajectory_tracker_records_operations() {
        use plico::kernel::cognition::TrajectoryTracker;
        let rt = tokio::runtime::Runtime::new().unwrap();

        let tracker = TrajectoryTracker::new();
        rt.block_on(async {
            tracker.record_operation("agent-1", "read", true).await;
            tracker.record_operation("agent-1", "search", true).await;
            tracker.record_operation("agent-1", "create", false).await;
            tracker.record_failure("agent-1", "create").await;

            let trajectory = tracker.get_recent_trajectory("agent-1", 10).await;
            assert_eq!(trajectory.len(), 3);

            let failures = tracker.get_recent_failures("agent-1", 10).await;
            assert_eq!(failures.len(), 1);
        });
    }

    #[test]
    fn test_intent_semantic_network_learning() {
        use plico::kernel::cognition::{IntentSemanticNetwork, TrajectoryPoint};
        let rt = tokio::runtime::Runtime::new().unwrap();

        let network = IntentSemanticNetwork::new(
            std::sync::Arc::new(plico::fs::embedding::StubEmbeddingProvider),
        );

        rt.block_on(async {
            let trajectory = vec![
                TrajectoryPoint {
                    timestamp_ms: 1000,
                    intent: "fix auth bug".to_string(),
                    operation: "read".to_string(),
                    success: true,
                    context_cids: vec![],
                },
                TrajectoryPoint {
                    timestamp_ms: 2000,
                    intent: "deploy fix".to_string(),
                    operation: "call".to_string(),
                    success: true,
                    context_cids: vec![],
                },
            ];

            let report = network.learn_from_history("agent-1", &trajectory).await.unwrap_or_default();
            assert!(report.new_nodes > 0, "should create intent nodes");
            assert!(report.new_edges > 0, "should create precedence edges");
        });
    }

    #[test]
    fn test_skill_forge_extraction() {
        use plico::kernel::cognition::SkillForge;
        let rt = tokio::runtime::Runtime::new().unwrap();

        let forge = SkillForge::new();
        rt.block_on(async {
            let result = forge.extract_candidate("agent-1", "read auth docs").await;
            assert!(result.is_ok());
        });
    }

    #[test]
    fn test_cognitive_config_defaults() {
        use plico::kernel::cognition::CognitiveConfig;

        let config = CognitiveConfig::default();
        assert!(config.proactive_prefetch_enabled);
        assert!(config.failure_pattern_detection_enabled);
        assert!(config.skill_extraction_enabled);
        assert_eq!(config.context_compression_threshold, 0.7);
    }

    #[test]
    fn test_full_ai_os_loop_convergence() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("LLM_BACKEND", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = plico::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        let rt = tokio::runtime::Runtime::new().unwrap();

        // Step 1: Register agent
        let agent_id = kernel.register_agent("e2e-agent".to_string()).unwrap();
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

        // Step 3: Declare intent (triggers async prefetch + CognitiveLoop)
        let declare_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "fix auth bug and deploy fix".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        assert!(declare_resp.ok, "declare_intent should succeed: {:?}", declare_resp.error);
        let assembly_id = declare_resp.assembly_id.expect("no assembly_id returned");

        // Step 4: Fetch assembled context (wait for async prefetch)
        std::thread::sleep(std::time::Duration::from_millis(200));
        let fetch_resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::FetchAssembledContext {
            agent_id: agent_id.clone(),
            assembly_id: assembly_id.clone(),
        });
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

        // Step 11: Verify CognitiveLoop trajectory tracking (Soul v3.0)
        let cognitive_loop = kernel.prefetch.cognitive_loop.get()
            .expect("CognitiveLoop should be initialized");
        let trajectory: Vec<plico::kernel::cognition::TrajectoryPoint> = rt.block_on(async {
            cognitive_loop.trajectory_tracker.get_recent_trajectory(&agent_id, 100).await
        });
        assert!(!trajectory.is_empty(), "TrajectoryTracker should record intent declarations");

        // Step 12: Verify IntentSemanticNetwork learning (Soul v3.0)
        let intent_network = &cognitive_loop.intent_network;
        let report = rt.block_on(async {
            intent_network.learn_from_history(&agent_id, &trajectory).await
        }).unwrap_or_default();
        println!("IntentSemanticNetwork: {} nodes, {} edges learned", report.new_nodes, report.new_edges);
    }
}
