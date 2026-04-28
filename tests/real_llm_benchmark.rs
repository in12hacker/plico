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
// B13: Batch vs Sequential Embedding — measure batch API speedup
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b13_batch_embedding() {
    let emb = match make_embedding_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let texts: Vec<&str> = vec![
        "Rust is a systems programming language",
        "PostgreSQL is an advanced relational database",
        "Kubernetes orchestrates containerized applications",
        "gRPC uses protocol buffers for serialization",
        "Redis provides in-memory key-value storage",
        "Docker containers isolate application environments",
        "GraphQL enables flexible API queries",
        "Prometheus monitors system metrics and alerts",
        "Terraform manages infrastructure as code",
        "Elasticsearch powers full-text search capabilities",
    ];

    println!("\n=== B13: Batch vs Sequential Embedding ({} texts) ===", texts.len());

    let t_seq = Instant::now();
    let mut seq_results = Vec::new();
    for text in &texts {
        match emb.embed(text) {
            Ok(r) => seq_results.push(r),
            Err(e) => { eprintln!("  Sequential embed error: {e}"); return; }
        }
    }
    let seq_ms = t_seq.elapsed().as_millis();

    let t_batch = Instant::now();
    let batch_results = match emb.embed_batch(&texts) {
        Ok(r) => r,
        Err(e) => { eprintln!("  Batch embed error: {e}"); return; }
    };
    let batch_ms = t_batch.elapsed().as_millis();

    let speedup = if batch_ms > 0 { seq_ms as f64 / batch_ms as f64 } else { f64::INFINITY };

    println!("  Sequential: {}ms ({:.1}ms/text)", seq_ms, seq_ms as f64 / texts.len() as f64);
    println!("  Batch:      {}ms ({:.1}ms/text)", batch_ms, batch_ms as f64 / texts.len() as f64);
    println!("  Speedup:    {:.2}x", speedup);
    println!("  Results:    seq={} batch={}", seq_results.len(), batch_results.len());

    assert_eq!(seq_results.len(), batch_results.len());

    let mut embedding_match = 0;
    for (s, b) in seq_results.iter().zip(batch_results.iter()) {
        let sim = cosine_similarity(&s.embedding, &b.embedding);
        if sim > 0.99 { embedding_match += 1; }
    }
    println!("  Consistency: {}/{} embeddings match (>0.99 cosine)", embedding_match, texts.len());
}

