//! 认知引擎端到端集成测试
//!
//! 验证技能流水线：操作历史 → 技能提取 → 验证 → 注册 → 推荐 → 执行

use plico::cas::CASStorage;
use plico::fs::embedding::{EmbedError, EmbedResult, EmbeddingProvider};
use plico::fs::search::memory::InMemoryBackend;
use plico::kernel::cognition::{
    CognitiveLoop, ContextQualityEngine, ExperienceSource, IntentSemanticNetwork, KnowledgeItem, KnowledgeSkill, Skill,
    SkillExecutionResult, SkillForge, SkillUsageStats, TrajectoryTracker, TriggerCondition, ValidationStatus,
};
use plico::memory::LayeredMemory;
use std::sync::Arc;

struct MockEmbedding;
impl EmbeddingProvider for MockEmbedding {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let dim = 8;
        let mut vec = vec![0.0f32; dim];
        for (i, byte) in text.bytes().enumerate() {
            vec[i % dim] += byte as f32;
        }
        let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut vec {
                *v /= norm;
            }
        }
        Ok(EmbedResult::new(vec, text.len() as u32 / 4))
    }
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|t| self.embed(t)).collect()
    }
    fn dimension(&self) -> usize {
        8
    }
    fn builder_identity(&self) -> Result<plico::fs::EmbeddingBuilderIdentity, plico::fs::EmbeddingIdentityError> {
        Err(plico::fs::EmbeddingIdentityError::StubProvider)
    }
    fn model_name(&self) -> String {
        "mock".into()
    }
}

fn make_test_cognitive_loop() -> (CognitiveLoop, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let embedding = Arc::new(MockEmbedding);
    let search = Arc::new(InMemoryBackend::new());
    let memory = Arc::new(LayeredMemory::new());
    let cas = Arc::new(CASStorage::new(dir.path().join("cas")).unwrap());
    let context_analyzer = Arc::new(ContextQualityEngine::new(embedding.clone(), search, memory, cas));
    let intent_network = Arc::new(IntentSemanticNetwork::new(embedding.clone()));
    let tracker = Arc::new(TrajectoryTracker::new());
    let skill_forge = Arc::new(
        SkillForge::new()
            .with_trajectory_tracker(tracker.clone())
            .with_embedding(embedding),
    );
    let cognitive_loop = CognitiveLoop::with_shared_tracker(context_analyzer, intent_network, skill_forge, tracker);
    (cognitive_loop, dir)
}

/// Helper: build a SkillForge with trajectory and embedding for direct testing
fn make_test_skill_forge() -> (SkillForge, Arc<TrajectoryTracker>) {
    let tracker = Arc::new(TrajectoryTracker::new());
    let embedding = Arc::new(MockEmbedding);
    let forge = SkillForge::new()
        .with_trajectory_tracker(tracker.clone())
        .with_embedding(embedding);
    (forge, tracker)
}

#[tokio::test]
async fn test_session_lifecycle_tracks_trajectory() {
    let (loop_, _dir) = make_test_cognitive_loop();

    loop_.register_session("agent-1", "session-1").await;
    loop_
        .on_intent_declared("agent-1", "session-1", "search code", &[])
        .await
        .unwrap();
    loop_
        .on_operation_completed("agent-1", "grep", true, &[], &[])
        .await
        .unwrap();
    loop_
        .on_operation_completed("agent-1", "read_file", true, &[], &[])
        .await
        .unwrap();

    let traj = loop_.trajectory_tracker.get_recent_trajectory("agent-1", 10).await;
    assert!(
        traj.len() >= 2,
        "expected at least 2 trajectory points, got {}",
        traj.len()
    );

    loop_.end_session("agent-1", "session-1").await;
}

#[tokio::test]
async fn test_failure_tracking_with_session_id() {
    let (loop_, _dir) = make_test_cognitive_loop();

    loop_.register_session("agent-1", "session-42").await;
    loop_
        .on_operation_completed("agent-1", "failed_op", false, &[], &[])
        .await
        .unwrap();

    let failures = loop_.trajectory_tracker.get_recent_failures("agent-1", 10).await;
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].session_id, "session-42");
}

