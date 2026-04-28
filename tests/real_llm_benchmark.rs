//! Real LLM Benchmark — end-to-end tests with actual local LLM and embedding models.
//!
//! Requires running llama-server instances:
//! - LLM (Gemma 4): port 18920
//! - Embedding (v5-small-retrieval): port 18921
//!
//! Run:
//!   LLAMA_URL=http://127.0.0.1:18920 \
//!   EMBEDDING_API_BASE=http://127.0.0.1:18921 \
//!   cargo test --test real_llm_benchmark -- --nocapture --test-threads=1
//!
//! Each test prints latency and quality metrics for real-world evaluation.

use plico::kernel::AIKernel;
use plico::memory::{MemoryScope, MemoryType, MemoryTier, MemoryContent, MemoryEntry};
use plico::fs::retrieval_router::{
    classify_by_rules, classify_by_llm_response, intent_classification_prompt,
    QueryIntent,
};
use plico::memory::distillation::{distill_working_memory, to_long_term_entry, summarization_prompt};
use plico::memory::forgetting::{contradiction_prompt, parse_contradiction_response};
use plico::memory::causal::CausalGraph;
use plico::llm::{LlmProvider, ChatMessage, ChatOptions};
use plico::fs::embedding::types::EmbeddingProvider;
use std::time::Instant;
use tempfile::tempdir;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

const SKIP_MSG: &str = "SKIP: LLM/embedding servers not available (set LLAMA_URL and EMBEDDING_API_BASE)";

fn is_real_backend() -> bool {
    std::env::var("LLAMA_URL").is_ok() || std::env::var("EMBEDDING_API_BASE").is_ok()
}

fn make_real_kernel() -> Option<(AIKernel, tempfile::TempDir)> {
    if !is_real_backend() {
        return None;
    }
    std::env::set_var("LLM_BACKEND", "llama");
    std::env::set_var("EMBEDDING_BACKEND", "openai");
    let dir = tempdir().unwrap();
    match AIKernel::new(dir.path().to_path_buf()) {
        Ok(kernel) => Some((kernel, dir)),
        Err(e) => {
            eprintln!("Failed to create kernel with real backends: {e}");
            None
        }
    }
}

fn make_llm_provider() -> Option<Box<dyn LlmProvider>> {
    let url = std::env::var("LLAMA_URL").ok()?;
    let url = if url.contains("/v1") { url } else { format!("{}/v1", url) };
    let model = std::env::var("LLAMA_MODEL").unwrap_or_else(|_| "default".into());
    match plico::llm::openai::OpenAICompatibleProvider::new(&url, &model, None) {
        Ok(p) => Some(Box::new(p)),
        Err(e) => {
            eprintln!("Failed to create LLM provider: {e}");
            None
        }
    }
}

fn make_embedding_provider() -> Option<Box<dyn EmbeddingProvider>> {
    let url = std::env::var("EMBEDDING_API_BASE").ok()?;
    let url = if url.contains("/v1") { url } else { format!("{}/v1", url) };
    match plico::fs::embedding::openai::OpenAIEmbeddingBackend::new(&url, "default", None) {
        Ok(p) => Some(Box::new(p)),
        Err(e) => {
            eprintln!("Failed to create embedding provider: {e}");
            None
        }
    }
}

fn llm_chat(provider: &dyn LlmProvider, prompt: &str) -> Result<String, String> {
    let messages = vec![ChatMessage {
        role: "user".to_string(),
        content: prompt.to_string(),
    }];
    let opts = ChatOptions {
        temperature: 0.1,
        max_tokens: Some(200),
    };
    provider.chat(&messages, &opts)
        .map(|(text, _prompt_tok, _compl_tok)| text)
        .map_err(|e| format!("{e}"))
}

