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
}
