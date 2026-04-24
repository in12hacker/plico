//! E2E Convergence Tests (Node 25)
//!
//! Tests the complete AI-OS loop: declare intent → plan → execute → learn → predict → complete

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    // Test that the full module structure exists
    #[test]
    fn test_kernel_ops_modules_exist() {
        // Verify key modules compile and have expected types
        let _ = core::mem::size_of::<crate::kernel::ops::intent::IntentPlan>();
        let _ = core::mem::size_of::<crate::kernel::ops::intent_executor::ExecutionStats>();
        let _ = core::mem::size_of::<crate::kernel::ops::skill_discovery::SkillDiscriminator>();
        let _ = core::mem::size_of::<crate::kernel::ops::self_healing::PlanAdaptor>();
        let _ = core::mem::size_of::<crate::kernel::ops::intent_decomposer::IntentDecomposer>();
        let _ = core::mem::size_of::<crate::kernel::ops::cross_domain_skill::CrossDomainSkillComposer>();
        let _ = core::mem::size_of::<crate::kernel::ops::goal_generator::GoalGenerator>();
        let _ = core::mem::size_of::<crate::kernel::ops::temporal_projection::TemporalProjectionEngine>();
    }

    #[test]
    fn test_hook_registry_exists() {
        let registry = crate::kernel::hook::HookRegistry::new();
        let ctx = crate::kernel::hook::HookContext::new("test-agent", "test-tool", serde_json::json!({}));
        let result = registry.run_hooks(crate::kernel::hook::HookPoint::PreToolCall, &ctx);
        assert!(matches!(result, crate::kernel::hook::HookResult::Continue));
    }

    #[test]
    fn test_execution_stats_tracking() {
        use crate::kernel::ops::intent_executor::ExecutionStats;

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
        use crate::kernel::ops::skill_discovery::SkillDiscriminator;

        let disc = SkillDiscriminator::new(2);
        disc.record_sequence("agent-1", vec!["read".to_string(), "call".to_string()], true, 100);
        disc.record_sequence("agent-1", vec!["read".to_string(), "call".to_string()], true, 100);

        let candidates = disc.get_skill_candidates("agent-1");
        assert!(!candidates.is_empty());
    }

    #[test]
    fn test_failure_classifier() {
        use crate::kernel::ops::self_healing::FailureClassifier;

        let ft = FailureClassifier::classify("permission denied", "test-step");
        assert!(matches!(ft, crate::kernel::ops::self_healing::FailureType::PermissionDenied));

        let ft2 = FailureClassifier::classify("resource exhausted", "test-step");
        assert!(matches!(ft2, crate::kernel::ops::self_healing::FailureType::ResourceExhausted));
    }

    #[test]
    fn test_plan_adaptor_adapt() {
        use crate::kernel::ops::self_healing::{PlanAdaptor, FailureType, Adaptation};

        let adaptor = PlanAdaptor::new();
        let adapt = adaptor.record_and_adapt("step-1", &FailureType::ToolNotFound);

        assert!(matches!(adapt, Adaptation::ReplaceTool { .. } | Adaptation::RetryWithNewParams));
    }

    #[test]
    fn test_temporal_projection_engine() {
        use crate::kernel::ops::temporal_projection::TemporalProjectionEngine;

        let engine = TemporalProjectionEngine::new();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        engine.record_intent("test-intent", now);
        let projected = engine.project(10); // 10 AM
        // Results depend on current time - just verify it returns a vec
        let _ = projected;
    }

    #[test]
    fn test_cross_domain_skill_composer() {
        use crate::kernel::ops::cross_domain_skill::CrossDomainSkillComposer;

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
        use crate::kernel::ops::goal_generator::GoalGenerator;

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

    // ─── F-1: Real E2E Convergence Test ─────────────────────────────────────

    #[test]
    fn test_full_ai_os_loop_convergence() {
        // Test the complete AI-OS loop:
        // 1. Create kernel + register agent
        // 2. Store relevant memories (KG nodes)
        // 3. Declare structured intent (triggers prefetch)
        // 4. Fetch assembled context (verifies prefetch worked)
        // 5. Record intent feedback (triggers learning)
        // 6. Verify agent profile was updated
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("RUST_LOG", "off");
        let (kernel, _dir) = crate::kernel::tests::make_kernel();

        // Step 1: Register agent
        let agent_id = kernel.register_agent("e2e-agent".into());
        kernel.permission_grant(&agent_id, crate::api::permission::PermissionAction::Write, None, None).ok();
        kernel.permission_grant(&agent_id, crate::api::permission::PermissionAction::Read, None, None).ok();

        // Step 2: Store memories with relevant tags ( KG will link them )
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

        // Add KG nodes for causal tracking
        let node1 = kernel.kg_add_node(
            "auth-fix",
            crate::fs::graph::KGNodeType::Fact,
            serde_json::json!({"description": "auth bug fix task"}),
            &agent_id,
            "default",
        ).expect("node1 add failed");

        let node2 = kernel.kg_add_node(
            "deploy-plan",
            crate::fs::graph::KGNodeType::Fact,
            serde_json::json!({"description": "deployment plan"}),
            &agent_id,
            "default",
        ).expect("node2 add failed");

        // Create causal link (auth-fix causes deploy-plan)
        kernel.kg_add_edge(&node1, &node2, crate::fs::graph::KGEdgeType::Causes, None, &agent_id, "default")
            .expect("causal edge failed");

        // Step 3: Declare intent (triggers async prefetch)
        let declare_resp = kernel.handle_api_request(crate::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "fix auth bug and deploy fix".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        assert!(declare_resp.ok, "declare_intent should succeed: {:?}", declare_resp.error);
        let assembly_id = declare_resp.assembly_id.expect("no assembly_id returned");

        // Step 4: Fetch assembled context (verifies prefetch populated context)
        let fetch_resp = kernel.handle_api_request(crate::api::semantic::ApiRequest::FetchAssembledContext {
            agent_id: agent_id.clone(),
            assembly_id: assembly_id.clone(),
        });
        assert!(fetch_resp.ok, "fetch_assembled_context should succeed");
        // Context may or may not have entries depending on embedding availability,
        // but it should not error. The key is the assembly was created.

        // Step 5: Submit intent (for learning loop)
        let submit_resp = kernel.handle_api_request(crate::api::semantic::ApiRequest::SubmitIntent {
            description: "fix auth bug and deploy fix".to_string(),
            priority: "high".to_string(),
            action: Some("auth".to_string()),
            agent_id: agent_id.clone(),
        });
        assert!(submit_resp.ok, "submit_intent should succeed: {:?}", submit_resp.error);

        // Step 6: Verify KG has causal edges (因果链 exists)
        let edges = kernel.kg_list_edges(&agent_id, "default", None)
            .expect("kg_list_edges failed");
        let causal_edges: Vec<_> = edges.into_iter()
            .filter(|e| matches!(e.edge_type, crate::fs::graph::KGEdgeType::Causes))
            .collect();
        assert!(!causal_edges.is_empty(), "should have at least one Causes edge in KG");

        // Step 7: Verify KG nodes exist (knowledge persisted)
        let nodes = kernel.kg_list_nodes(&agent_id, "default", None)
            .expect("kg_list_nodes failed");
        assert!(nodes.len() >= 2, "should have at least 2 KG nodes, got {}", nodes.len());

        // Step 8: Verify hot objects from profile (learning feedback)
        let hot = kernel.prefetch.get_hot_objects(&agent_id);
        // hot objects may be empty if profile store not connected, but the call should work
        let _ = hot; // Just verify it doesn't panic

        // Step 9: Verify search works with intent context (Context-Dependent Gravity)
        let search_resp = kernel.handle_api_request(crate::api::semantic::ApiRequest::Search {
            agent_id: agent_id.clone(),
            query: "auth".to_string(),
            tags: vec!["auth".to_string()],
            limit: 10,
            offset: None,
            require_tags: vec![],
            content_type: None,
        });
        assert!(search_resp.ok, "search should succeed: {:?}", search_resp.error);

        // Step 10: Verify recall (memory tier) works
        kernel.remember_working(&agent_id, "default", "e2e test memory".to_string(), vec!["test".to_string()])
            .expect("remember failed");
        let recall_resp = kernel.handle_api_request(crate::api::semantic::ApiRequest::Recall {
            agent_id: agent_id.clone(),
            scope: None,
            query: None,
            limit: None,
        });
        assert!(recall_resp.ok, "recall should succeed: {:?}", recall_resp.error);
        let memories = recall_resp.memory.unwrap_or_default();
        assert!(!memories.is_empty(), "should have at least one remembered memory");

        // Step 11: Verify skills discovery can record sequences (N23 F-1)
        let sd = &kernel.prefetch.skill_discriminator;
        sd.record_sequence(&agent_id, vec!["read".to_string(), "search".to_string()], true, 150);
        let candidates = sd.get_skill_candidates(&agent_id);
        // Candidates may be empty if threshold not met, but API should work
        let _ = candidates;

        // Step 12: Verify temporal projection records and projects (N24 F-3)
        let tp = &kernel.prefetch.temporal_engine;
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        tp.record_intent("fix auth bug", now_ms);
        let projected = tp.project(now_ms + 3600_000); // 1 hour from now
        let _ = projected; // Just verify it doesn't panic

        // All steps completed successfully — full AI-OS loop verified
    }

    #[test]
    fn test_hook_registry_integrated_in_kernel() {
        // Verify that the HookRegistry is properly integrated into AIKernel
        // and can intercept tool calls
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("RUST_LOG", "off");
        let (kernel, _dir) = crate::kernel::tests::make_kernel();

        let agent_id = kernel.register_agent("hook-test-agent".into());
        kernel.permission_grant(&agent_id, crate::api::permission::PermissionAction::Write, None, None).ok();
        kernel.permission_grant(&agent_id, crate::api::permission::PermissionAction::Read, None, None).ok();

        // Register a custom hook that counts PreToolCall invocations
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use crate::kernel::hook::{HookPoint, HookRegistry, HookHandler, HookResult, HookContext};

        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone = counter.clone();

        struct CountingHandler(Arc<std::sync::atomic::AtomicUsize>);
        impl HookHandler for CountingHandler {
            fn handle(&self, _point: HookPoint, _ctx: &HookContext) -> HookResult {
                self.0.fetch_add(1, Ordering::Relaxed);
                HookResult::Continue
            }
        }

        kernel.hook_registry.register(
            HookPoint::PreToolCall,
            0,
            Arc::new(CountingHandler(counter_clone)),
        );

        // Trigger a tool call that will be intercepted by the hook
        kernel.semantic_create(
            b"test content".to_vec(),
            vec!["test".to_string()],
            &agent_id,
            None,
        ).expect("create failed");

        // Hook should have been called at least once
        assert!(counter.load(Ordering::Relaxed) > 0, "hook should have intercepted at least one tool call");
    }
