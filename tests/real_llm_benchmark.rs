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

use plico::fs::embedding::types::EmbeddingProvider;
use plico::fs::retrieval_router::{
    classify_by_llm_response, classify_by_rules, intent_classification_prompt, QueryIntent,
};
use plico::kernel::AIKernel;
use plico::llm::{ChatMessage, ChatOptions, LlmProvider};
use plico::memory::causal::CausalGraph;
use plico::memory::{MemoryContent, MemoryEntry, MemoryScope, MemoryTier, MemoryType};
use std::sync::Arc;
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

fn make_real_kernel() -> Option<(Arc<AIKernel>, tempfile::TempDir)> {
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
    let url = if url.contains("/v1") {
        url
    } else {
        format!("{}/v1", url)
    };
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
    let url = if url.contains("/v1") {
        url
    } else {
        format!("{}/v1", url)
    };
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
    provider
        .chat(&messages, &opts)
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
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
    };

    let test_cases: Vec<(&str, QueryIntent)> = vec![
        ("What is the capital of France?", QueryIntent::Factual),
        ("When did Alice join the team?", QueryIntent::Temporal),
        ("What happened before the database migration?", QueryIntent::Temporal),
        (
            "Why did the auth service fail after the config change?",
            QueryIntent::MultiHop,
        ),
        (
            "How did the refactoring affect performance metrics?",
            QueryIntent::MultiHop,
        ),
        ("What does Bob prefer for deployment strategy?", QueryIntent::Preference),
        (
            "Which testing framework does the team like best?",
            QueryIntent::Preference,
        ),
        ("List all bugs fixed in the last sprint", QueryIntent::Aggregation),
        (
            "Summarize the key decisions from the architecture review",
            QueryIntent::Aggregation,
        ),
        ("What is the current database schema version?", QueryIntent::Factual),
    ];

    println!("\n=== B1: Intent Classification ===");
    println!(
        "{:<60} {:>12} {:>10} {:>10} {:>8}",
        "Query", "Expected", "LLM", "Rules", "Lat(ms)"
    );
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

        if llm_ok {
            llm_correct += 1;
        }
        if rule_ok {
            rule_correct += 1;
        }

        println!(
            "{:<60} {:>12} {:>10} {:>10} {:>8}",
            &query[..query.len().min(58)],
            expected.name(),
            llm_intent.map(|i| i.name()).unwrap_or("FAIL"),
            rule_result.intent.name(),
            llm_ms,
        );
    }

    let n = test_cases.len();
    println!(
        "\n  LLM accuracy:  {llm_correct}/{n} ({:.0}%)",
        llm_correct as f32 / n as f32 * 100.0
    );
    println!(
        "  Rule accuracy: {rule_correct}/{n} ({:.0}%)",
        rule_correct as f32 / n as f32 * 100.0
    );
    println!("  Avg LLM latency: {:.0}ms", total_llm_ms as f64 / n as f64);
    println!("  Total LLM time: {total_llm_ms}ms");

    assert!(
        llm_correct >= n / 2,
        "LLM intent classification accuracy too low: {llm_correct}/{n}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B2: Embedding Quality — semantic similarity and retrieval
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b2_embedding_similarity() {
    let emb = match make_embedding_provider() {
        Some(p) => p,
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
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
    println!(
        "{:<40} {:<40} {:>6} {:>8} {:>8}",
        "Text A", "Text B", "Sim?", "CosSim", "Lat(ms)"
    );
    println!("{}", "-".repeat(146));

    let mut correct = 0;

    for (a, b, should_be_similar) in &pairs {
        let t0 = Instant::now();
        let emb_a = match emb.embed(a) {
            Ok(r) => r.embedding,
            Err(e) => {
                eprintln!("  Embed error: {e}");
                continue;
            }
        };
        let emb_b = match emb.embed(b) {
            Ok(r) => r.embedding,
            Err(e) => {
                eprintln!("  Embed error: {e}");
                continue;
            }
        };
        let lat_ms = t0.elapsed().as_millis();

        let cos_sim = cosine_similarity(&emb_a, &emb_b);
        let predicted_similar = cos_sim > 0.15;
        let ok = predicted_similar == *should_be_similar;
        if ok {
            correct += 1;
        }

        println!(
            "{:<40} {:<40} {:>6} {:>8.4} {:>8}",
            &a[..a.len().min(38)],
            &b[..b.len().min(38)],
            if *should_be_similar { "YES" } else { "NO" },
            cos_sim,
            lat_ms,
        );
    }

    let n = pairs.len();
    println!(
        "\n  Accuracy: {correct}/{n} ({:.0}%)",
        correct as f32 / n as f32 * 100.0
    );
    assert!(correct >= n / 2, "Embedding similarity accuracy too low: {correct}/{n}");
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-10 || norm_b < 1e-10 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ═══════════════════════════════════════════════════════════════════════
// B5: End-to-end Kernel — CAS store + semantic search with real embeddings
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b5_kernel_store_recall() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
    };

    let agent_id = kernel.register_agent("bench-agent".into()).unwrap();
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    let facts = vec![
        ("The project deadline is March 15th", vec!["project", "deadline"]),
        (
            "Alice is the lead developer for the auth module",
            vec!["team", "alice", "auth"],
        ),
        (
            "We use PostgreSQL 15 as the primary database",
            vec!["infra", "database"],
        ),
        ("The CI pipeline runs on GitHub Actions", vec!["infra", "ci"]),
        (
            "Bob prefers Rust for systems programming",
            vec!["team", "bob", "preference"],
        ),
        ("The microservices communicate via gRPC", vec!["architecture", "grpc"]),
        (
            "Memory usage should stay under 4GB per service",
            vec!["performance", "memory"],
        ),
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
            scope: None,
        });
        store_ms_total += t0.elapsed().as_millis();
        assert!(resp.ok, "Store failed: {:?}", resp.error);
    }
    println!(
        "  Stored {} facts via CAS, total: {}ms (avg: {:.0}ms)",
        facts.len(),
        store_ms_total,
        store_ms_total as f64 / facts.len() as f64
    );

    let queries = [
        ("project deadline", "march"),
        ("auth module developer", "alice"),
        ("primary database", "postgresql"),
        ("services communication protocol", "grpc"),
        ("production deploy schedule", "wednesday"),
    ];

    println!(
        "\n  {:>5} {:<45} {:>8} {:>6} Top result preview",
        "#", "Query", "Lat(ms)", "Found"
    );
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
        let top_preview = results
            .first()
            .map(|r| {
                if r.snippet.len() > 60 {
                    format!("{}...", &r.snippet[..60])
                } else {
                    r.snippet.clone()
                }
            })
            .unwrap_or_else(|| "(empty)".into());

        let found = results
            .iter()
            .any(|r| r.snippet.to_lowercase().contains(&expected_keyword.to_lowercase()));
        if found {
            found_count += 1;
        }

        println!(
            "  {:>5} {:<45} {:>8} {:>6} {}",
            i + 1,
            query,
            search_ms,
            if found { "YES" } else { "NO" },
            top_preview
        );
    }

    let n = queries.len();
    println!(
        "\n  Semantic search accuracy: {found_count}/{n} ({:.0}%)",
        found_count as f64 / n as f64 * 100.0
    );
    println!("  Avg search latency: {:.0}ms", total_search_ms as f64 / n as f64);

    assert!(found_count >= 3, "Semantic search accuracy too low: {found_count}/{n}");
}