// ═══════════════════════════════════════════════════════════════════════
// B14: End-to-End Multi-Round Conversation — distill + recall cycle
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b14_conversation_cycle() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let llm = match make_llm_provider() {
        Some(p) => p,
        None => { eprintln!("{SKIP_MSG}"); return; }
    };

    let agent_id = kernel.register_agent("convo-agent".into());
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    println!("\n=== B14: Multi-Round Conversation Cycle ===");

    let rounds = vec![
        vec![
            ("User asked about deployment strategy", MemoryType::Episodic, &["deploy"][..]),
            ("Team decided on blue-green deployment", MemoryType::Semantic, &["deploy", "decision"][..]),
        ],
        vec![
            ("Discussed monitoring setup for production", MemoryType::Episodic, &["monitoring"][..]),
            ("Prometheus + Grafana chosen for observability", MemoryType::Semantic, &["monitoring", "decision"][..]),
            ("Always set up alerts before deploying new services", MemoryType::Procedural, &["monitoring", "best-practice"][..]),
        ],
        vec![
            ("Sprint 5 planning: focus on auth redesign", MemoryType::Episodic, &["sprint", "auth"][..]),
            ("Auth will use JWT with refresh tokens", MemoryType::Semantic, &["auth", "decision"][..]),
        ],
    ];

    let mut total_store_ms = 0u128;
    let mut total_distill_ms = 0u128;
    let mut all_lt_count = 0;

    for (round_idx, round_entries) in rounds.iter().enumerate() {
        let t_store = Instant::now();
        let mut working_entries = Vec::new();
        for (content, mem_type, tags) in round_entries {
            let entry = make_entry(&uuid::Uuid::new_v4().to_string(), content, *mem_type,
                tags);
            working_entries.push(entry);
            let _ = kernel.remember(&agent_id, "default", content.to_string());
        }
        let store_ms = t_store.elapsed().as_millis();
        total_store_ms += store_ms;

        let t_distill = Instant::now();
        let distilled = distill_working_memory(&working_entries, |text| {
            let prompt = summarization_prompt(text);
            llm_chat(&*llm, &prompt).ok()
        });
        let distill_ms = t_distill.elapsed().as_millis();
        total_distill_ms += distill_ms;

        for d in &distilled {
            let _ = kernel.remember_long_term(
                &agent_id, "default",
                d.content.clone(),
                d.tags.clone(),
                d.importance,
            );
        }
        all_lt_count += distilled.len();

        println!("  Round {}: {} entries → {} distilled (store: {}ms, distill: {}ms)",
            round_idx + 1, round_entries.len(), distilled.len(), store_ms, distill_ms);
    }

    println!("\n  Totals: store={}ms, distill={}ms, LT entries={}", total_store_ms, total_distill_ms, all_lt_count);

    let verification_queries = vec![
        ("What deployment strategy did the team choose?", "blue-green"),
        ("What monitoring tools are being used?", "prometheus"),
        ("How does the auth system work?", "jwt"),
    ];

    println!("\n  {:>3} {:<50} {:>8} {:>6} {}", "#", "Verification Query", "Lat(ms)", "Found", "Top result");
    println!("  {}", "-".repeat(100));

    let mut found_count = 0;
    for (i, (query, keyword)) in verification_queries.iter().enumerate() {
        let t0 = Instant::now();
        let results = kernel.recall_semantic(&agent_id, "default", query, 3);
        let lat = t0.elapsed().as_millis();

        match results {
            Ok(entries) => {
                let found = entries.iter().any(|e|
                    e.content.display().to_string().to_lowercase().contains(keyword));
                if found { found_count += 1; }

                let preview = entries.first()
                    .map(|e| {
                        let s = e.content.display().to_string();
                        if s.len() > 50 { format!("{}...", &s[..50]) } else { s }
                    })
                    .unwrap_or_else(|| "(empty)".into());

                println!("  {:>3} {:<50} {:>8} {:>6} {}", i + 1, query, lat, if found { "YES" } else { "NO" }, preview);
            }
            Err(e) => {
                println!("  {:>3} {:<50} {:>8} {:>6} ERR: {}", i + 1, query, lat, "ERR", e);
            }
        }
    }

    let n = verification_queries.len();
    println!("\n  Verification: {found_count}/{n} ({:.0}%)", found_count as f64 / n as f64 * 100.0);
    assert!(found_count >= 2, "Multi-round recall too low: {found_count}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B15: CSC Contradiction Detection — 20 cases with causal chain contradictions
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b15_csc_contradiction_detection() {
    use plico::memory::contradiction::{
        build_context, RuleBasedClassifier, ContradictionClassifier,
    };
    println!("\n═══ B15: CSC Contradiction Detection ═══");

    let cases: Vec<(&str, &str, bool, &str)> = vec![
        ("Deploy cadence is weekly on Monday", "Deploy cadence is bi-weekly on Thursday", true, "schedule conflict"),
        ("The API rate limit is 1000 req/min", "The API rate limit is 500 req/min", true, "number conflict"),
        ("Rust is the primary backend language", "Rust is the primary backend language", false, "identical"),
        ("Authentication uses JWT tokens", "Authentication uses session cookies", true, "method conflict"),
        ("The database is PostgreSQL 15", "The database is PostgreSQL 16", true, "version conflict"),
        ("Team meeting is at 10am", "Team standup is at 10am", false, "different meetings"),
        ("Cache TTL is 5 minutes", "Cache TTL is 1 hour", true, "duration conflict"),
        ("Code reviews are required", "Code reviews are recommended", true, "policy conflict"),
        ("The frontend uses React 18", "The frontend uses React 19", true, "version conflict"),
        ("Logs are stored for 30 days", "Logs are stored for 90 days", true, "duration conflict"),
        ("Python 3.11 is the minimum version", "Python 3.12 is the minimum version", true, "version conflict"),
        ("The sky is blue", "Rust is a programming language", false, "unrelated"),
        ("Backups run daily at 2am", "Backups run daily at 2am UTC", false, "compatible"),
        ("Max file size is 10MB", "Max file size is 50MB", true, "limit conflict"),
        ("TLS 1.2 is required", "TLS 1.3 is required", true, "version conflict"),
        ("CI runs on GitHub Actions", "CI runs on GitLab CI", true, "platform conflict"),
        ("Default branch is main", "Default branch is master", true, "name conflict"),
        ("Tests must pass before merge", "Tests should pass before merge", true, "requirement conflict"),
        ("The team has 5 members", "The team has 8 members", true, "count conflict"),
        ("Release cycle is monthly", "Release cycle is quarterly", true, "frequency conflict"),
    ];

    let classifier = RuleBasedClassifier;
    let embedding_provider = make_embedding_provider();
    let has_embeddings = embedding_provider.is_some();

    let mut correct = 0;
    let total = cases.len();

    for (i, (old_text, new_text, expected_contradiction, desc)) in cases.iter().enumerate() {
        let mut old_entry = make_entry(&format!("old-{}", i), old_text, MemoryType::Semantic, &[]);
        let mut new_entry = make_entry(&format!("new-{}", i), new_text, MemoryType::Semantic, &[]);

        if let Some(ref provider) = embedding_provider {
            old_entry.embedding = provider.embed(old_text).ok().map(|r| r.embedding);
            new_entry.embedding = provider.embed(new_text).ok().map(|r| r.embedding);
        }

        let ctx = build_context(&old_entry, &new_entry, None);
        let result = classifier.classify(&old_entry, &new_entry, &ctx);

        let is_correct = result.is_contradiction == *expected_contradiction;
        if is_correct { correct += 1; }
        let mark = if is_correct { "✓" } else { "✗" };
        println!(
            "  {mark} [{i:>2}] {desc:<25} expected={:<5} got={:<5} conf={:.2} — {}",
            expected_contradiction, result.is_contradiction, result.confidence, result.evidence
        );
    }

    let accuracy = correct as f64 / total as f64 * 100.0;
    println!("\n  CSC Rule-Based Accuracy: {correct}/{total} ({accuracy:.0}%)");
    println!("  Embeddings used: {has_embeddings}");
    if has_embeddings {
        assert!(correct >= 10, "CSC accuracy too low with embeddings: {correct}/{total}");
    }
}

// ═══════════════════════════════════════════════════════════════════════
// B16: RFE Retrieval Fusion — pure cosine vs multi-signal
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b16_rfe_retrieval_fusion() {
    use plico::fs::retrieval_fusion::{RetrievalFusionEngine, FusionWeights, RetrievalQuery};

    println!("\n═══ B16: RFE Retrieval Fusion vs Pure Cosine ═══");

    let embedding_provider = match make_embedding_provider() {
        Some(p) => p,
        None => { println!("  {SKIP_MSG}"); return; }
    };

    let contents = vec![
        ("mem-rust-backend", "Rust is used for the backend API server", vec!["backend", "rust"], 10),
        ("mem-rust-frontend", "Rust/WASM is used for the frontend interactive components", vec!["frontend", "rust"], 2),
        ("mem-python-ml", "Python handles the ML pipeline and model training", vec!["ml", "python"], 5),
        ("mem-deploy-k8s", "Kubernetes orchestrates all backend microservices", vec!["backend", "deploy"], 8),
        ("mem-react-ui", "React powers the main user dashboard", vec!["frontend", "react"], 3),
    ];

    let mut entries: Vec<MemoryEntry> = Vec::new();
    for (id, content, tags, access) in &contents {
        let mut e = make_entry(id, content, MemoryType::Semantic, &tags.iter().map(|s| s.as_ref()).collect::<Vec<&str>>());
        e.access_count = *access;
        e.embedding = embedding_provider.embed(content).ok().map(|r| r.embedding);
        entries.push(e);
    }

    let queries: Vec<(&str, &str, Vec<String>)> = vec![
        ("What does the backend use?", "mem-rust-backend", vec!["backend".into()]),
        ("What frontend technology?", "mem-react-ui", vec!["frontend".into()]),
        ("How is ML done?", "mem-python-ml", vec!["ml".into()]),
    ];

    let engine = RetrievalFusionEngine::new(FusionWeights::default());
    let mut rfe_correct = 0;
    let mut cosine_correct = 0;
    let total = queries.len();

    for (query_text, expected_id, query_tags) in &queries {
        let query_emb = embedding_provider.embed(query_text).unwrap();

        // Pure cosine ranking
        let mut cosine_ranked: Vec<(&MemoryEntry, f32)> = entries.iter()
            .filter_map(|e| {
                e.embedding.as_ref().map(|emb| {
                    let sim = cosine_similarity(&query_emb.embedding, emb);
                    (e, sim)
                })
            })
            .collect();
        cosine_ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let cosine_top = cosine_ranked.first().map(|(e, _)| e.id.as_str()).unwrap_or("");
        if cosine_top == *expected_id { cosine_correct += 1; }

        // RFE ranking
        let rfe_query = RetrievalQuery {
            query_embedding: &query_emb.embedding,
            query_tags,
            query_memory_type: Some(MemoryType::Semantic),
            context_entry_id: None,
            bm25_scores: None,
        };
        let rfe_results = engine.rank(&entries, &rfe_query, None, 5);
        let rfe_top = rfe_results.first().map(|r| r.entry.id.as_str()).unwrap_or("");
        if rfe_top == *expected_id { rfe_correct += 1; }

        println!(
            "  Query: {:<30} expected={:<20} cosine_top={:<20} rfe_top={:<20}",
            query_text, expected_id, cosine_top, rfe_top
        );
    }

    println!("\n  Pure Cosine: {cosine_correct}/{total}");
    println!("  RFE Fusion:  {rfe_correct}/{total}");
    assert!(rfe_correct >= cosine_correct, "RFE should be at least as good as pure cosine");
}

// ═══════════════════════════════════════════════════════════════════════
// B17: MCE Consolidation Effect — 50 memories consolidation quality
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b17_mce_consolidation() {
    use plico::memory::consolidation::{MemoryConsolidationEngine, ConsolidationConfig, ConsolidationAction};

    println!("\n═══ B17: MCE Consolidation ═══");

    let embedding_provider = make_embedding_provider();

    let base_contents = vec![
        "Deploy cadence is weekly on Monday",
        "Deploy frequency is weekly Monday morning",
        "The API rate limit is 1000 requests per minute",
        "API rate limiting: 500 requests per minute max",
        "Rust is used for the backend services",
        "The backend is written in Rust",
        "Frontend uses React 18 with TypeScript",
        "Python handles data processing pipelines",
        "Docker containers are used in production",
        "Kubernetes orchestrates the container fleet",
    ];

    let t0 = Instant::now();
    let mut entries = Vec::new();
    for (i, content) in base_contents.iter().enumerate() {
        let mut e = make_entry(
            &format!("mce-{}", i), content, MemoryType::Semantic, &[]
        );
        if let Some(ref provider) = embedding_provider {
            e.embedding = provider.embed(content).ok().map(|r| r.embedding);
        }
        if i % 5 == 0 {
            e.access_count = 0;
            let age_days: u64 = 30;
            e.last_accessed = now_ms() - (age_days * 24 * 60 * 60 * 1000);
        }
        if i % 3 == 0 {
            e.access_count = 10;
        }
        entries.push(e);
    }

    let embed_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let engine = MemoryConsolidationEngine::new(ConsolidationConfig::default());
    let report = engine.consolidate(&entries);
    let consolidation_ms = t1.elapsed().as_millis();

    println!("  Entries scanned: {}", report.entries_scanned);
    println!("  Merges found: {}", report.merges);
    println!("  Contradictions found: {}", report.contradictions_found);
    println!("  Decays applied: {}", report.decays_applied);
    println!("  Boosts applied: {}", report.boosts_applied);
    println!("  Total actions: {}", report.actions.len());
    println!("  Embedding time: {embed_ms}ms");
    println!("  Consolidation time: {consolidation_ms}ms");

    for action in &report.actions {
        match action {
            ConsolidationAction::Merge { keep_id, remove_id, .. } => {
                println!("    MERGE: keep={keep_id} remove={remove_id}");
            }
            ConsolidationAction::Supersede { old_id, new_id, confidence, evidence } => {
                println!("    SUPERSEDE: old={old_id} new={new_id} conf={confidence:.2} — {evidence}");
            }
            ConsolidationAction::DecayConfidence { entry_id, new_importance } => {
                println!("    DECAY: {entry_id} → importance={new_importance}");
            }
            ConsolidationAction::BoostConfidence { entry_id, new_importance } => {
                println!("    BOOST: {entry_id} → importance={new_importance}");
            }
        }
    }

    assert!(report.entries_scanned == base_contents.len());
    assert!(report.decays_applied + report.boosts_applied > 0, "Should have at least some decay/boost actions");
}

// ═══════════════════════════════════════════════════════════════════════
// B18: Agent Profile Learning Curve — weight adaptation over queries
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b18_agent_profile_learning() {
    use plico::kernel::ops::agent_profile::{AgentProfile, SignalFeedback};
    use plico::fs::retrieval_router::QueryIntent;

    println!("\n═══ B18: Agent Profile Learning Curve ═══");

    let mut profile = AgentProfile::new("bench-agent");
    let initial_weights = profile.retrieval_weights.clone();

    println!("  Initial weights: semantic={:.3} causal={:.3} access={:.3} tag={:.3} temporal={:.3} type={:.3}",
        initial_weights.semantic, initial_weights.causal, initial_weights.access,
        initial_weights.tag, initial_weights.temporal, initial_weights.type_match);

    let num_queries = 100;
    for i in 0..num_queries {
        let intent = if i % 3 == 0 { QueryIntent::Factual } else { QueryIntent::Temporal };
        profile.record_query(intent, 50.0 + (i as f64) * 0.5);

        let feedback = vec![SignalFeedback {
            semantic_was_high: true,
            causal_was_high: i % 5 == 0,
            access_was_high: false,
            tag_was_high: true,
            temporal_was_high: i % 2 == 0,
            type_was_match: true,
            bm25_was_high: i % 3 == 0,
        }];
        profile.learn_weights(&feedback);

        if (i + 1) % 25 == 0 {
            let w = &profile.retrieval_weights;
            println!(
                "  After {:>3} queries: semantic={:.3} causal={:.3} access={:.3} tag={:.3} temporal={:.3} type={:.3}",
                i + 1, w.semantic, w.causal, w.access, w.tag, w.temporal, w.type_match
            );
        }
    }

    let final_weights = &profile.retrieval_weights;
    println!("\n  Dominant intent: {:?}", profile.dominant_intent());
    println!("  Total queries: {}", profile.total_queries);
    println!("  Avg retrieval latency: {:.1}ms", profile.avg_retrieval_latency_ms);

    // Semantic and tag should have increased relative to initial
    let semantic_grew = final_weights.semantic > initial_weights.semantic * 0.9;
    let tag_grew = final_weights.tag > initial_weights.tag * 0.9;
    println!("\n  Semantic weight grew: {semantic_grew}");
    println!("  Tag weight grew: {tag_grew}");
    let sum = final_weights.total();
    println!("  Weights sum: {sum:.4} (should be ~1.0)");
    assert!((sum - 1.0).abs() < 0.01, "Weights must normalize to 1.0 (got {sum})");
}

// ═══════════════════════════════════════════════════════════════════════
// B19: Real-World Context Ingestion — Cursor/Claude development context
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b19_real_world_context_ingestion() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => { println!("  {SKIP_MSG}"); return; }
    };

    println!("\n═══ B19: Real-World Context Ingestion (Cursor/Claude Dev Context) ═══\n");

    let agent_id = kernel.register_agent("ctx-bench-agent".to_string());

    // 20 real knowledge fragments extracted from Plico development transcripts.
    // Source: Cursor agent-transcripts/ + ~/.claude/projects/ JSONL sessions.
    let dev_knowledge: Vec<(&str, MemoryType, Vec<&str>)> = vec![
        // --- Semantic: architectural facts ---
        ("Plico uses Content-Addressed Storage with SHA-256 hashing for automatic deduplication of all stored objects",
         MemoryType::Semantic, vec!["cas", "storage", "architecture"]),
        ("The AIKernel has four memory tiers: Ephemeral for session cache, Working for task context, LongTerm for persistent knowledge, and Procedural for reusable workflows",
         MemoryType::Semantic, vec!["memory", "architecture", "layered"]),
        ("EmbeddingProvider and LlmProvider are trait abstractions that keep the kernel model-agnostic — any OpenAI-compatible server works",
         MemoryType::Semantic, vec!["embedding", "llm", "trait", "architecture"]),
        ("The CausalGraph tracks parent-child relationships between memories, enabling root cause analysis and impact propagation queries",
         MemoryType::Semantic, vec!["causal", "graph", "architecture"]),
        ("RetrievalFusionEngine combines six signals: semantic similarity, causal proximity, access affinity, tag overlap, temporal recency, and memory type matching",
         MemoryType::Semantic, vec!["retrieval", "rfe", "algorithm"]),
        ("CausalSemanticContradiction algorithm detects conflicts using embedding cosine distance, causal chain proximity, temporal divergence, and optional LLM classification",
         MemoryType::Semantic, vec!["contradiction", "csc", "algorithm"]),
        ("PromptRegistry supports versioned templates with global and agent-level overrides, compile-in defaults, and variable substitution via render method",
         MemoryType::Semantic, vec!["prompt", "registry", "configuration"]),

        // --- Episodic: bug fixes and events ---
        ("B6 recall_routed returned zero hits because remember() stores to Ephemeral tier without embeddings — fixed by switching to remember_long_term which creates embeddings for semantic search",
         MemoryType::Episodic, vec!["bug", "recall", "embedding", "fix"]),
        ("ConnectionRefusedError on port 8080 because llama-server instances were actually running on ports 18920 for LLM and 18921 for embeddings — environment variable LLAMA_URL must be set correctly",
         MemoryType::Episodic, vec!["bug", "llm", "connection", "fix"]),
        ("B3 distillation showed negative compression rate of -25.3% before prompt optimization — the LLM was generating summaries longer than input due to verbose prompt instructions",
         MemoryType::Episodic, vec!["bug", "distillation", "prompt", "optimization"]),
        ("MemoryConsolidationEngine test failed with unresolved MemoryContent import — needed to add use statement specifically inside cfg(test) module since it was unused in non-test code paths",
         MemoryType::Episodic, vec!["bug", "test", "import", "fix"]),
        ("Intent classification rules engine misclassified temporal queries containing 'after' keyword when combined with 'why' — LLM correctly handled these as multi_hop queries",
         MemoryType::Episodic, vec!["bug", "intent", "classification"]),

        // --- Procedural: workflows and patterns ---
        ("To run real LLM benchmarks: set LLAMA_URL=http://127.0.0.1:18920 and EMBEDDING_API_BASE=http://127.0.0.1:18921, then cargo test --test real_llm_benchmark -- --nocapture --test-threads=1",
         MemoryType::Procedural, vec!["benchmark", "llm", "workflow"]),
        ("Pattern for Tokio runtime nesting: use try_current() plus block_in_place() to avoid 'cannot start a runtime from within a runtime' panic in daemon mode",
         MemoryType::Procedural, vec!["tokio", "runtime", "pattern"]),
        ("Debug auth issues: check token expiry, then timezone handling, then session store connectivity — this three-step sequence resolves 90% of authentication failures",
         MemoryType::Procedural, vec!["debug", "auth", "workflow"]),
        ("Memory distillation parallel optimization: group working memories by MemoryType, process each group in std::thread::scope for concurrent LLM summarization calls",
         MemoryType::Procedural, vec!["distillation", "parallel", "optimization"]),

        // --- Mixed: project-specific knowledge ---
        ("Gemma 4 26B-A4B MoE model with Q4_K_M quantization runs at 9.3 QPS for intent classification with average latency of 107ms and CV of 2.4% on NVIDIA GB10",
         MemoryType::Semantic, vec!["gemma", "performance", "benchmark"]),
        ("v5-small-retrieval embedding model produces 1024-dimensional vectors with batch throughput of 294 embeddings per second at 3.4ms per text",
         MemoryType::Semantic, vec!["embedding", "performance", "benchmark"]),
        ("AgentProfile tracks usage patterns via intent histogram and memory type preferences, adapting RetrievalFusionEngine weights through exponential moving average learning",
         MemoryType::Semantic, vec!["agent", "profile", "adaptive", "learning"]),
        ("MemoryConsolidationEngine performs four operations: semantic deduplication via embedding similarity, contradiction resolution using CSC, confidence decay for stale entries, and access-based enhancement",
         MemoryType::Semantic, vec!["consolidation", "mce", "algorithm"]),
    ];

    // Phase 1: Ingest all knowledge into kernel
    println!("  Phase 1: Ingesting {} knowledge fragments...", dev_knowledge.len());
    let t_ingest = Instant::now();
    for (i, (content, _mem_type, tags)) in dev_knowledge.iter().enumerate() {
        let tag_strings: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        match kernel.remember_long_term(&agent_id, "default", content.to_string(), tag_strings, 7) {
            Ok(_) => {},
            Err(e) => println!("    WARN: Failed to store item {i}: {e}"),
        }
    }
    let ingest_ms = t_ingest.elapsed().as_millis();
    println!("  Ingestion complete: {}ms ({:.1}ms/item)\n", ingest_ms, ingest_ms as f64 / dev_knowledge.len() as f64);

    // Phase 2: Recall with real development questions
    let recall_queries: Vec<(&str, &str, &str)> = vec![
        ("How does Plico store data?", "SHA-256", "CAS architecture"),
        ("What memory tiers does the kernel have?", "Ephemeral", "memory layers"),
        ("How does contradiction detection work?", "cosine", "CSC algorithm"),
        ("What caused the recall_routed bug?", "Ephemeral", "B6 bug"),
        ("How to run benchmarks with real LLM?", "LLAMA_URL", "benchmark workflow"),
        ("What is the embedding throughput?", "294", "embedding perf"),
        ("How does memory consolidation work?", "deduplication", "MCE operations"),
        ("What pattern fixes Tokio runtime nesting?", "block_in_place", "Tokio pattern"),
        ("How does the retrieval fusion engine rank results?", "six signals", "RFE algorithm"),
        ("What was the distillation compression problem?", "negative", "B3 prompt bug"),
    ];

    println!("  Phase 2: Recalling with {} development queries...\n", recall_queries.len());
    println!("  {:>3} {:<50} {:>8} {:>6} {}", "#", "Query", "Lat(ms)", "Found", "Top result preview");
    println!("  {}", "-".repeat(110));

    let mut found_count = 0;
    let mut total_lat_ms = 0u128;
    for (i, (query, keyword, desc)) in recall_queries.iter().enumerate() {
        let t0 = Instant::now();
        let results = kernel.recall_semantic(&agent_id, "default", query, 5);
        let lat = t0.elapsed().as_millis();
        total_lat_ms += lat;

        match results {
            Ok(entries) => {
                let found = entries.iter().any(|e|
                    e.content.display().to_string().to_lowercase().contains(&keyword.to_lowercase()));
                if found { found_count += 1; }

                let preview = entries.first()
                    .map(|e| {
                        let s = e.content.display().to_string();
                        if s.len() > 60 { format!("{}...", &s[..60]) } else { s }
                    })
                    .unwrap_or_else(|| "(empty)".into());

                println!("  {:>3} {:<50} {:>8} {:>6} {}",
                    i + 1, desc, lat, if found { "✓" } else { "✗" }, preview);
            }
            Err(e) => {
                println!("  {:>3} {:<50} {:>8} {:>6} ERR: {}", i + 1, desc, lat, "ERR", e);
            }
        }
    }

    let n = recall_queries.len();
    let accuracy = found_count as f64 / n as f64 * 100.0;
    let avg_lat = total_lat_ms as f64 / n as f64;

    println!("\n  ── B19 Results ──");
    println!("  Knowledge ingested: {} items in {}ms", dev_knowledge.len(), ingest_ms);
    println!("  Recall accuracy: {found_count}/{n} ({accuracy:.0}%)");
    println!("  Avg recall latency: {avg_lat:.1}ms");
    println!("  Data source: Cursor agent-transcripts + Claude Code sessions (~23K raw items, 20 curated)");
    assert!(found_count >= 5, "Real-world recall accuracy too low: {found_count}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B20: LongMemEval-aligned — Information Extraction + Multi-Session + Temporal
//      + Knowledge Update + Abstention (5 categories, ingest-then-query)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b20_longmemeval_suite() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(pair) => pair,
        None => { println!("{SKIP_MSG}"); return; }
    };
    println!("\n═══ B20: LongMemEval-Aligned Suite (5 categories, ingest-then-query) ═══\n");
    let agent_id = kernel.register_agent("longmem-agent".to_string());

    // ── Phase 1: Ingest — simulate multi-session chat history ──
    // Each "session" is a topic block with timestamps, mimicking LongMemEval_S structure.
    let sessions: Vec<(&str, Vec<&str>, &str)> = vec![
        // (session_date, messages, session_topic)
        ("2025-01-15", vec![
            "User told the assistant they recently moved to Portland, Oregon from Austin, Texas",
            "User mentioned they work as a senior systems engineer at Cloudflare",
            "User asked about good hiking trails near Portland and was recommended Forest Park",
        ], "relocation"),
        ("2025-02-03", vec![
            "User discussed their preference for Rust over Go for systems programming",
            "User mentioned their team is migrating from PostgreSQL to CockroachDB",
            "User said they prefer neovim with LazyVim configuration over VS Code",
        ], "tech_preferences"),
        ("2025-02-20", vec![
            "User said they adopted a rescue dog named Luna, a 3-year-old border collie mix",
            "User mentioned they run 5km every morning before work",
            "User talked about training for the Portland Marathon in October",
        ], "personal_life"),
        ("2025-03-10", vec![
            "User asked about database migration strategies and was told to use blue-green deployment",
            "User mentioned their CockroachDB migration hit a snag with foreign key constraints",
            "User shared they fixed the migration by using batch processing with 1000-row chunks",
        ], "db_migration"),
        ("2025-03-25", vec![
            "User said they got promoted to Staff Engineer at Cloudflare",
            "User mentioned their new role involves leading the edge computing platform team",
            "User discussed plans to present at KubeCon 2025 about edge-native databases",
        ], "career_update"),
        ("2025-04-05", vec![
            "User corrected earlier information: they now prefer Zed editor over neovim",
            "User explained the switch was because Zed has better Rust LSP integration",
            "User said they still use neovim for quick terminal edits",
        ], "preference_update"),
        ("2025-04-15", vec![
            "User mentioned Luna the dog completed basic obedience training",
            "User talked about signing up Luna for agility training classes",
            "User said their marathon training increased to 15km long runs on weekends",
        ], "personal_update"),
        ("2025-04-20", vec![
            "User discussed a production incident where edge cache invalidation failed",
            "User explained the root cause was a race condition in the distributed lock",
            "User shared they resolved it by switching to a consensus-based invalidation protocol",
        ], "incident"),
    ];

    let ingest_start = Instant::now();
    let mut ingested = 0;
    for (date, messages, topic) in &sessions {
        for msg in messages {
            let content = format!("[{}] {}", date, msg);
            let tags = vec![topic.to_string(), "longmemeval".to_string()];
            if kernel.remember_long_term(&agent_id, "default", content, tags, 7).is_ok() {
                ingested += 1;
            }
        }
    }
    let ingest_ms = ingest_start.elapsed().as_millis();
    println!("  Ingest: {} items in {}ms ({:.1}ms/item)\n",
        ingested, ingest_ms, ingest_ms as f64 / ingested as f64);

    // ── Phase 2: Query — 5 LongMemEval categories ──
    struct LMEQuery {
        category: &'static str,
        question: &'static str,
        expected_keyword: &'static str,
        should_abstain: bool,
    }

    let queries = vec![
        // Information Extraction (IE)
        LMEQuery { category: "IE", question: "What kind of dog does the user have?",
            expected_keyword: "Luna", should_abstain: false },
        LMEQuery { category: "IE", question: "Where does the user work?",
            expected_keyword: "Cloudflare", should_abstain: false },
        LMEQuery { category: "IE", question: "What city did the user move to?",
            expected_keyword: "Portland", should_abstain: false },
        // Multi-Session Reasoning (MR)
        LMEQuery { category: "MR", question: "What is the user training for and what daily exercise do they do?",
            expected_keyword: "marathon", should_abstain: false },
        LMEQuery { category: "MR", question: "What database technology is the user's team migrating to and what deployment strategy was recommended?",
            expected_keyword: "CockroachDB", should_abstain: false },
        // Temporal Reasoning (TR)
        LMEQuery { category: "TR", question: "What happened after the user moved to Portland regarding their career?",
            expected_keyword: "promoted", should_abstain: false },
        LMEQuery { category: "TR", question: "When did the user's database migration encounter problems?",
            expected_keyword: "March", should_abstain: false },
        // Knowledge Update (KU)
        LMEQuery { category: "KU", question: "What is the user's current preferred code editor?",
            expected_keyword: "Zed", should_abstain: false },
        LMEQuery { category: "KU", question: "What is the user's current role at their company?",
            expected_keyword: "Staff", should_abstain: false },
        // Abstention (ABS)
        LMEQuery { category: "ABS", question: "What programming language does the user's partner use?",
            expected_keyword: "", should_abstain: true },
        LMEQuery { category: "ABS", question: "What is the user's salary at Cloudflare?",
            expected_keyword: "", should_abstain: true },
    ];

    let mut category_results: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new();
    let mut total_query_ms: u128 = 0;

    println!("  {:>4} {:>4} {:<65} {:>8} {:>6}", "#", "Cat", "Question", "Latency", "Hit");
    println!("  {}", "-".repeat(95));

    for (i, q) in queries.iter().enumerate() {
        let t = Instant::now();
        let result = kernel.recall_semantic(&agent_id, "default", q.question, 5);
        let lat = t.elapsed().as_millis();
        total_query_ms += lat;

        let hit = match &result {
            Ok(entries) if q.should_abstain => {
                let any_relevant = entries.iter().any(|e| {
                    let s = e.content.display().to_string().to_lowercase();
                    s.contains("partner") || s.contains("salary") || s.contains("spouse")
                });
                !any_relevant
            }
            Ok(entries) => entries.iter().any(|e|
                e.content.display().to_string().to_lowercase()
                    .contains(&q.expected_keyword.to_lowercase())),
            Err(_) => false,
        };

        let (total, hits) = category_results.entry(q.category).or_insert((0, 0));
        *total += 1;
        if hit { *hits += 1; }

        let trunc_q: String = if q.question.len() > 62 {
            format!("{}...", &q.question[..62])
        } else {
            q.question.to_string()
        };
        println!("  {:>4} {:>4} {:<65} {:>6}ms {:>6}",
            i + 1, q.category, trunc_q, lat, if hit { "✓" } else { "✗" });
    }

    println!("\n  ── B20 Category Breakdown ──");
    let mut total_hits = 0;
    let mut total_qs = 0;
    for cat in &["IE", "MR", "TR", "KU", "ABS"] {
        if let Some((total, hits)) = category_results.get(cat) {
            let pct = *hits as f64 / *total as f64 * 100.0;
            println!("  {:<25} {}/{} ({:.0}%)", cat, hits, total, pct);
            total_hits += hits;
            total_qs += total;
        }
    }
    let overall_pct = total_hits as f64 / total_qs as f64 * 100.0;
    let avg_query_ms = total_query_ms as f64 / total_qs as f64;
    println!("  ─────────────────────────");
    println!("  Overall:                  {}/{} ({:.0}%)", total_hits, total_qs, overall_pct);
    println!("  Ingest time:              {}ms ({} items)", ingest_ms, ingested);
    println!("  Avg query latency:        {:.1}ms", avg_query_ms);
    println!("  Benchmark: LongMemEval-aligned (IE/MR/TR/KU/ABS)");

    assert!(total_hits >= 6, "LongMemEval suite accuracy too low: {total_hits}/{total_qs}");
}