// ═══════════════════════════════════════════════════════════════════════
// B1: Intent Classification — LLM accuracy vs rule-based
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b1_intent_classification() {
    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let test_cases: Vec<(&str, QueryIntent)> = vec![
        ("What is the capital of France?", QueryIntent::Factual),
        ("When did Alice join the team?", QueryIntent::Temporal),
        ("What happened before the database migration?", QueryIntent::Temporal),
        ("Why did the auth service fail after the config change?", QueryIntent::MultiHop),
        ("How did the refactoring affect performance metrics?", QueryIntent::MultiHop),
        ("What does Bob prefer for deployment strategy?", QueryIntent::Preference),
        ("Which testing framework does the team like best?", QueryIntent::Preference),
        ("List all bugs fixed in the last sprint", QueryIntent::Aggregation),
        ("Summarize the key decisions from the architecture review", QueryIntent::Aggregation),
        ("What is the current database schema version?", QueryIntent::Factual),
    ];

    println!("\n=== B1: Intent Classification ===");
    println!("{:<60} {:>12} {:>10} {:>10} {:>8}", "Query", "Expected", "LLM", "Rules", "Lat(ms)");
    println!("{}", "-".repeat(104));

    let mut llm_correct = 0;
    let mut rule_correct = 0;
    let mut total_llm_ms = 0u128;

    for (query, expected) in &test_cases {
        let prompt = intent_classification_prompt(query);
        let t0 = Instant::now();
        let llm_result = match llm_chat(&*llm, &prompt) {
            Ok(resp) => classify_by_llm_response(&resp),
            Err(e) => {
                eprintln!("  LLM error for '{}': {}", query, e);
                None
            }
        };
        let llm_ms = t0.elapsed().as_millis();
        total_llm_ms += llm_ms;

        let rule_result = classify_by_rules(query);

        let llm_intent = llm_result.as_ref().map(|c| c.intent);
        let llm_ok = llm_intent == Some(*expected);
        let rule_ok = rule_result.intent == *expected;

        if llm_ok { llm_correct += 1; }
        if rule_ok { rule_correct += 1; }

        println!("{:<60} {:>12} {:>10} {:>10} {:>8}",
            &query[..query.len().min(58)],
            expected.name(),
            llm_intent.map(|i| i.name()).unwrap_or("FAIL"),
            rule_result.intent.name(),
            llm_ms,
        );
    }

    let n = test_cases.len();
    println!("\n  LLM accuracy:  {llm_correct}/{n} ({:.0}%)", llm_correct as f32 / n as f32 * 100.0);
    println!("  Rule accuracy: {rule_correct}/{n} ({:.0}%)", rule_correct as f32 / n as f32 * 100.0);
    println!("  Avg LLM latency: {:.0}ms", total_llm_ms as f64 / n as f64);
    println!("  Total LLM time: {total_llm_ms}ms");

    assert!(llm_correct >= n / 2, "LLM intent classification accuracy too low: {llm_correct}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B2: Embedding Quality — semantic similarity and retrieval
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b2_embedding_similarity() {
    let emb = match make_embedding_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let pairs: Vec<(&str, &str, bool)> = vec![
        ("The cat sat on the mat", "A feline rested on the rug", true),
        ("Database migration completed", "Schema update finished", true),
        ("The weather is sunny today", "Quantum physics is complex", false),
        ("Alice deployed the new API", "Alice pushed the service update", true),
        ("Memory pressure is high", "RAM usage exceeded threshold", true),
        ("I like pizza", "The stock market crashed", false),
    ];

    println!("\n=== B2: Embedding Semantic Similarity ===");
    println!("{:<40} {:<40} {:>6} {:>8} {:>8}", "Text A", "Text B", "Sim?", "CosSim", "Lat(ms)");
    println!("{}", "-".repeat(146));

    let mut correct = 0;

    for (a, b, should_be_similar) in &pairs {
        let t0 = Instant::now();
        let emb_a = match emb.embed(a) {
            Ok(r) => r.embedding,
            Err(e) => { eprintln!("  Embed error: {e}"); continue; }
        };
        let emb_b = match emb.embed(b) {
            Ok(r) => r.embedding,
            Err(e) => { eprintln!("  Embed error: {e}"); continue; }
        };
        let lat_ms = t0.elapsed().as_millis();

        let cos_sim = cosine_similarity(&emb_a, &emb_b);
        let predicted_similar = cos_sim > 0.15;
        let ok = predicted_similar == *should_be_similar;
        if ok { correct += 1; }

        println!("{:<40} {:<40} {:>6} {:>8.4} {:>8}",
            &a[..a.len().min(38)],
            &b[..b.len().min(38)],
            if *should_be_similar { "YES" } else { "NO" },
            cos_sim,
            lat_ms,
        );
    }

    let n = pairs.len();
    println!("\n  Accuracy: {correct}/{n} ({:.0}%)", correct as f32 / n as f32 * 100.0);
    assert!(correct >= n / 2, "Embedding similarity accuracy too low: {correct}/{n}");
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 { return 0.0; }
    dot / (norm_a * norm_b)
}

// ═══════════════════════════════════════════════════════════════════════
// B3: Memory Distillation — LLM summarization quality
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b3_distillation() {
    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let working_entries = vec![
        make_entry("w1", "Alice fixed the login bug by updating the session token validation logic", MemoryType::Episodic, &["bugfix", "auth"]),
        make_entry("w2", "The session tokens were expiring too early due to timezone mismatch", MemoryType::Episodic, &["bugfix", "auth"]),
        make_entry("w3", "Deploy to staging succeeded after the auth fix", MemoryType::Episodic, &["deploy", "auth"]),
        make_entry("w4", "Best practice: always use UTC timestamps for session management", MemoryType::Semantic, &["best-practice", "auth"]),
        make_entry("w5", "To debug auth issues: check token expiry, verify timezone, inspect session store", MemoryType::Procedural, &["debug", "auth"]),
    ];

    println!("\n=== B3: Memory Distillation ===");

    let t0 = Instant::now();
    let distilled = distill_working_memory(&working_entries, |text| {
        let prompt = summarization_prompt(text);
        llm_chat(&*llm, &prompt).ok()
    });
    let distill_ms = t0.elapsed().as_millis();

    println!("  Input entries: {}", working_entries.len());
    println!("  Output distilled: {}", distilled.len());
    println!("  Distillation time: {}ms", distill_ms);

    for (i, d) in distilled.iter().enumerate() {
        let content_preview = if d.content.len() > 120 {
            format!("{}...", &d.content[..120])
        } else {
            d.content.clone()
        };
        println!("  [{i}] type={:?} importance={} tags={:?}", d.memory_type, d.importance, d.tags);
        println!("      content: {content_preview}");
    }

    assert!(!distilled.is_empty(), "Distillation produced no output");
    let total_input_chars: usize = working_entries.iter().map(|e| e.content.display().len()).sum();
    let total_output_chars: usize = distilled.iter().map(|d| d.content.len()).sum();
    let compression = 1.0 - (total_output_chars as f64 / total_input_chars as f64);
    println!("  Compression ratio: {:.1}% (input: {} chars, output: {} chars)",
        compression * 100.0, total_input_chars, total_output_chars);
}

// ═══════════════════════════════════════════════════════════════════════
// B4: Contradiction Detection — LLM vs rule-based
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b4_contradiction_detection() {
    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let test_cases: Vec<(&str, &str, bool)> = vec![
        ("The project uses PostgreSQL", "The project uses MySQL", true),
        ("Alice is the tech lead", "Bob is the tech lead", true),
        ("The API response time is 200ms", "The API response time is 500ms", true),
        ("We deploy on Fridays", "We deploy on Tuesdays", true),
        ("The server runs on Linux", "The server has 16GB RAM", false),
        ("Alice reviewed the PR", "Alice also wrote the tests", false),
        ("The meeting is at 3pm", "The meeting is at 3pm UTC", false),
        ("Python 3.9 is required", "Python 3.11 is recommended", true),
    ];

    println!("\n=== B4: Contradiction Detection ===");
    println!("{:<40} {:<40} {:>6} {:>8} {:>8}", "Old", "New", "Contr?", "LLM", "Lat(ms)");
    println!("{}", "-".repeat(146));

    let mut llm_correct = 0;
    let mut total_ms = 0u128;

    for (old, new, is_contradiction) in &test_cases {
        let prompt = contradiction_prompt(old, new);
        let t0 = Instant::now();
        let llm_detected = match llm_chat(&*llm, &prompt) {
            Ok(resp) => parse_contradiction_response(&resp),
            Err(e) => {
                eprintln!("  LLM error: {e}");
                false
            }
        };
        let lat_ms = t0.elapsed().as_millis();
        total_ms += lat_ms;

        if llm_detected == *is_contradiction { llm_correct += 1; }

        println!("{:<40} {:<40} {:>6} {:>8} {:>8}",
            &old[..old.len().min(38)],
            &new[..new.len().min(38)],
            if *is_contradiction { "YES" } else { "NO" },
            if llm_detected { "YES" } else { "NO" },
            lat_ms,
        );
    }

    let n = test_cases.len();
    println!("\n  LLM accuracy: {llm_correct}/{n} ({:.0}%)", llm_correct as f32 / n as f32 * 100.0);
    println!("  Avg LLM latency: {:.0}ms", total_ms as f64 / n as f64);
}

// ═══════════════════════════════════════════════════════════════════════
// B5: End-to-end Kernel — CAS store + semantic search with real embeddings
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b5_kernel_store_recall() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let agent_id = kernel.register_agent("bench-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    let facts = vec![
        ("The project deadline is March 15th", vec!["project", "deadline"]),
        ("Alice is the lead developer for the auth module", vec!["team", "alice", "auth"]),
        ("We use PostgreSQL 15 as the primary database", vec!["infra", "database"]),
        ("The CI pipeline runs on GitHub Actions", vec!["infra", "ci"]),
        ("Bob prefers Rust for systems programming", vec!["team", "bob", "preference"]),
        ("The microservices communicate via gRPC", vec!["architecture", "grpc"]),
        ("Memory usage should stay under 4GB per service", vec!["performance", "memory"]),
        ("We deploy to production every Wednesday", vec!["process", "deploy"]),
    ];

    println!("\n=== B5: End-to-End CAS Store + Semantic Search ===");

    let mut store_ms_total = 0u128;
    for (content, tags) in &facts {
        let t0 = Instant::now();
        let resp = kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: content.to_string(),
            content_encoding: ContentEncoding::Utf8,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        store_ms_total += t0.elapsed().as_millis();
        assert!(resp.ok, "Store failed: {:?}", resp.error);
    }
    println!("  Stored {} facts via CAS, total: {}ms (avg: {:.0}ms)",
        facts.len(), store_ms_total, store_ms_total as f64 / facts.len() as f64);

    let queries = vec![
        ("project deadline", "march"),
        ("auth module developer", "alice"),
        ("primary database", "postgresql"),
        ("services communication protocol", "grpc"),
        ("production deploy schedule", "wednesday"),
    ];

    println!("\n  {:>5} {:<45} {:>8} {:>6} {}", "#", "Query", "Lat(ms)", "Found", "Top result preview");
    println!("  {}", "-".repeat(120));

    let mut found_count = 0;
    let mut total_search_ms = 0u128;

    for (i, (query, expected_keyword)) in queries.iter().enumerate() {
        let t0 = Instant::now();
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: query.to_string(),
            limit: Some(3),
            offset: None,
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
            intent_context: None,
        });
        let search_ms = t0.elapsed().as_millis();
        total_search_ms += search_ms;

        let results = resp.results.as_deref().unwrap_or(&[]);
        let top_preview = results.first()
            .map(|r| if r.snippet.len() > 60 { format!("{}...", &r.snippet[..60]) } else { r.snippet.clone() })
            .unwrap_or_else(|| "(empty)".into());

        let found = results.iter().any(|r|
            r.snippet.to_lowercase().contains(&expected_keyword.to_lowercase())
        );
        if found { found_count += 1; }

        println!("  {:>5} {:<45} {:>8} {:>6} {}",
            i + 1, query, search_ms, if found { "YES" } else { "NO" }, top_preview);
    }

    let n = queries.len();
    println!("\n  Semantic search accuracy: {found_count}/{n} ({:.0}%)", found_count as f64 / n as f64 * 100.0);
    println!("  Avg search latency: {:.0}ms", total_search_ms as f64 / n as f64);

    assert!(found_count >= 3, "Semantic search accuracy too low: {found_count}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B6: Recall Routed — intent-classified retrieval with real LLM + embeddings
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b6_recall_routed() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };
    let agent_id = kernel.register_agent("routed-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    let memories: Vec<(&str, Vec<&str>)> = vec![
        ("2024-01-15: Sprint planning meeting — decided to migrate to Kubernetes", vec!["meeting", "k8s"]),
        ("2024-02-01: Successfully deployed auth service to K8s cluster", vec!["deploy", "k8s"]),
        ("2024-02-15: Performance regression found in API gateway after K8s migration", vec!["bug", "k8s"]),
        ("Alice prefers blue-green deployments over canary", vec!["preference", "deploy"]),
        ("Bob prefers canary deployments with gradual rollout", vec!["preference", "deploy"]),
        ("Total API requests per day: 2.5 million", vec!["metrics", "api"]),
        ("Database backup runs every 6 hours", vec!["ops", "database"]),
        ("Redis cache hit rate is 95%", vec!["metrics", "redis"]),
    ];

    println!("\n=== B6: Recall Routed (Intent-classified with LongTerm Memory) ===");

    let t_store = Instant::now();
    for (content, tags) in &memories {
        let _ = kernel.remember_long_term(
            &agent_id, "default",
            content.to_string(),
            tags.iter().map(|s| s.to_string()).collect(),
            50,
        );
    }
    let store_ms = t_store.elapsed().as_millis();
    println!("  Stored {} memories to LongTerm (with embeddings) in {}ms", memories.len(), store_ms);

    let routed_queries = vec![
        ("What happened after the K8s migration?", QueryIntent::Temporal),
        ("Why did the API gateway have performance issues?", QueryIntent::MultiHop),
        ("What does Alice prefer for deployment?", QueryIntent::Preference),
        ("How many API requests per day?", QueryIntent::Factual),
        ("Summarize all infrastructure decisions", QueryIntent::Aggregation),
    ];

    println!("\n  {:>5} {:<50} {:>10} {:>10} {:>8} {:>6}",
        "#", "Query", "Expected", "Classified", "Lat(ms)", "Hits");
    println!("  {}", "-".repeat(100));

    let mut correct_intent = 0;
    let mut total_hits = 0;

    for (i, (query, expected_intent)) in routed_queries.iter().enumerate() {
        let t0 = Instant::now();
        let result = kernel.recall_routed(&agent_id, "default", query);
        let lat_ms = t0.elapsed().as_millis();

        match result {
            Ok((entries, classified)) => {
                let intent_ok = classified.intent == *expected_intent;
                if intent_ok { correct_intent += 1; }
                total_hits += entries.len();

                println!("  {:>5} {:<50} {:>10} {:>10} {:>8} {:>6}",
                    i + 1,
                    &query[..query.len().min(48)],
                    expected_intent.name(),
                    classified.intent.name(),
                    lat_ms,
                    entries.len(),
                );
                for e in entries.iter().take(2) {
                    let s = e.content.display();
                    let preview = if s.len() > 70 { format!("{}...", &s[..70]) } else { s.to_string() };
                    println!("          → {preview}");
                }
            }
            Err(e) => {
                println!("  {:>5} {:<50} {:>10} {:>10} {:>8} {:>6}",
                    i + 1, &query[..query.len().min(48)],
                    expected_intent.name(), "ERROR", lat_ms, e);
            }
        }
    }

    let n = routed_queries.len();
    println!("\n  Intent accuracy: {correct_intent}/{n} ({:.0}%)", correct_intent as f64 / n as f64 * 100.0);
    println!("  Total hits: {total_hits}");
    assert!(total_hits > 0, "Recall routed should return some results");
}