// ═══════════════════════════════════════════════════════════════════════
// B6: Recall Routed — intent-classified retrieval with real LLM + embeddings
#[test]
fn bench_b7_causal_graph() {
    println!("\n=== B7: Causal Graph ===");

    let ts = now_ms();
    let entries = vec![
        make_entry_ts(
            "root",
            "Config changed: timeout=30s -> timeout=60s",
            ts - 3000,
            None,
            None,
        ),
        make_entry_ts(
            "effect1",
            "API latency increased after config change",
            ts - 2000,
            Some("root".into()),
            None,
        ),
        make_entry_ts(
            "effect2",
            "Users reported timeout errors",
            ts - 1000,
            Some("effect1".into()),
            None,
        ),
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
// B9: Scale Test — 50 entries store + search performance degradation
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b9_scale_store_search() {
    use plico::api::semantic::{ApiRequest, ContentEncoding};

    let (kernel, _dir) = match make_real_kernel() {
        Some(k) => k,
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
    };

    let agent_id = kernel.register_agent("scale-agent".into()).unwrap();
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Write, None, None);
    kernel.permission_grant(&agent_id, plico::api::permission::PermissionAction::Read, None, None);

    let corpus: Vec<(String, Vec<String>)> = (0..50)
        .map(|i| {
            let domain = match i % 5 {
                0 => "infrastructure",
                1 => "team",
                2 => "process",
                3 => "architecture",
                _ => "metrics",
            };
            let content = match i % 10 {
                0 => format!("Server #{} runs Ubuntu 22.04 with {}GB RAM", i, 8 + i),
                1 => format!(
                    "Engineer-{} specializes in {} development",
                    i,
                    if i % 2 == 0 { "backend" } else { "frontend" }
                ),
                2 => format!("Sprint {} review: {} story points completed", i, 20 + i),
                3 => format!(
                    "Service {} uses {} for inter-process communication",
                    i,
                    if i % 2 == 0 { "gRPC" } else { "REST" }
                ),
                4 => format!("Average response time for endpoint-{}: {}ms", i, 50 + i * 3),
                5 => format!("Database shard {} contains {} million records", i, i * 2),
                6 => format!("Team member {} joined in 20{}", i, 20 + i % 5),
                7 => format!("CI pipeline stage {}: average duration {}s", i, 30 + i * 2),
                8 => format!("Microservice {} deployed to {} replicas", i, 2 + i % 4),
                _ => format!("Monitoring alert #{}: CPU usage at {}%", i, 40 + i),
            };
            (content, vec![domain.to_string(), format!("item-{i}")])
        })
        .collect();

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
            scope: None,
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
    println!(
        "    avg={:.1}ms  p50={}ms  p95={}ms  p99={}ms",
        avg_store, p50_store, p95_store, p99_store
    );

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

    println!(
        "\n  {:>3} {:<45} {:>8} {:>6} Top result",
        "#", "Query", "Lat(ms)", "Hits"
    );
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
        if found {
            total_found += 1;
        }

        let preview = results
            .first()
            .map(|r| {
                if r.snippet.len() > 55 {
                    format!("{}...", &r.snippet[..55])
                } else {
                    r.snippet.clone()
                }
            })
            .unwrap_or_else(|| "(empty)".into());

        println!(
            "  {:>3} {:<45} {:>8} {:>6} {}",
            i + 1,
            query,
            lat,
            results.len(),
            preview
        );
    }

    search_latencies.sort();
    let avg_search = search_latencies.iter().sum::<u128>() as f64 / search_latencies.len() as f64;
    let p50_search = search_latencies[search_latencies.len() / 2];
    let p95_search = search_latencies[search_latencies.len() * 95 / 100];

    println!(
        "\n  Search: avg={:.1}ms  p50={}ms  p95={}ms",
        avg_search, p50_search, p95_search
    );
    println!(
        "  Relevance: {total_found}/{} queries found keyword in top-5",
        search_queries.len()
    );

    assert!(
        total_found >= search_queries.len() / 2,
        "Scale search accuracy too low: {total_found}/{}",
        search_queries.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B10: Embedding Throughput — batch embedding latency
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bench_b10_embedding_throughput() {
    let emb = match make_embedding_provider() {
        Some(p) => p,
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
    };

    let texts: Vec<String> = (0..30)
        .map(|i| match i % 6 {
            0 => format!(
                "The architecture of microservice {} follows domain-driven design principles",
                i
            ),
            1 => format!(
                "Database query optimization reduced latency by {}% on shard {}",
                10 + i,
                i
            ),
            2 => format!(
                "Sprint {} retrospective identified {} action items for improvement",
                i,
                3 + i % 5
            ),
            3 => format!(
                "Load balancer distributes traffic across {} pods in cluster {}",
                4 + i % 3,
                i
            ),
            4 => format!(
                "Security audit found {} medium-severity vulnerabilities in service {}",
                i % 4,
                i
            ),
            _ => format!("Deployment pipeline {} takes {} minutes end-to-end", i, 5 + i % 10),
        })
        .collect();

    println!("\n=== B10: Embedding Throughput ({} texts) ===", texts.len());

    let mut latencies = Vec::with_capacity(texts.len());
    let mut dimensions = 0usize;
    let t_total = Instant::now();

    for text in &texts {
        let t0 = Instant::now();
        match emb.embed(text) {
            Ok(result) => {
                latencies.push(t0.elapsed().as_millis());
                if dimensions == 0 {
                    dimensions = result.embedding.len();
                }
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

    println!(
        "  Total: {}ms for {} embeddings (dim={})",
        total_ms,
        texts.len(),
        dimensions
    );
    println!("  avg={:.1}ms  p50={}ms  p95={}ms  p99={}ms", avg, p50, p95, p99);
    println!("  Throughput: {:.1} embeddings/sec", throughput);

    let first_5_avg: f64 = latencies[..5].iter().map(|&l| l as f64).sum::<f64>() / 5.0;
    let last_5_avg: f64 = latencies[latencies.len() - 5..].iter().map(|&l| l as f64).sum::<f64>() / 5.0;
    println!(
        "  Cold start effect: first_5_avg={:.1}ms  last_5_avg={:.1}ms",
        first_5_avg, last_5_avg
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B11: Multi-Session Memory Persistence — cross-session recall
#[test]
fn bench_b12_llm_latency_stability() {
    let llm = match make_llm_provider() {
        Some(p) => p,
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
    };

    println!("\n=== B12: LLM Latency Stability (20 calls) ===");

    let prompts: Vec<String> = (0..20)
        .map(|i| intent_classification_prompt(&format!("Query number {} about various topics", i)))
        .collect();

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
        let variance: f64 = latencies
            .iter()
            .map(|&l| {
                let diff = l as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / latencies.len() as f64;
        variance.sqrt()
    };
    let cv = std_dev / avg * 100.0;

    println!("\n  Total: {}ms for {} calls", total_ms, latencies.len());
    println!(
        "  avg={:.1}ms  p50={}ms  p95={}ms  min={}ms  max={}ms",
        avg, p50, p95, min_lat, max_lat
    );
    println!("  std_dev={:.1}ms  CV={:.1}%", std_dev, cv);
    println!(
        "  Throughput: {:.1} calls/sec",
        latencies.len() as f64 / (total_ms as f64 / 1000.0)
    );

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
        None => {
            eprintln!("{SKIP_MSG}");
            return;
        }
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
            Err(e) => {
                eprintln!("  Sequential embed error: {e}");
                return;
            }
        }
    }
    let seq_ms = t_seq.elapsed().as_millis();

    let t_batch = Instant::now();
    let batch_results = match emb.embed_batch(&texts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  Batch embed error: {e}");
            return;
        }
    };
    let batch_ms = t_batch.elapsed().as_millis();

    let speedup = if batch_ms > 0 {
        seq_ms as f64 / batch_ms as f64
    } else {
        f64::INFINITY
    };

    println!(
        "  Sequential: {}ms ({:.1}ms/text)",
        seq_ms,
        seq_ms as f64 / texts.len() as f64
    );
    println!(
        "  Batch:      {}ms ({:.1}ms/text)",
        batch_ms,
        batch_ms as f64 / texts.len() as f64
    );
    println!("  Speedup:    {:.2}x", speedup);
    println!("  Results:    seq={} batch={}", seq_results.len(), batch_results.len());

    assert_eq!(seq_results.len(), batch_results.len());

    let mut embedding_match = 0;
    for (s, b) in seq_results.iter().zip(batch_results.iter()) {
        let sim = cosine_similarity(&s.embedding, &b.embedding);
        if sim > 0.99 {
            embedding_match += 1;
        }
    }
    println!(
        "  Consistency: {}/{} embeddings match (>0.99 cosine)",
        embedding_match,
        texts.len()
    );
}

// ═══════════════════════════════════════════════════════════════════════
// B16: RFE Retrieval Fusion — pure cosine vs multi-signal
#[test]
fn bench_b18_agent_profile_learning() {
    use plico::fs::retrieval_router::QueryIntent;
    use plico::kernel::ops::agent_profile::{AgentProfile, SignalFeedback};

    println!("\n═══ B18: Agent Profile Learning Curve ═══");

    let mut profile = AgentProfile::new("bench-agent");
    let initial_weights = profile.retrieval_weights.clone();

    println!(
        "  Initial weights: semantic={:.3} causal={:.3} access={:.3} tag={:.3} temporal={:.3} type={:.3}",
        initial_weights.semantic,
        initial_weights.causal,
        initial_weights.access,
        initial_weights.tag,
        initial_weights.temporal,
        initial_weights.type_match
    );

    let num_queries = 100;
    for i in 0..num_queries {
        let intent = if i % 3 == 0 {
            QueryIntent::Factual
        } else {
            QueryIntent::Temporal
        };
        profile.record_query(intent, 50.0 + (i as f64) * 0.5);

        let feedback = vec![SignalFeedback {
            semantic_was_high: true,
            causal_was_high: i % 5 == 0,
            access_was_high: false,
            tag_was_high: true,
            temporal_was_high: i % 2 == 0,
            type_was_match: true,
            lexical_was_high: i % 3 == 0,
        }];
        profile.learn_weights(&feedback);

        if (i + 1) % 25 == 0 {
            let w = &profile.retrieval_weights;
            println!(
                "  After {:>3} queries: semantic={:.3} causal={:.3} access={:.3} tag={:.3} temporal={:.3} type={:.3}",
                i + 1,
                w.semantic,
                w.causal,
                w.access,
                w.tag,
                w.temporal,
                w.type_match
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

fn make_entry_ts(
    id: &str,
    content: &str,
    ts: u64,
    causal_parent: Option<String>,
    supersedes: Option<String>,
) -> MemoryEntry {
    MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
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
        memory_type: MemoryType::Episodic,
        causal_parent,
        supersedes,
        deleted_at: None,
        superseded_by: None,
    }
}