// ═══════════════════════════════════════════════════════════════════════
// B21: LoCoMo-aligned — Single-Hop / Multi-Hop / Temporal / Adversarial QA
//      (ingest-then-query with separate timing)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b21_locomo_suite() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(pair) => pair,
        None => { println!("{SKIP_MSG}"); return; }
    };
    println!("\n═══ B21: LoCoMo-Aligned Suite (4 QA categories, ingest-then-query) ═══\n");
    let agent_id = kernel.register_agent("locomo-agent".to_string());

    // ── Phase 1: Ingest multi-session dialogue ──
    // Simulates LoCoMo's two-speaker conversation structure.
    let dialogue: Vec<(&str, &str, &str)> = vec![
        // (session, speaker, utterance)
        ("2025-01-10 session1", "Alice", "I just got back from my trip to Kyoto! The bamboo forest was incredible."),
        ("2025-01-10 session1", "Bob", "That sounds amazing! How long were you there?"),
        ("2025-01-10 session1", "Alice", "Two weeks. I also visited Nara and fed the deer. My favorite meal was at a tiny ramen shop called Ichiran in Shijo."),
        ("2025-01-20 session2", "Bob", "How's work going? Still at the biotech startup?"),
        ("2025-01-20 session2", "Alice", "Actually, I switched to a fintech company called Stripe last month. I'm working on their billing infrastructure team."),
        ("2025-01-20 session2", "Bob", "Wow, big change! Are you still doing Python mostly?"),
        ("2025-01-20 session2", "Alice", "No, I transitioned to Ruby on Rails for the backend. But I still use Python for data analysis on the side."),
        ("2025-02-05 session3", "Alice", "Guess what? I adopted two cats from the shelter. Named them Mochi and Kinako."),
        ("2025-02-05 session3", "Bob", "So cute! What breed are they?"),
        ("2025-02-05 session3", "Alice", "Mochi is a Scottish Fold and Kinako is a regular tabby. They're both about 1 year old."),
        ("2025-02-15 session4", "Bob", "Did you hear about the earthquake in Japan?"),
        ("2025-02-15 session4", "Alice", "Yes! I was so worried about Kyoto but my friends there said it was minor. The epicenter was in Hokkaido."),
        ("2025-02-15 session4", "Bob", "Glad everyone is safe. Are you planning to go back?"),
        ("2025-02-15 session4", "Alice", "Definitely, I'm planning a trip for Golden Week in May. This time I want to visit Okinawa too."),
        ("2025-03-01 session5", "Alice", "I signed up for a pottery class on weekends. Making my own matcha bowls!"),
        ("2025-03-01 session5", "Bob", "That's so cool, very Japanese-inspired!"),
        ("2025-03-01 session5", "Alice", "Totally. I also started learning Japanese with the Genki textbook. Currently on chapter 5."),
        ("2025-03-15 session6", "Bob", "How's Stripe treating you?"),
        ("2025-03-15 session6", "Alice", "Great! I just got a spot bonus for shipping the usage-based billing feature. My tech lead said it saved the team 3 months of work."),
        ("2025-03-15 session6", "Alice", "Oh, and I need to update you — Mochi got sick last week but she's fully recovered now after the vet visit."),
    ];

    let ingest_start = Instant::now();
    let mut ingested = 0;
    for (session, speaker, utterance) in &dialogue {
        let content = format!("[{}] {}: {}", session, speaker, utterance);
        let tags = vec!["locomo".to_string(), speaker.to_lowercase()];
        if kernel.remember_long_term(&agent_id, "default", content, tags, 6).is_ok() {
            ingested += 1;
        }
    }
    let ingest_ms = ingest_start.elapsed().as_millis();
    println!("  Ingest: {} turns in {}ms ({:.1}ms/turn)\n",
        ingested, ingest_ms, ingest_ms as f64 / ingested as f64);

    // ── Phase 2: Query — 4 LoCoMo QA categories ──
    struct LoCoQ {
        category: &'static str,
        question: &'static str,
        expected_keyword: &'static str,
    }

    let queries = vec![
        // Single-Hop (fact from one session)
        LoCoQ { category: "single", question: "What is the name of Alice's cats?",
            expected_keyword: "Mochi" },
        LoCoQ { category: "single", question: "Where does Alice currently work?",
            expected_keyword: "Stripe" },
        LoCoQ { category: "single", question: "What Japanese textbook is Alice using?",
            expected_keyword: "Genki" },
        // Multi-Hop (requires cross-session reasoning)
        LoCoQ { category: "multi", question: "What programming language did Alice switch to when she changed jobs?",
            expected_keyword: "Ruby" },
        LoCoQ { category: "multi", question: "What creative hobby did Alice start that relates to her Japan interest?",
            expected_keyword: "pottery" },
        // Temporal (time-based reasoning)
        LoCoQ { category: "temporal", question: "When is Alice planning her next trip to Japan?",
            expected_keyword: "May" },
        LoCoQ { category: "temporal", question: "What happened to Mochi recently?",
            expected_keyword: "sick" },
        // Adversarial (should not hallucinate)
        LoCoQ { category: "adversarial", question: "What is Bob's job title at Stripe?",
            expected_keyword: "" },
        LoCoQ { category: "adversarial", question: "How many children does Alice have?",
            expected_keyword: "" },
    ];

    let mut cat_results: std::collections::HashMap<&str, (usize, usize)> = std::collections::HashMap::new();
    let mut total_query_ms: u128 = 0;

    println!("  {:>4} {:>10} {:<58} {:>8} {:>6}", "#", "Cat", "Question", "Latency", "Hit");
    println!("  {}", "-".repeat(90));

    for (i, q) in queries.iter().enumerate() {
        let t = Instant::now();
        let result = kernel.recall_semantic(&agent_id, "default", q.question, 5);
        let lat = t.elapsed().as_millis();
        total_query_ms += lat;

        let hit = match &result {
            Ok(entries) if q.expected_keyword.is_empty() => {
                // Adversarial: check that no result directly answers the question
                let any_direct = entries.iter().any(|e| {
                    let s = e.content.display().to_string().to_lowercase();
                    s.contains("bob") && (s.contains("job") || s.contains("title") || s.contains("work"))
                        && s.contains("stripe")
                });
                !any_direct
            }
            Ok(entries) => entries.iter().any(|e|
                e.content.display().to_string().to_lowercase()
                    .contains(&q.expected_keyword.to_lowercase())),
            Err(_) => false,
        };

        let (total, hits) = cat_results.entry(q.category).or_insert((0, 0));
        *total += 1;
        if hit { *hits += 1; }

        let trunc_q: String = if q.question.len() > 55 {
            format!("{}...", &q.question[..55])
        } else {
            q.question.to_string()
        };
        println!("  {:>4} {:>10} {:<58} {:>6}ms {:>6}",
            i + 1, q.category, trunc_q, lat, if hit { "✓" } else { "✗" });
    }

    println!("\n  ── B21 Category Breakdown (LoCoMo-aligned) ──");
    let mut total_hits = 0;
    let mut total_qs = 0;
    for cat in &["single", "multi", "temporal", "adversarial"] {
        if let Some((total, hits)) = cat_results.get(cat) {
            let pct = *hits as f64 / *total as f64 * 100.0;
            println!("  {:<25} {}/{} ({:.0}%)", cat, hits, total, pct);
            total_hits += hits;
            total_qs += total;
        }
    }
    let overall_pct = total_hits as f64 / total_qs as f64 * 100.0;
    let avg_query_ms = total_query_ms as f64 / total_qs as f64;
    println!("  ─────────────────────────");
    println!("  Overall:                  {}/{} ({:.0}%)", total_hits, total_qs, overall_pct);
    println!("  Ingest time:              {}ms ({} turns)", ingest_ms, ingested);
    println!("  Avg query latency:        {:.1}ms", avg_query_ms);
    println!("  Benchmark: LoCoMo-aligned (single/multi/temporal/adversarial)");

    assert!(total_hits >= 5, "LoCoMo suite accuracy too low: {total_hits}/{total_qs}");
}