// ═══════════════════════════════════════════════════════════════════════
// B7: Causal Graph with real data
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b7_causal_graph() {
    println!("\n=== B7: Causal Graph ===");

    let ts = now_ms();
    let entries = vec![
        make_entry_ts("root", "Config changed: timeout=30s -> timeout=60s", ts - 3000,
            None, None),
        make_entry_ts("effect1", "API latency increased after config change", ts - 2000,
            Some("root".into()), None),
        make_entry_ts("effect2", "Users reported timeout errors", ts - 1000,
            Some("effect1".into()), None),
    ];

    let t0 = Instant::now();
    let graph = CausalGraph::build(&entries);
    let build_us = t0.elapsed().as_micros();

    let ancestors = graph.ancestors("effect2");
    let root_cause = graph.root_cause("effect2");
    let descendants = graph.descendants("root");

    println!("  Build time: {}μs", build_us);
    println!("  Ancestors of 'effect2': {:?}", ancestors);
    println!("  Root cause of 'effect2': {:?}", root_cause);
    println!("  Descendants of 'root': {:?}", descendants);

    assert_eq!(root_cause, "root");
    assert_eq!(ancestors.len(), 2);
    assert_eq!(descendants.len(), 2);
    println!("  All assertions PASSED");
}

// ═══════════════════════════════════════════════════════════════════════
// B8: Full Pipeline — store → distill → recall with real LLM
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b8_full_pipeline() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let agent_id = kernel.register_agent("pipeline-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    println!("\n=== B8: Full Pipeline (Store → Distill → Recall) ===");

    let session_facts = vec![
        ("Discussed migrating from Heroku to AWS ECS", MemoryType::Episodic),
        ("Cost analysis shows 40% savings with AWS", MemoryType::Semantic),
        ("Heroku costs $3000/month, estimated AWS ECS cost $1800/month", MemoryType::Semantic),
        ("Migration plan: 1) containerize apps 2) set up ECS clusters 3) gradual traffic shift", MemoryType::Procedural),
        ("Risk: possible downtime during DNS cutover", MemoryType::Episodic),
        ("Mitigation: use Route53 weighted routing for zero-downtime migration", MemoryType::Procedural),
    ];

    let t_store = Instant::now();
    let mut working_entries = Vec::new();
    for (content, mem_type) in &session_facts {
        let entry = make_entry(&uuid::Uuid::new_v4().to_string(), content, *mem_type, &["migration", "aws"]);
        working_entries.push(entry);
        let _ = kernel.remember(&agent_id, "default", content.to_string());
    }
    let store_ms = t_store.elapsed().as_millis();
    println!("  Phase 1 — Store: {} entries in {}ms", session_facts.len(), store_ms);

    let t_distill = Instant::now();
    let distilled = distill_working_memory(&working_entries, |text| {
        let prompt = summarization_prompt(text);
        llm_chat(&*llm, &prompt).ok()
    });
    let distill_ms = t_distill.elapsed().as_millis();
    println!("  Phase 2 — Distill: {} → {} entries in {}ms",
        working_entries.len(), distilled.len(), distill_ms);

    for d in &distilled {
        let lt_entry = to_long_term_entry(d, &agent_id, "default");
        let _ = kernel.remember(&agent_id, "default", lt_entry.content.display().to_string());
    }

    let t_recall = Instant::now();
    let recall_results = kernel.recall_relevant(&agent_id, "default", 4096);
    let recall_ms = t_recall.elapsed().as_millis();

    println!("  Phase 3 — Recall: {} results in {}ms", recall_results.len(), recall_ms);
    for (i, r) in recall_results.iter().take(5).enumerate() {
        let preview = {
            let s = r.content.display();
            if s.len() > 80 { format!("{}...", &s[..80]) } else { s.to_string() }
        };
        println!("    [{i}] tier={:?} type={:?}", r.tier, r.memory_type);
        println!("        {preview}");
    }

    println!("\n  Total pipeline: {}ms (store: {}ms + distill: {}ms + recall: {}ms)",
        store_ms + distill_ms + recall_ms, store_ms, distill_ms, recall_ms);

    assert!(!recall_results.is_empty(), "Full pipeline recall should find results");
}

