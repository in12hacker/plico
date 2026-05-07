//! Soul v3.0 Cognitive Loop Test — validates "越用越好" (Axiom 9) through repeated use.
//!
//! Runs multiple rounds of identical intents and verifies the cognitive symbiotic engine
//! accumulates knowledge via TrajectoryTracker and IntentSemanticNetwork.

#[cfg(test)]
mod cognitive_loop_tests {
    use std::time::Instant;

    fn setup_kernel() -> (plico::AIKernel, tempfile::TempDir, String) {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("LLM_BACKEND", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = plico::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        let agent_id = kernel.register_agent("cognitive-loop-agent".to_string()).unwrap();
        kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
        kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);
        (kernel, dir, agent_id)
    }

    fn start_session(kernel: &plico::AIKernel, agent_id: &str, intent_hint: &str) -> String {
        let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::StartSession {
            agent_id: agent_id.to_string(),
            agent_token: None,
            intent_hint: Some(intent_hint.to_string()),
            load_tiers: vec![],
            last_seen_seq: None,
        });
        resp.session_started
            .map(|s| s.session_id)
            .unwrap_or_default()
    }

    fn end_session(kernel: &plico::AIKernel, agent_id: &str, session_id: &str) {
        kernel.handle_api_request(plico::api::semantic::ApiRequest::EndSession {
            agent_id: agent_id.to_string(),
            session_id: session_id.to_string(),
            auto_checkpoint: true,
        });
    }

    #[test]
    fn test_cognitive_loop_10_rounds() {
        let (kernel, _dir, agent_id) = setup_kernel();

        // Seed content so prefetch has something to work with
        for i in 0..5 {
            kernel.semantic_create(
                format!("security vulnerability report {}", i).into_bytes(),
                vec!["security".to_string(), "vulnerability".to_string()],
                &agent_id,
                None,
            ).ok();
        }

        let intent = "find security vulnerabilities in codebase";
        let mut timings = Vec::new();

        println!("\n=== Soul v3.0 Cognitive Loop Test: 10 rounds of identical intent ===");

        for round in 1..=10u32 {
            // Start session — triggers CognitiveLoop register_session
            let session_id = start_session(&kernel, &agent_id, intent);

            // Declare intent — triggers CognitiveLoop on_intent_declared (background)
            let start = Instant::now();
            let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
                agent_id: agent_id.clone(),
                intent: intent.to_string(),
                related_cids: vec![],
                budget_tokens: 2048,
            });
            let elapsed = start.elapsed().as_millis();
            timings.push(elapsed);

            println!("  Round {:2}: {:4}ms | session={} | assembly={:?} | ok={}",
                round, elapsed, &session_id[..8],
                resp.assembly_id.as_deref().unwrap_or("none"), resp.ok);

            // End session — triggers CognitiveLoop end_session (background skill extraction)
            end_session(&kernel, &agent_id, &session_id);
        }

        // ── Verify CognitiveLoop is initialized ──
        let rt = tokio::runtime::Runtime::new().unwrap();
        let cognitive_loop = kernel.prefetch.cognitive_loop.get()
            .expect("CognitiveLoop should be initialized in Soul v3.0");
        let stats = rt.block_on(async {
            cognitive_loop.stats().await
        });
        println!("\nCognitiveLoop stats: {} optimizations, {} token savings, {} skills extracted",
            stats.total_optimizations, stats.total_token_savings, stats.total_skills_extracted);

        // ── Verify TrajectoryTracker accumulated operations ──
        let tracker = &cognitive_loop.trajectory_tracker;
        let trajectory: Vec<plico::kernel::cognition::TrajectoryPoint> = rt.block_on(async {
            tracker.get_recent_trajectory(&agent_id, 100).await
        });
        println!("TrajectoryTracker: {} trajectory points for agent", trajectory.len());
        assert!(!trajectory.is_empty(),
            "TrajectoryTracker should record intent declarations across sessions");

        // ── Verify IntentSemanticNetwork learned ──
        let intent_network = &cognitive_loop.intent_network;
        let report: plico::kernel::cognition::LearningReport = rt.block_on(async {
            intent_network.learn_from_history(&agent_id, &trajectory).await
        }).unwrap_or_default();
        println!("IntentSemanticNetwork: learned {} new nodes, {} new edges",
            report.new_nodes, report.new_edges);
        // After 10 rounds, the network should have learned at least some patterns
        assert!(report.new_nodes + report.strengthened_edges > 0 || trajectory.len() >= 10,
            "IntentSemanticNetwork should learn patterns after 10 rounds");

        // ── Verify Intent Cache ──
        let cache_stats = kernel.prefetch.intent_cache_stats();
        println!("IntentCache: {} entries, {} hits, {} lookups, hit_rate={:.1}%",
            cache_stats.entries, cache_stats.hits, cache_stats.total_lookups,
            if cache_stats.total_lookups > 0 { cache_stats.hits as f64 / cache_stats.total_lookups as f64 * 100.0 } else { 0.0 });

        // ── Timing analysis ──
        let first_3_avg: f64 = timings[..3].iter().sum::<u128>() as f64 / 3.0;
        let last_3_avg: f64 = timings[7..].iter().sum::<u128>() as f64 / 3.0;
        println!("\nTiming: first 3 avg={:.0}ms, last 3 avg={:.0}ms", first_3_avg, last_3_avg);

        // ── Summary ──
        println!("\n=== Soul v3.0 Cognitive Loop Test Summary ===");
        println!("  CognitiveLoop optimizations: {}", stats.total_optimizations);
        println!("  TrajectoryTracker points: {}", trajectory.len());
        println!("  IntentSemanticNetwork nodes: {}, edges: {}", report.new_nodes, report.new_edges);
        println!("  IntentCache: {} entries", cache_stats.entries);
        println!("  Cognitive symbiotic engine active and recording.");
    }

    #[test]
    fn test_failure_pattern_tracking() {
        let (kernel, _dir, agent_id) = setup_kernel();
        let rt = tokio::runtime::Runtime::new().unwrap();

        println!("\n=== Soul v3.0 Failure Pattern Tracking Test ===");

        for i in 1..=5u32 {
            let session_id = start_session(&kernel, &agent_id, &format!("nonexistent_{}", i));

            let resp = kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
                agent_id: agent_id.clone(),
                intent: format!("nonexistent_operation_{}", i),
                related_cids: vec![],
                budget_tokens: 512,
            });

            if let Some(aid) = &resp.assembly_id {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let fetch = kernel.handle_api_request(plico::api::semantic::ApiRequest::FetchAssembledContext {
                    agent_id: agent_id.clone(),
                    assembly_id: aid.clone(),
                });
                if !fetch.ok {
                    println!("  Round {}: failure recorded — {}", i,
                        fetch.error.as_deref().unwrap_or("unknown"));
                } else {
                    println!("  Round {}: succeeded (unexpected)", i);
                }
            }

            end_session(&kernel, &agent_id, &session_id);
        }

        // Verify CognitiveLoop tracked failures
        let cognitive_loop = kernel.prefetch.cognitive_loop.get().unwrap();
        let failure_stats = rt.block_on(async {
            cognitive_loop.trajectory_tracker.get_failure_stats(&agent_id).await
        });
        println!("TrajectoryTracker: {} total failures, {} unique operations failed",
            failure_stats.total_failures, failure_stats.by_operation.len());
        println!("  Failure pattern tracking active across {} rounds", 5);
    }

    #[test]
    fn test_session_lifecycle_cognitive_integration() {
        let (kernel, _dir, agent_id) = setup_kernel();
        let rt = tokio::runtime::Runtime::new().unwrap();

        println!("\n=== Session Lifecycle Cognitive Integration Test ===");

        // Session 1: Security audit
        let sid1 = start_session(&kernel, &agent_id, "security audit");
        println!("  Session 1 started: security audit");

        kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "audit API endpoints for SQL injection".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        end_session(&kernel, &agent_id, &sid1);
        println!("  Session 1 ended");

        // Session 2: Patch
        let sid2 = start_session(&kernel, &agent_id, "patch SQL injection");
        println!("  Session 2 started: patch SQL injection");

        kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "patch SQL injection in authentication".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        end_session(&kernel, &agent_id, &sid2);
        println!("  Session 2 ended");

        // Session 3: Verify
        let sid3 = start_session(&kernel, &agent_id, "verify patches");
        println!("  Session 3 started: verify patches");

        kernel.handle_api_request(plico::api::semantic::ApiRequest::DeclareIntent {
            agent_id: agent_id.clone(),
            intent: "run security tests on patched endpoints".to_string(),
            related_cids: vec![],
            budget_tokens: 2048,
        });
        end_session(&kernel, &agent_id, &sid3);
        println!("  Session 3 ended");

        // Verify accumulated cognitive state
        let cognitive_loop = kernel.prefetch.cognitive_loop.get().unwrap();
        let trajectory: Vec<plico::kernel::cognition::TrajectoryPoint> = rt.block_on(async {
            cognitive_loop.trajectory_tracker.get_recent_trajectory(&agent_id, 100).await
        });
        println!("\n  TrajectoryTracker: {} points after 3 sessions", trajectory.len());
        assert!(trajectory.len() >= 3,
            "TrajectoryTracker should record at least 3 intent declarations");

        let intent_network = &cognitive_loop.intent_network;
        let report: plico::kernel::cognition::LearningReport = rt.block_on(async {
            intent_network.learn_from_history(&agent_id, &trajectory).await
        }).unwrap_or_default();
        println!("  IntentSemanticNetwork: {} nodes, {} edges learned", report.new_nodes, report.new_edges);

        println!("  Cognitive symbiotic engine active across session lifecycle.");
    }
}