// ═══════════════════════════════════════════════════════════════════════
// B22: Scale Test — 500 entries, latency degradation curve,
//      ingest-then-query pipeline with separate timing
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b22_scale_500() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(pair) => pair,
        None => { println!("{SKIP_MSG}"); return; }
    };
    println!("\n═══ B22: Scale Test — 500 entries, latency degradation curve ═══\n");
    let agent_id = kernel.register_agent("scale-agent".to_string());

    // ── Phase 1: Ingest 500 diverse entries ──
    let domains = [
        "machine learning", "database systems", "web development", "operating systems",
        "cryptography", "networking", "compiler design", "distributed systems",
        "mobile development", "cloud computing", "devops", "security",
        "data engineering", "frontend frameworks", "backend architecture",
        "microservices", "containerization", "serverless", "edge computing", "IoT",
    ];
    let facts_per_domain = [
        "uses gradient descent for optimization",
        "implements B-tree indexing for fast lookups",
        "leverages virtual DOM for efficient rendering",
        "supports multi-threaded task scheduling",
        "applies RSA algorithm for asymmetric encryption",
        "uses TCP three-way handshake for reliable connections",
        "performs lexical analysis before parsing",
        "implements Raft consensus for leader election",
        "supports offline-first architecture patterns",
        "enables auto-scaling based on CPU utilization",
        "uses blue-green deployment for zero downtime",
        "implements role-based access control (RBAC)",
        "uses Apache Spark for large-scale processing",
        "supports component-based UI architecture",
        "implements event-driven microservices pattern",
        "uses service mesh for inter-service communication",
        "leverages container orchestration with Kubernetes",
        "uses function-as-a-service for event processing",
        "deploys CDN nodes for reduced latency",
        "supports MQTT protocol for device communication",
    ];
    let adjectives = ["advanced", "modern", "efficient", "scalable", "robust",
        "optimized", "production-grade", "enterprise", "lightweight", "high-performance"];

    let ingest_start = Instant::now();
    let mut ingested = 0;
    let mut ingest_checkpoints: Vec<(usize, u128)> = Vec::new();

    for i in 0..500 {
        let domain = domains[i % domains.len()];
        let fact = facts_per_domain[i % facts_per_domain.len()];
        let adj = adjectives[i % adjectives.len()];
        let session_num = i / 25 + 1;
        let content = format!(
            "[session-{}] The {} {} system {} with variant-{} configuration for project-{}",
            session_num, adj, domain, fact, i % 7, i % 50
        );
        let tags = vec![domain.to_string(), format!("session-{}", session_num)];
        if kernel.remember_long_term(&agent_id, "default", content, tags, 5).is_ok() {
            ingested += 1;
        }
        if (i + 1) % 100 == 0 {
            ingest_checkpoints.push((i + 1, ingest_start.elapsed().as_millis()));
        }
    }
    let total_ingest_ms = ingest_start.elapsed().as_millis();

    println!("  ── Ingest Curve ──");
    println!("  {:>6} {:>10} {:>12}", "Items", "Total ms", "ms/item");
    let mut prev_ms: u128 = 0;
    for (count, ms) in &ingest_checkpoints {
        let delta = ms - prev_ms;
        println!("  {:>6} {:>10} {:>12.1}", count, ms, delta as f64 / 100.0);
        prev_ms = *ms;
    }
    println!("  Total: {} items in {}ms ({:.1}ms/item)\n",
        ingested, total_ingest_ms, total_ingest_ms as f64 / ingested as f64);

    // ── Phase 2: Query at different scale points ──
    let scale_queries = [
        ("What system uses gradient descent for optimization?", "gradient descent"),
        ("Which project implements Raft consensus?", "Raft consensus"),
        ("What uses container orchestration with Kubernetes?", "Kubernetes"),
        ("Which architecture supports offline-first patterns?", "offline"),
        ("What implements role-based access control?", "RBAC"),
        ("What uses Apache Spark for processing?", "Spark"),
        ("Which system uses MQTT protocol?", "MQTT"),
        ("What applies RSA algorithm for encryption?", "RSA"),
        ("Which system supports event-driven microservices?", "event-driven"),
        ("What leverages virtual DOM for rendering?", "virtual DOM"),
    ];

    println!("  ── Query Latency at 500-entry scale ──");
    println!("  {:>4} {:<55} {:>8} {:>6}", "#", "Query", "Latency", "Hit");
    println!("  {}", "-".repeat(80));

    let mut total_query_ms: u128 = 0;
    let mut hits = 0;
    for (i, (q, keyword)) in scale_queries.iter().enumerate() {
        let t = Instant::now();
        let result = kernel.recall_semantic(&agent_id, "default", q, 5);
        let lat = t.elapsed().as_millis();
        total_query_ms += lat;

        let hit = result.as_ref().map(|entries| {
            entries.iter().any(|e|
                e.content.display().to_string().to_lowercase().contains(&keyword.to_lowercase()))
        }).unwrap_or(false);
        if hit { hits += 1; }

        let trunc_q: String = if q.len() > 52 { format!("{}...", &q[..52]) } else { q.to_string() };
        println!("  {:>4} {:<55} {:>6}ms {:>6}",
            i + 1, trunc_q, lat, if hit { "✓" } else { "✗" });
    }

    let n = scale_queries.len();
    let avg_query_ms = total_query_ms as f64 / n as f64;
    println!("\n  ── B22 Results ──");
    println!("  Scale:                    500 entries ingested");
    println!("  Ingest time:              {}ms ({:.1}ms/item)", total_ingest_ms, total_ingest_ms as f64 / 500.0);
    println!("  Query accuracy:           {}/{} ({:.0}%)", hits, n, hits as f64 / n as f64 * 100.0);
    println!("  Avg query latency:        {:.1}ms", avg_query_ms);
    println!("  Benchmark: Scale degradation test (LongMemEval_S-class)");

    assert!(hits >= 5, "Scale test accuracy too low at 500 entries: {hits}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B23: Real Cursor/Claude Context — Scale Ingestion (hundreds of entries)
//      with ingest-then-query timing
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b23_real_context_scale() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(pair) => pair,
        None => { println!("{SKIP_MSG}"); return; }
    };
    println!("\n═══ B23: Real Cursor/Claude Context — Scale Ingestion ═══\n");
    let agent_id = kernel.register_agent("realctx-agent".to_string());

    // Extract real knowledge from Cursor agent transcripts
    let transcript_dir = std::path::Path::new("/home/leo/.cursor/projects/home-leo-work-Plico/agent-transcripts");
    let claude_history = std::path::Path::new("/home/leo/.claude/history.jsonl");

    let mut knowledge_items: Vec<(String, Vec<String>)> = Vec::new();

    // Scan Cursor transcripts for extractable knowledge
    if transcript_dir.exists() {
        let mut files: Vec<_> = std::fs::read_dir(transcript_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        files.sort_by_key(|e| e.file_name());

        for dir_entry in files.iter().take(30) {
            let jsonl_path = dir_entry.path().join(
                format!("{}.jsonl", dir_entry.file_name().to_string_lossy())
            );
            if !jsonl_path.exists() { continue; }
            if let Ok(content) = std::fs::read_to_string(&jsonl_path) {
                let mut line_count = 0;
                for line in content.lines().take(200) {
                    if line.len() < 50 { continue; }
                    // Extract assistant messages containing code/architecture info
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(text) = val.get("message")
                            .or_else(|| val.get("content"))
                            .and_then(|v| v.as_str())
                        {
                            let t = text.trim();
                            if t.len() >= 80 && t.len() <= 500 {
                                if t.contains("Plico") || t.contains("kernel")
                                    || t.contains("memory") || t.contains("benchmark")
                                    || t.contains("embedding") || t.contains("retrieval")
                                    || t.contains("LLM") || t.contains("agent")
                                {
                                    knowledge_items.push((
                                        t.to_string(),
                                        vec!["cursor-transcript".to_string()],
                                    ));
                                    line_count += 1;
                                    if line_count >= 10 { break; }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Scan Claude history
    if claude_history.exists() {
        if let Ok(content) = std::fs::read_to_string(claude_history) {
            let mut count = 0;
            for line in content.lines() {
                if line.len() < 50 { continue; }
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                    if let Some(text) = val.get("message")
                        .or_else(|| val.get("content"))
                        .and_then(|v| v.as_str())
                    {
                        let t = text.trim();
                        if t.len() >= 80 && t.len() <= 500 {
                            knowledge_items.push((
                                t.to_string(),
                                vec!["claude-history".to_string()],
                            ));
                            count += 1;
                            if count >= 100 { break; }
                        }
                    }
                }
            }
        }
    }

    // If not enough real data, supplement with curated dev knowledge
    let curated = vec![
        "Plico is an AI-Native OS with kernel-level memory, built entirely in Rust for safety and performance",
        "The RetrievalFusionEngine now uses 7 signals: semantic, causal, access, tag, temporal, type_match, and bm25_keyword",
        "FusionWeights are Serialize/Deserialize enabled for persistence and runtime configuration of retrieval signal weights",
        "BM25 keyword search complements vector similarity for exact-term matching in hybrid retrieval pipelines",
        "CausalSemanticContradiction algorithm uses cosine distance threshold of 0.35 and near-identical early return at 0.98",
        "MemoryConsolidationEngine runs background sweeps for TTL expiry, importance decay, and redundancy deduplication",
        "AgentProfile tracks per-agent intent histograms and learns personalized FusionWeights via exponential moving average",
        "The prompt registry supports versioned templates with compile-in defaults and runtime agent-level overrides",
        "LongMemEval evaluates 5 abilities: information extraction, multi-session reasoning, temporal reasoning, knowledge updates, abstention",
        "LoCoMo benchmark tests single-hop, multi-hop, temporal, open-domain, and adversarial QA categories",
        "Edge cache invalidation race condition was resolved by switching to consensus-based invalidation protocol",
        "Database migration from PostgreSQL to CockroachDB uses blue-green deployment with 1000-row batch chunks",
        "BM25 index uses TREC/SIGIR standard k1=1.2 b=0.75 parameters with dynamic avgdl auto-adjustment",
        "Kernel recall_routed runs three-channel concurrent pipeline: intent classification + embedding + BM25 search",
        "Plico MCP server exposes 19 actions including session_start, put, get, search, hybrid, remember, recall",
        "VersionFeatures and version_supports were removed as compatibility code since the project is pre-release with only one API version",
        "Deprecated v9_metrics test file was deleted as it contained ESTIMATED metrics superseded by v11_metrics real measurements",
        "Retrieval router classifies queries into factual, temporal, multi_hop, preference, and aggregation intents",
        "Memory tiers are Ephemeral (L0), Working (L1), LongTerm (L2), with semantic embeddings only on L2",
        "The three-channel concurrent pipeline in recall_routed reduces end-to-end latency by overlapping independent operations",
    ];
    for item in &curated {
        knowledge_items.push((item.to_string(), vec!["curated-dev".to_string()]));
    }

    let total_items = knowledge_items.len();
    println!("  Extracted {} knowledge items (cursor: {}, claude: {}, curated: {})",
        total_items,
        knowledge_items.iter().filter(|(_, t)| t.contains(&"cursor-transcript".to_string())).count(),
        knowledge_items.iter().filter(|(_, t)| t.contains(&"claude-history".to_string())).count(),
        curated.len());

    // ── Phase 1: Ingest ──
    let ingest_start = Instant::now();
    let mut ingested = 0;
    for (content, tags) in &knowledge_items {
        if kernel.remember_long_term(&agent_id, "default", content.clone(), tags.clone(), 6).is_ok() {
            ingested += 1;
        }
    }
    let ingest_ms = ingest_start.elapsed().as_millis();
    println!("  Ingest: {} items in {}ms ({:.1}ms/item)\n",
        ingested, ingest_ms, ingest_ms as f64 / ingested.max(1) as f64);

    // ── Phase 2: Query with dev-relevant questions ──
    let queries: Vec<(&str, &str)> = vec![
        ("How does Plico's retrieval fusion work?", "signal"),
        ("What are the memory tiers in Plico?", "tier"),
        ("How is contradiction detection implemented?", "contradiction"),
        ("What benchmark standards does Plico target?", "LongMemEval"),
        ("How does the BM25 index complement vector search?", "BM25"),
        ("What concurrent pipeline does recall_routed use?", "concurrent"),
        ("How are FusionWeights learned per agent?", "weight"),
        ("What MCP tools does Plico expose?", "MCP"),
        ("What is the MemoryConsolidationEngine responsible for?", "consolidation"),
        ("How was the edge cache race condition resolved?", "consensus"),
    ];

    println!("  {:>4} {:<55} {:>8} {:>6}", "#", "Query", "Latency", "Hit");
    println!("  {}", "-".repeat(78));

    let mut total_query_ms: u128 = 0;
    let mut hits = 0;
    for (i, (q, keyword)) in queries.iter().enumerate() {
        let t = Instant::now();
        let result = kernel.recall_semantic(&agent_id, "default", q, 5);
        let lat = t.elapsed().as_millis();
        total_query_ms += lat;

        let hit = result.as_ref().map(|entries| {
            entries.iter().any(|e|
                e.content.display().to_string().to_lowercase().contains(&keyword.to_lowercase()))
        }).unwrap_or(false);
        if hit { hits += 1; }

        println!("  {:>4} {:<55} {:>6}ms {:>6}",
            i + 1, q, lat, if hit { "✓" } else { "✗" });
    }

    let n = queries.len();
    let avg_query_ms = total_query_ms as f64 / n as f64;
    println!("\n  ── B23 Results ──");
    println!("  Scale:                    {} items ingested ({} real + {} curated)",
        ingested, ingested - curated.len(), curated.len());
    println!("  Ingest time:              {}ms ({:.1}ms/item)", ingest_ms, ingest_ms as f64 / ingested.max(1) as f64);
    println!("  Query accuracy:           {}/{} ({:.0}%)", hits, n, hits as f64 / n as f64 * 100.0);
    println!("  Avg query latency:        {:.1}ms", avg_query_ms);
    println!("  Benchmark: Real dev context at scale");

    assert!(hits >= 5, "Real context scale accuracy too low: {hits}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B24: RFE 7-Signal Fusion Quality — validates BM25 integration
//      (unit-level with real embedding)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b24_rfe_7signal() {
    let (kernel, _dir) = match make_real_kernel() {
        Some(pair) => pair,
        None => { println!("{SKIP_MSG}"); return; }
    };
    println!("\n═══ B24: RFE 7-Signal Fusion Quality (BM25 integration) ═══\n");
    let agent_id = kernel.register_agent("rfe7-agent".to_string());

    // Ingest items with specific keywords that BM25 should match strongly
    let items = vec![
        ("Kubernetes pods use cgroups v2 for resource isolation and namespace separation", vec!["k8s", "containers"]),
        ("PostgreSQL MVCC uses transaction IDs and visibility maps for concurrent access", vec!["database", "postgres"]),
        ("Rust borrow checker enforces ownership rules at compile time preventing data races", vec!["rust", "safety"]),
        ("Neural network backpropagation computes gradients using the chain rule of calculus", vec!["ml", "training"]),
        ("HTTP/3 uses QUIC protocol over UDP for faster connection establishment", vec!["networking", "http"]),
        ("Git uses SHA-1 hashes for content-addressable storage of objects", vec!["git", "vcs"]),
        ("Docker containers share the host kernel but have isolated filesystem and network", vec!["docker", "containers"]),
        ("Redis uses single-threaded event loop with epoll for high-throughput key-value operations", vec!["redis", "cache"]),
        ("TLS 1.3 reduced handshake to one round trip improving connection latency", vec!["security", "tls"]),
        ("WebAssembly provides near-native execution speed in web browsers via stack-based VM", vec!["wasm", "web"]),
    ];

    let ingest_start = Instant::now();
    for (content, tags) in &items {
        let str_tags: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        let _ = kernel.remember_long_term(&agent_id, "default", content.to_string(), str_tags, 7);
    }
    let ingest_ms = ingest_start.elapsed().as_millis();

    // Queries designed to test BM25 keyword advantage
    let queries = vec![
        // Exact keyword match — BM25 should boost
        ("What protocol does HTTP/3 use?", "QUIC"),
        ("How does the Rust borrow checker work?", "ownership"),
        ("What hashing does Git use?", "SHA"),
        ("How does Redis achieve high throughput?", "event loop"),
        ("What is WebAssembly execution model?", "stack"),
        // Semantic similarity — embedding should lead
        ("How do containers isolate resources?", "cgroups"),
        ("How does the database handle concurrent transactions?", "MVCC"),
        ("How do neural networks learn from data?", "backpropagation"),
        ("How was TLS connection speed improved?", "round trip"),
        ("How does Docker differ from VMs?", "kernel"),
    ];

    println!("  Ingest: {} items in {}ms\n", items.len(), ingest_ms);
    println!("  {:>4} {:<55} {:>8} {:>6}", "#", "Query", "Latency", "Hit");
    println!("  {}", "-".repeat(78));

    let mut total_ms: u128 = 0;
    let mut hits = 0;
    for (i, (q, keyword)) in queries.iter().enumerate() {
        let t = Instant::now();
        let result = kernel.recall_semantic(&agent_id, "default", q, 3);
        let lat = t.elapsed().as_millis();
        total_ms += lat;

        let hit = result.as_ref().map(|entries| {
            entries.iter().any(|e|
                e.content.display().to_string().to_lowercase().contains(&keyword.to_lowercase()))
        }).unwrap_or(false);
        if hit { hits += 1; }

        println!("  {:>4} {:<55} {:>6}ms {:>6}",
            i + 1, q, lat, if hit { "✓" } else { "✗" });
    }

    let n = queries.len();
    println!("\n  ── B24 Results ──");
    println!("  RFE 7-signal accuracy:    {}/{} ({:.0}%)", hits, n, hits as f64 / n as f64 * 100.0);
    println!("  Avg query latency:        {:.1}ms", total_ms as f64 / n as f64);
    println!("  Ingest time:              {}ms", ingest_ms);
    println!("  Benchmark: RFE 7-signal fusion with BM25 keyword integration");

    assert!(hits >= 6, "RFE 7-signal accuracy too low: {hits}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B25: Real LongMemEval dataset (S setting, 500 questions)
//      Industry-standard benchmark with ingest-then-query pipeline
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b25_longmemeval_real() {
    if !is_real_backend() { println!("{SKIP_MSG}"); return; }
    let dataset_path = "benchmarks/datasets/LongMemEval/data/longmemeval_s_cleaned.json";
    if !std::path::Path::new(dataset_path).exists() {
        println!("  SKIP: LongMemEval dataset not found at {dataset_path}");
        return;
    }

    let llm = match make_llm_provider() { Some(p) => p, None => { println!("{SKIP_MSG}"); return; } };

    println!("\n═══════════════════════════════════════════════════════════════════");
    println!("  B25: LongMemEval Real Dataset (S setting, 500 questions)");
    println!("  Industry benchmark: https://github.com/xiaowu0162/LongMemEval");
    println!("═══════════════════════════════════════════════════════════════════\n");

    let raw = std::fs::read_to_string(dataset_path).expect("read dataset");
    let dataset: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("parse JSON");
    println!("  Dataset loaded: {} questions\n", dataset.len());

    let types_to_sample = [
        "single-session-user", "single-session-assistant", "single-session-preference",
        "temporal-reasoning", "knowledge-update", "multi-session",
    ];
    let samples_per_type = 10;

    let mut sampled: Vec<&serde_json::Value> = Vec::new();
    for qtype in &types_to_sample {
        let of_type: Vec<&serde_json::Value> = dataset.iter()
            .filter(|q| q["question_type"].as_str() == Some(qtype))
            .collect();
        let take = samples_per_type.min(of_type.len());
        sampled.extend_from_slice(&of_type[..take]);
    }
    println!("  Sampled {} questions ({} per type × {} types)\n",
        sampled.len(), samples_per_type, types_to_sample.len());

    let mut category_results: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    let mut total_ingest_ms: u128 = 0;
    let mut total_query_ms: u128 = 0;
    let mut total_judge_ms: u128 = 0;
    let mut total_ingested: usize = 0;

    println!("  {:>3} {:<24} {:<50} {:>7} {:>7} {:>6}",
        "#", "Type", "Question", "Ingest", "Query", "Hit");
    println!("  {}", "-".repeat(100));

    for (qi, q) in sampled.iter().enumerate() {
        let qtype = q["question_type"].as_str().unwrap_or("unknown");
        let question = q["question"].as_str().unwrap_or("");
        let expected = q["answer"].as_str().unwrap_or("");

        let sessions = match q["haystack_sessions"].as_array() {
            Some(s) => s,
            None => continue,
        };
        let dates = q["haystack_dates"].as_array();

        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LLM_BACKEND", "llama");
        std::env::set_var("EMBEDDING_BACKEND", "openai");
        let kernel = match plico::kernel::AIKernel::new(dir.path().to_path_buf()) {
            Ok(k) => k,
            Err(_) => continue,
        };
        let agent_id = kernel.register_agent("lme-agent".to_string());

        let ingest_t = Instant::now();
        let mut n_turns = 0;
        for (si, session) in sessions.iter().enumerate() {
            let turns = match session.as_array() { Some(t) => t, None => continue };
            let date_str = dates.and_then(|d| d.get(si))
                .and_then(|v| v.as_str()).unwrap_or("unknown");
            for turn in turns {
                let role = turn["role"].as_str().unwrap_or("user");
                let content = turn["content"].as_str().unwrap_or("");
                if content.len() < 5 { continue; }
                let mem_content = format!("[{}] {}: {}", date_str, role, content);
                let tags = vec![format!("session-{si}"), "longmemeval".to_string()];
                let _ = kernel.remember_long_term(&agent_id, "default", mem_content, tags, 5);
                n_turns += 1;
            }
        }
        let ingest_ms = ingest_t.elapsed().as_millis();
        total_ingest_ms += ingest_ms;
        total_ingested += n_turns;

        let query_t = Instant::now();
        let results = kernel.recall_semantic(&agent_id, "default", question, 10);
        let query_ms = query_t.elapsed().as_millis();
        total_query_ms += query_ms;

        let context: String = results.as_ref().map(|entries| {
            entries.iter().take(5)
                .map(|e| e.content.display().to_string())
                .collect::<Vec<_>>().join("\n")
        }).unwrap_or_default();

        let judge_t = Instant::now();
        let hit = if context.is_empty() {
            false
        } else {
            let kw_lower = expected.to_lowercase();
            let ctx_lower = context.to_lowercase();
            if ctx_lower.contains(&kw_lower) {
                true
            } else {
                let judge_prompt = format!(
                    "Given the following retrieved context:\n---\n{}\n---\n\nQuestion: {}\nExpected answer: {}\n\nDoes the retrieved context contain enough information to answer the question correctly? Reply ONLY 'yes' or 'no'.",
                    &context[..context.len().min(1500)], question, expected
                );
                llm_chat(&*llm, &judge_prompt)
                    .map(|r| r.trim().to_lowercase().starts_with("yes"))
                    .unwrap_or(false)
            }
        };
        let judge_ms = judge_t.elapsed().as_millis();
        total_judge_ms += judge_ms;

        let (total, hits) = category_results.entry(qtype.to_string()).or_insert((0, 0));
        *total += 1;
        if hit { *hits += 1; }

        let trunc_q: String = if question.len() > 48 { format!("{}...", &question[..48]) } else { question.to_string() };
        println!("  {:>3} {:<24} {:<50} {:>5}ms {:>5}ms {:>6}",
            qi + 1, qtype, trunc_q, ingest_ms, query_ms, if hit { "✓" } else { "✗" });
    }

    println!("\n  ══ B25 LongMemEval Category Breakdown ══");
    let mut grand_total = 0;
    let mut grand_hits = 0;
    for qtype in &types_to_sample {
        if let Some((total, hits)) = category_results.get(*qtype) {
            println!("  {:<30} {}/{} ({:.0}%)", qtype, hits, total, *hits as f64 / *total as f64 * 100.0);
            grand_total += total;
            grand_hits += hits;
        }
    }
    let overall = grand_hits as f64 / grand_total as f64 * 100.0;
    println!("  ──────────────────────────────");
    println!("  Overall:                       {}/{} ({:.1}%)", grand_hits, grand_total, overall);
    println!("  Total turns ingested:          {}", total_ingested);
    println!("  Total ingest time:             {}ms", total_ingest_ms);
    println!("  Avg ingest per question:       {:.0}ms", total_ingest_ms as f64 / sampled.len() as f64);
    println!("  Total query time:              {}ms", total_query_ms);
    println!("  Avg query latency:             {:.1}ms", total_query_ms as f64 / sampled.len() as f64);
    println!("  Total judge time:              {}ms", total_judge_ms);
    println!("  Benchmark: LongMemEval S-setting (ICLR 2025)");

    assert!(grand_hits >= sampled.len() / 4, "LongMemEval accuracy too low: {grand_hits}/{grand_total}");
}

// ═══════════════════════════════════════════════════════════════════════
// B26: Real LoCoMo dataset (10 conversations, 1986 QA)
//      Industry-standard benchmark with ingest-then-query pipeline
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b26_locomo_real() {
    if !is_real_backend() { println!("{SKIP_MSG}"); return; }
    let dataset_path = "benchmarks/datasets/LoCoMo/data/locomo10.json";
    if !std::path::Path::new(dataset_path).exists() {
        println!("  SKIP: LoCoMo dataset not found at {dataset_path}");
        return;
    }

    let llm = match make_llm_provider() { Some(p) => p, None => { println!("{SKIP_MSG}"); return; } };

    println!("\n═══════════════════════════════════════════════════════════════════");
    println!("  B26: LoCoMo Real Dataset (10 conversations, ACL 2024)");
    println!("  Industry benchmark: https://github.com/snap-research/LoCoMo");
    println!("═══════════════════════════════════════════════════════════════════\n");

    let raw = std::fs::read_to_string(dataset_path).expect("read dataset");
    let dataset: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("parse JSON");
    println!("  Conversations: {}", dataset.len());
    let total_qa: usize = dataset.iter()
        .map(|c| c["qa"].as_array().map(|a| a.len()).unwrap_or(0)).sum();
    println!("  Total QA pairs: {}\n", total_qa);

    let convs_to_test = 2.min(dataset.len());
    let mut category_results: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
    let mut total_ingest_ms: u128 = 0;
    let mut total_query_ms: u128 = 0;
    let mut total_judge_ms: u128 = 0;
    let mut total_ingested: usize = 0;
    let mut total_qa_tested: usize = 0;

    let category_names = ["unknown", "single-hop", "temporal", "common-sense", "multi-hop", "adversarial"];

    for ci in 0..convs_to_test {
        let conv = &dataset[ci];
        let conv_data = &conv["conversation"];
        let speaker_a = conv_data["speaker_a"].as_str().unwrap_or("A");
        let speaker_b = conv_data["speaker_b"].as_str().unwrap_or("B");

        println!("  ── Conversation {} ({} & {}) ──", ci + 1, speaker_a, speaker_b);

        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("LLM_BACKEND", "llama");
        std::env::set_var("EMBEDDING_BACKEND", "openai");
        let kernel = match plico::kernel::AIKernel::new(dir.path().to_path_buf()) {
            Ok(k) => k,
            Err(e) => { println!("  Kernel error: {e}"); continue; }
        };
        let agent_id = kernel.register_agent("locomo-agent".to_string());

        let ingest_t = Instant::now();
        let mut n_turns = 0;
        for si in 1..=50 {
            let session_key = format!("session_{si}");
            let date_key = format!("session_{si}_date_time");
            let session = match conv_data.get(&session_key) { Some(s) => s, None => break };
            let date = conv_data.get(&date_key).and_then(|v| v.as_str()).unwrap_or("unknown");

            let turns = match session.as_array() { Some(t) => t, None => continue };
            for turn in turns {
                let speaker = turn["speaker"].as_str().unwrap_or("unknown");
                let text = turn["text"].as_str().unwrap_or("");
                if text.len() < 5 { continue; }
                let content = format!("[{}] {}: {}", date, speaker, text);
                let tags = vec![format!("session-{si}"), "locomo".to_string()];
                let _ = kernel.remember_long_term(&agent_id, "default", content, tags, 5);
                n_turns += 1;
            }
        }
        let ingest_ms = ingest_t.elapsed().as_millis();
        total_ingest_ms += ingest_ms;
        total_ingested += n_turns;
        println!("  Ingested: {} turns in {}ms ({:.1}ms/turn)", n_turns, ingest_ms, ingest_ms as f64 / n_turns.max(1) as f64);

        let qas = match conv["qa"].as_array() { Some(q) => q, None => continue };
        let qa_sample_size = 50.min(qas.len());
        let step = if qas.len() > qa_sample_size { qas.len() / qa_sample_size } else { 1 };
        let sampled_qas: Vec<&serde_json::Value> = qas.iter().step_by(step).take(qa_sample_size).collect();

        println!("  Testing {} QA pairs (sampled from {})\n", sampled_qas.len(), qas.len());
        println!("  {:>3} {:<14} {:<52} {:>7} {:>5}", "#", "Category", "Question", "Query", "Hit");
        println!("  {}", "-".repeat(85));

        for (qi, qa) in sampled_qas.iter().enumerate() {
            let question = qa["question"].as_str().unwrap_or("");
            let expected = qa["answer"].as_str().unwrap_or("");
            let cat_id = qa["category"].as_u64().unwrap_or(0) as usize;
            let cat_name = category_names.get(cat_id).unwrap_or(&"unknown");

            let query_t = Instant::now();
            let results = kernel.recall_semantic(&agent_id, "default", question, 10);
            let query_ms = query_t.elapsed().as_millis();
            total_query_ms += query_ms;

            let context: String = results.as_ref().map(|entries| {
                entries.iter().take(5)
                    .map(|e| e.content.display().to_string())
                    .collect::<Vec<_>>().join("\n")
            }).unwrap_or_default();

            let judge_t = Instant::now();
            let hit = if context.is_empty() {
                false
            } else {
                let kw_lower = expected.to_lowercase();
                let ctx_lower = context.to_lowercase();
                if kw_lower.len() <= 3 || ctx_lower.contains(&kw_lower) {
                    ctx_lower.contains(&kw_lower)
                } else {
                    let judge_prompt = format!(
                        "Retrieved context:\n---\n{}\n---\nQuestion: {}\nExpected: {}\nDoes the context contain the answer? Reply ONLY 'yes' or 'no'.",
                        &context[..context.len().min(1500)], question, expected
                    );
                    llm_chat(&*llm, &judge_prompt)
                        .map(|r| r.trim().to_lowercase().starts_with("yes"))
                        .unwrap_or(false)
                }
            };
            let judge_ms = judge_t.elapsed().as_millis();
            total_judge_ms += judge_ms;

            let (total, hits) = category_results.entry(cat_name.to_string()).or_insert((0, 0));
            *total += 1;
            if hit { *hits += 1; }
            total_qa_tested += 1;

            let trunc_q: String = if question.len() > 50 { format!("{}...", &question[..50]) } else { question.to_string() };
            println!("  {:>3} {:<14} {:<52} {:>5}ms {:>5}",
                qi + 1, cat_name, trunc_q, query_ms, if hit { "✓" } else { "✗" });
        }
        println!();
    }

    println!("  ══ B26 LoCoMo Category Breakdown ══");
    let mut grand_total = 0;
    let mut grand_hits = 0;
    for cat in &category_names[1..] {
        if let Some((total, hits)) = category_results.get(*cat) {
            println!("  {:<20} {}/{} ({:.0}%)", cat, hits, total, *hits as f64 / *total as f64 * 100.0);
            grand_total += total;
            grand_hits += hits;
        }
    }
    let overall = if grand_total > 0 { grand_hits as f64 / grand_total as f64 * 100.0 } else { 0.0 };
    println!("  ──────────────────────────────");
    println!("  Overall:               {}/{} ({:.1}%)", grand_hits, grand_total, overall);
    println!("  Conversations tested:  {}", convs_to_test);
    println!("  QA pairs tested:       {}", total_qa_tested);
    println!("  Total turns ingested:  {}", total_ingested);
    println!("  Total ingest time:     {}ms", total_ingest_ms);
    println!("  Total query time:      {}ms", total_query_ms);
    println!("  Avg query latency:     {:.1}ms", total_query_ms as f64 / total_qa_tested.max(1) as f64);
    println!("  Total judge time:      {}ms", total_judge_ms);
    println!("  Benchmark: LoCoMo (ACL 2024, snap-research/LoCoMo)");
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