// ═══════════════════════════════════════════════════════════════════════
// B9: Scale Test — 50 entries store + search performance degradation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b9_scale_store_search() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let agent_id = kernel.register_agent("scale-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    let corpus: Vec<(String, Vec<String>)> = (0..50).map(|i| {
        let domain = match i % 5 {
            0 => "infrastructure",
            1 => "team",
            2 => "process",
            3 => "architecture",
            _ => "metrics",
        };
        let content = match i % 10 {
            0 => format!("Server #{} runs Ubuntu 22.04 with {}GB RAM", i, 8 + i),
            1 => format!("Engineer-{} specializes in {} development", i, if i % 2 == 0 { "backend" } else { "frontend" }),
            2 => format!("Sprint {} review: {} story points completed", i, 20 + i),
            3 => format!("Service {} uses {} for inter-process communication", i, if i % 2 == 0 { "gRPC" } else { "REST" }),
            4 => format!("Average response time for endpoint-{}: {}ms", i, 50 + i * 3),
            5 => format!("Database shard {} contains {} million records", i, i * 2),
            6 => format!("Team member {} joined in 20{}", i, 20 + i % 5),
            7 => format!("CI pipeline stage {}: average duration {}s", i, 30 + i * 2),
            8 => format!("Microservice {} deployed to {} replicas", i, 2 + i % 4),
            _ => format!("Monitoring alert #{}: CPU usage at {}%", i, 40 + i),
        };
        (content, vec![domain.to_string(), format!("item-{i}")])
    }).collect();

    println!("\n=== B9: Scale Test ({} entries) ===", corpus.len());

    let mut store_latencies = Vec::with_capacity(corpus.len());
    let t_total_store = Instant::now();
    for (content, tags) in &corpus {
        let t0 = Instant::now();
        let resp = kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: content.clone(),
            content_encoding: ContentEncoding::Utf8,
            tags: tags.clone(),
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        store_latencies.push(t0.elapsed().as_millis());
        assert!(resp.ok, "Store failed at entry: {:?}", resp.error);
    }
    let total_store_ms = t_total_store.elapsed().as_millis();

    store_latencies.sort();
    let p50_store = store_latencies[store_latencies.len() / 2];
    let p95_store = store_latencies[store_latencies.len() * 95 / 100];
    let p99_store = store_latencies[store_latencies.len() * 99 / 100];
    let avg_store = total_store_ms as f64 / corpus.len() as f64;

    println!("  Store: {} entries in {}ms", corpus.len(), total_store_ms);
    println!("    avg={:.1}ms  p50={}ms  p95={}ms  p99={}ms", avg_store, p50_store, p95_store, p99_store);

    let search_queries = vec![
        ("Ubuntu server with RAM", "ram"),
        ("backend developer specialization", "backend"),
        ("sprint review story points", "sprint"),
        ("gRPC inter-process communication", "grpc"),
        ("response time endpoint latency", "response"),
        ("database shard records", "shard"),
        ("CI pipeline duration", "pipeline"),
        ("microservice deployment replicas", "replica"),
        ("monitoring CPU alert", "cpu"),
        ("team member joined date", "joined"),
    ];

    println!("\n  {:>3} {:<45} {:>8} {:>6} {}", "#", "Query", "Lat(ms)", "Hits", "Top result");
    println!("  {}", "-".repeat(110));

    let mut search_latencies = Vec::with_capacity(search_queries.len());
    let mut total_found = 0;

    for (i, (query, keyword)) in search_queries.iter().enumerate() {
        let t0 = Instant::now();
        let resp = kernel.handle_api_request(ApiRequest::Search {
            query: query.to_string(),
            limit: Some(5),
            offset: None,
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
            intent_context: None,
        });
        let lat = t0.elapsed().as_millis();
        search_latencies.push(lat);

        let results = resp.results.as_deref().unwrap_or(&[]);
        let found = results.iter().any(|r| r.snippet.to_lowercase().contains(keyword));
        if found { total_found += 1; }

        let preview = results.first()
            .map(|r| if r.snippet.len() > 55 { format!("{}...", &r.snippet[..55]) } else { r.snippet.clone() })
            .unwrap_or_else(|| "(empty)".into());

        println!("  {:>3} {:<45} {:>8} {:>6} {}", i + 1, query, lat, results.len(), preview);
    }

    search_latencies.sort();
    let avg_search = search_latencies.iter().sum::<u128>() as f64 / search_latencies.len() as f64;
    let p50_search = search_latencies[search_latencies.len() / 2];
    let p95_search = search_latencies[search_latencies.len() * 95 / 100];

    println!("\n  Search: avg={:.1}ms  p50={}ms  p95={}ms", avg_search, p50_search, p95_search);
    println!("  Relevance: {total_found}/{} queries found keyword in top-5", search_queries.len());

    assert!(total_found >= search_queries.len() / 2, "Scale search accuracy too low: {total_found}/{}",
        search_queries.len());
}