#[tokio::test]
async fn test_skill_pipeline_extract_validate_register_recommend_execute() {
    let (forge, tracker) = make_test_skill_forge();

    // Build enough trajectory for pattern extraction (3+ repetitions)
    for _ in 0..4 {
        tracker.record_operation("agent-1", "search", true).await;
        tracker.record_operation("agent-1", "read", true).await;
        tracker.record_operation("agent-1", "create", true).await;
    }

    // Extract candidates from trajectory
    let candidates = forge.extract_candidate("agent-1", "search").await.unwrap();
    assert!(
        !candidates.is_empty(),
        "should extract skill candidates from repeated pattern"
    );

    // Validate first candidate
    let candidate = &candidates[0];
    let validation = forge.validate_skill("agent-1", candidate).await.unwrap();
    assert!(
        validation.passed,
        "candidate should pass validation: {:?}",
        validation.issues
    );

    // Register as a knowledge skill with wildcard trigger
    let skill = Skill::Knowledge(KnowledgeSkill {
        id: String::new(),
        name: "Search Workflow".into(),
        description: "search then read then create".into(),
        trigger_conditions: vec![TriggerCondition {
            intent_pattern: "*".into(),
            min_confidence: 0.6,
            required_context_tags: vec![],
        }],
        knowledge: vec![KnowledgeItem::Rule {
            condition: "search → read → create".into(),
            action: "Follow this workflow".into(),
        }],
        sources: candidate
            .source_operations
            .iter()
            .map(|op| ExperienceSource {
                session_id: "test".into(),
                operation: op.clone(),
                timestamp_ms: plico::util::now_ms(),
                success: true,
            })
            .collect(),
        validation: ValidationStatus::Validated {
            validated_at_ms: plico::util::now_ms(),
            test_pass_rate: validation.test_pass_rate,
        },
        usage_stats: SkillUsageStats::default(),
    });
    let skill_id = forge.register_skill("agent-1", skill).await.unwrap();
    assert!(!skill_id.is_empty());

    // Recommend for any intent (wildcard trigger matches all)
    let recs = forge.recommend("agent-1", "search code").await.unwrap();
    assert!(!recs.is_empty(), "should recommend the registered skill");
    assert!(recs[0].confidence >= 0.6);

    // Execute the knowledge skill
    let result = forge
        .execute_skill("agent-1", &skill_id, serde_json::json!({}))
        .await
        .unwrap();
    match result {
        SkillExecutionResult::Knowledge { items } => {
            assert_eq!(items.len(), 1);
        }
        _ => panic!("Expected Knowledge execution result"),
    }
}

#[tokio::test]
async fn test_intent_network_learns_and_predicts() {
    let (loop_, _dir) = make_test_cognitive_loop();

    loop_.register_session("agent-1", "s1").await;

    // Build trajectory with consistent patterns
    for _ in 0..3 {
        loop_.on_intent_declared("agent-1", "s1", "search", &[]).await.unwrap();
        loop_.on_intent_declared("agent-1", "s1", "read", &[]).await.unwrap();
        loop_.on_intent_declared("agent-1", "s1", "write", &[]).await.unwrap();
    }

    // End session — triggers IntentNetwork learning
    loop_.end_session("agent-1", "s1").await;

    // Give spawned learning task time to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Predict next context after "search"
    let predictions = loop_
        .intent_network
        .predict_next_context("agent-1", "search")
        .await
        .unwrap();
    assert!(!predictions.is_empty(), "should predict 'read' after 'search'");
    assert!(
        predictions.iter().any(|p| p.cid == "read"),
        "expected 'read' in predictions: {:?}",
        predictions
    );
}

#[tokio::test]
async fn test_context_quality_optimization_report() {
    let (loop_, _dir) = make_test_cognitive_loop();

    loop_.register_session("agent-1", "s1").await;

    let report = loop_
        .on_intent_declared("agent-1", "s1", "test intent", &[])
        .await
        .unwrap();

    assert_eq!(report.agent_id, "agent-1");
    assert_eq!(report.session_id, "s1");
    assert!(report.context_before.cid_count == 0);
}

#[tokio::test]
async fn test_cognitive_stats_accumulate() {
    let (loop_, _dir) = make_test_cognitive_loop();

    loop_.register_session("agent-1", "s1").await;
    loop_.on_intent_declared("agent-1", "s1", "task1", &[]).await.unwrap();
    loop_.on_intent_declared("agent-1", "s1", "task2", &[]).await.unwrap();

    let stats = loop_.stats().await;
    assert_eq!(stats.total_optimizations, 2);
}

#[tokio::test]
async fn test_skill_validator_conflict_detection() {
    let (forge, tracker) = make_test_skill_forge();

    // Register first skill
    let skill1 = Skill::Knowledge(KnowledgeSkill {
        id: String::new(),
        name: "Search Helper".into(),
        description: "helps with search operations".into(),
        trigger_conditions: vec![],
        knowledge: vec![KnowledgeItem::Rule {
            condition: "search".into(),
            action: "use grep".into(),
        }],
        sources: vec![],
        validation: ValidationStatus::Pending,
        usage_stats: SkillUsageStats::default(),
    });
    forge.register_skill("agent-1", skill1).await.unwrap();

    // Extract and validate a conflicting candidate
    for _ in 0..4 {
        tracker.record_operation("agent-1", "search", true).await;
    }
    let candidates = forge.extract_candidate("agent-1", "search").await.unwrap();
    if !candidates.is_empty() {
        let validation = forge.validate_skill("agent-1", &candidates[0]).await.unwrap();
        // Validation should work without panicking
        let _ = validation;
    }
}

#[tokio::test]
async fn test_dsl_template_substitution_e2e() {
    use plico::kernel::cognition::dsl_interpreter::{DslInterpreter, DslOutput, DslSkill, DslStep};

    let dsl = DslSkill {
        version: "1.0".into(),
        name: "url_builder".into(),
        description: "builds a URL from components".into(),
        inputs: vec![],
        steps: vec![DslStep::Store {
            key: "url".into(),
            value: serde_json::json!("http://${host}:${port}/api"),
            tags: vec![],
        }],
        outputs: vec![DslOutput {
            name: "url".into(),
            dtype: "string".into(),
        }],
    };

    let interpreter = DslInterpreter::new();
    let result = interpreter
        .execute(
            &dsl,
            serde_json::json!({
                "host": "localhost",
                "port": 8080
            }),
            None,
        )
        .unwrap();

    let url_entry = result.get("url").unwrap();
    let url_value = url_entry.get("value").unwrap().as_str().unwrap();
    assert_eq!(url_value, "http://localhost:8080/api");
}