// ═══════════════════════════════════════════════════════════════════════
// B10: Embedding Throughput — batch embedding latency
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b10_embedding_throughput() {
    let emb = match make_embedding_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let texts: Vec<String> = (0..30).map(|i| {
        match i % 6 {
            0 => format!("The architecture of microservice {} follows domain-driven design principles", i),
            1 => format!("Database query optimization reduced latency by {}% on shard {}", 10 + i, i),
            2 => format!("Sprint {} retrospective identified {} action items for improvement", i, 3 + i % 5),
            3 => format!("Load balancer distributes traffic across {} pods in cluster {}", 4 + i % 3, i),
            4 => format!("Security audit found {} medium-severity vulnerabilities in service {}", i % 4, i),
            _ => format!("Deployment pipeline {} takes {} minutes end-to-end", i, 5 + i % 10),
        }
    }).collect();

    println!("\n=== B10: Embedding Throughput ({} texts) ===", texts.len());

    let mut latencies = Vec::with_capacity(texts.len());
    let mut dimensions = 0usize;
    let t_total = Instant::now();

    for text in &texts {
        let t0 = Instant::now();
        match emb.embed(text) {
            Ok(result) => {
                latencies.push(t0.elapsed().as_millis());
                if dimensions == 0 { dimensions = result.embedding.len(); }
            }
            Err(e) => {
                eprintln!("  Embed error: {e}");
                latencies.push(t0.elapsed().as_millis());
            }
        }
    }

    let total_ms = t_total.elapsed().as_millis();
    latencies.sort();

    let avg = total_ms as f64 / texts.len() as f64;
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];
    let throughput = texts.len() as f64 / (total_ms as f64 / 1000.0);

    println!("  Total: {}ms for {} embeddings (dim={})", total_ms, texts.len(), dimensions);
    println!("  avg={:.1}ms  p50={}ms  p95={}ms  p99={}ms", avg, p50, p95, p99);
    println!("  Throughput: {:.1} embeddings/sec", throughput);

    let first_5_avg: f64 = latencies[..5].iter().map(|&l| l as f64).sum::<f64>() / 5.0;
    let last_5_avg: f64 = latencies[latencies.len()-5..].iter().map(|&l| l as f64).sum::<f64>() / 5.0;
    println!("  Cold start effect: first_5_avg={:.1}ms  last_5_avg={:.1}ms", first_5_avg, last_5_avg);
}

// ═══════════════════════════════════════════════════════════════════════
// B11: Multi-Session Memory Persistence — cross-session recall
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b11_multi_session_memory() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let agent_id = kernel.register_agent("session-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    println!("\n=== B11: Multi-Session Memory Persistence ===");

    let sessions = vec![
        vec![
            "Project Alpha uses React 18 for the frontend",
            "The backend is built with Rust and Actix-web",
            "PostgreSQL 15 is the primary database",
        ],
        vec![
            "Alice is the frontend lead for Project Alpha",
            "Bob handles the infrastructure and DevOps",
            "Sprint planning happens every Monday",
        ],
        vec![
            "Deployed v2.0 of Project Alpha to production",
            "Performance improved 30% after Rust migration",
            "Next milestone is to add real-time notifications",
        ],
    ];

    let mut total_store_ms = 0u128;
    for (session_idx, session) in sessions.iter().enumerate() {
        let t0 = Instant::now();
        for content in session {
            let _ = kernel.remember_long_term(
                &agent_id, "default",
                content.to_string(),
                vec![format!("session-{session_idx}")],
                50,
            );
        }
        let ms = t0.elapsed().as_millis();
        total_store_ms += ms;
        println!("  Session {}: stored {} memories in {}ms", session_idx + 1, session.len(), ms);
    }

    let cross_session_queries = vec![
        ("What technology stack does Project Alpha use?", vec!["react", "rust"]),
        ("Who is responsible for the frontend?", vec!["alice"]),
        ("What was the performance improvement?", vec!["30%"]),
        ("When is sprint planning?", vec!["monday"]),
        ("What is the next milestone?", vec!["notification"]),
    ];

    println!("\n  {:>3} {:<50} {:>8} {:>6} {}", "#", "Cross-session Query", "Lat(ms)", "Found", "Evidence");
    println!("  {}", "-".repeat(110));

    let mut found_count = 0;
    let mut total_search_ms = 0u128;

    for (i, (query, keywords)) in cross_session_queries.iter().enumerate() {
        let t0 = Instant::now();
        let results = kernel.recall_semantic(&agent_id, "default", query, 5);
        let lat = t0.elapsed().as_millis();
        total_search_ms += lat;

        match results {
            Ok(entries) => {
                let found = entries.iter().any(|e| {
                    let content = e.content.display().to_string().to_lowercase();
                    keywords.iter().any(|kw| content.contains(kw))
                });
                if found { found_count += 1; }

                let evidence = entries.first()
                    .map(|e| {
                        let s = e.content.display().to_string();
                        if s.len() > 50 { format!("{}...", &s[..50]) } else { s }
                    })
                    .unwrap_or_else(|| "(empty)".into());

                println!("  {:>3} {:<50} {:>8} {:>6} {}", i + 1, query, lat, if found { "YES" } else { "NO" }, evidence);
            }
            Err(e) => {
                println!("  {:>3} {:<50} {:>8} {:>6} ERROR: {}", i + 1, query, lat, "ERR", e);
            }
        }
    }

    let n = cross_session_queries.len();
    println!("\n  Cross-session recall: {found_count}/{n} ({:.0}%)", found_count as f64 / n as f64 * 100.0);
    println!("  Total store: {}ms  Total search: {}ms", total_store_ms, total_search_ms);

    assert!(found_count >= n / 2, "Cross-session recall too low: {found_count}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B12: LLM Latency Stability — 20-call consistency check
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b12_llm_latency_stability() {
    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    println!("\n=== B12: LLM Latency Stability (20 calls) ===");

    let prompts: Vec<String> = (0..20).map(|i| {
        intent_classification_prompt(&format!("Query number {} about various topics", i))
    }).collect();

    let mut latencies = Vec::with_capacity(prompts.len());
    let t_total = Instant::now();

    for (i, prompt) in prompts.iter().enumerate() {
        let t0 = Instant::now();
        let result = llm_chat(&*llm, prompt);
        let lat = t0.elapsed().as_millis();
        latencies.push(lat);

        if i < 3 || i >= prompts.len() - 3 || result.is_err() {
            let status = match &result {
                Ok(r) => r.trim().to_string(),
                Err(e) => format!("ERR: {e}"),
            };
            println!("  [{:>2}] {}ms — {}", i + 1, lat, status);
        } else if i == 3 {
            println!("  ... ({} calls) ...", prompts.len() - 6);
        }
    }

    let total_ms = t_total.elapsed().as_millis();
    latencies.sort();
    let avg = total_ms as f64 / latencies.len() as f64;
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let min_lat = latencies[0];
    let max_lat = latencies[latencies.len() - 1];
    let std_dev = {
        let mean = avg;
        let variance: f64 = latencies.iter()
            .map(|&l| { let diff = l as f64 - mean; diff * diff })
            .sum::<f64>() / latencies.len() as f64;
        variance.sqrt()
    };
    let cv = std_dev / avg * 100.0;

    println!("\n  Total: {}ms for {} calls", total_ms, latencies.len());
    println!("  avg={:.1}ms  p50={}ms  p95={}ms  min={}ms  max={}ms", avg, p50, p95, min_lat, max_lat);
    println!("  std_dev={:.1}ms  CV={:.1}%", std_dev, cv);
    println!("  Throughput: {:.1} calls/sec", latencies.len() as f64 / (total_ms as f64 / 1000.0));

    if cv > 50.0 {
        println!("  WARNING: High latency variance (CV>50%) — LLM service may be unstable");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

fn make_entry(id: &str, content: &str, mem_type: MemoryType, tags: &[&str]) -> MemoryEntry {
    let ts = now_ms();
    MemoryEntry {
        id: id.into(),
        agent_id: "bench-agent".into(),
        tenant_id: "default".into(),
        content: MemoryContent::Text(content.into()),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        tier: MemoryTier::Working,
        scope: MemoryScope::Private,
        created_at: ts,
        last_accessed: ts,
        access_count: 1,
        importance: 5,
        ttl_ms: None,
        original_ttl_ms: None,
        embedding: None,
        memory_type: mem_type,
        causal_parent: None,
        supersedes: None,
    }
}

fn make_entry_ts(
    id: &str, content: &str, ts: u64,
    causal_parent: Option<String>, supersedes: Option<String>,
) -> MemoryEntry {
    MemoryEntry {
        id: id.into(),
        agent_id: "bench-agent".into(),
        tenant_id: "default".into(),
        content: MemoryContent::Text(content.into()),
        tags: vec![],
        tier: MemoryTier::Working,
        scope: MemoryScope::Private,
        created_at: ts,
        last_accessed: ts,
        access_count: 1,
        importance: 5,
        ttl_ms: None,
        original_ttl_ms: None,
        embedding: None,
        memory_type: MemoryType::Episodic,
        causal_parent,
        supersedes,
    }
}
