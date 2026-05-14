//! Performance Regression Tests
//!
//! Automated latency benchmarks for all perf-sensitive operations.
//! Each test measures latency percentiles and fails on regression.
//!
//! Design principles:
//! - Deterministic: stub backends, no external services
//! - Fast: completes in < 30s total
//! - Threshold-based: fails if latency exceeds baseline
//! - Catches regressions: deleting optimizations (e.g., binary_index) fails these tests
//!
//! ## Baseline thresholds (v46)
//!
//! | Operation | Dataset | P50 threshold | P95 threshold |
//! |-----------|---------|---------------|---------------|
//! | HNSW search | 100 vectors | < 1ms | < 5ms |
//! | HNSW search | 1000 vectors | < 5ms | < 15ms |
//! | HNSW search | 5000 vectors | < 10ms | < 30ms |
//! | HNSW upsert | single | < 1ms | < 5ms |
//! | HNSW delete | single | < 2ms | < 10ms |
//! | CAS write+read | single | < 20ms | < 50ms |
//! | Memory recall | 100 items | < 5ms | < 20ms |
//! | KG find_paths | 20-node star | < 10ms | < 30ms |

use std::sync::Arc;
use std::time::{Duration, Instant};

use plico::api::semantic::{ApiRequest, ContentEncoding};
use plico::fs::graph::types::{KGEdgeType, KGNodeType};
use plico::fs::search::hnsw::HnswBackend;
use plico::fs::search::{SearchFilter, SearchIndexMeta, SemanticSearch};
use plico::kernel::AIKernel;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_kernel() -> (Arc<AIKernel>, tempfile::TempDir) {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

fn sample_embedding(dim: usize, seed: f32) -> Vec<f32> {
    (0..dim)
        .map(|i| (seed * (i + 1) as f32).sin())
        .collect()
}

fn make_meta(cid: &str, tags: Vec<&str>) -> SearchIndexMeta {
    SearchIndexMeta {
        cid: cid.to_string(),
        tags: tags.into_iter().map(|s| s.to_string()).collect(),
        snippet: String::new(),
        content_type: "text".to_string(),
        created_at: 0,
        memory_type: None,
    }
}

/// Compute percentile from sorted durations.
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let idx = ((sorted.len() as f64 * p / 100.0) as usize).min(sorted.len() - 1);
    sorted[idx]
}

/// Run a closure N times, return sorted durations.
fn bench_n<F: FnMut()>(n: usize, mut f: F) -> Vec<Duration> {
    for _ in 0..3 {
        f();
    }
    let mut durations = Vec::with_capacity(n);
    for _ in 0..n {
        let start = Instant::now();
        f();
        durations.push(start.elapsed());
    }
    durations.sort();
    durations
}

/// Assert P50 and P95 are within thresholds.
fn assert_latency_ok(
    label: &str,
    durations: &[Duration],
    p50_max: Duration,
    p95_max: Duration,
) {
    let p50 = percentile(durations, 50.0);
    let p95 = percentile(durations, 95.0);
    let p99 = percentile(durations, 99.0);
    let avg: Duration = durations.iter().sum::<Duration>() / durations.len() as u32;

    eprintln!(
        "[perf] {}: avg={:.2?} P50={:.2?} P95={:.2?} P99={:.2?} (thresholds: P50<{:.2?} P95<{:.2?})",
        label, avg, p50, p95, p99, p50_max, p95_max
    );

    assert!(
        p50 <= p50_max,
        "[perf-regression] {} P50 {:?} exceeds threshold {:?}",
        label, p50, p50_max
    );
    assert!(
        p95 <= p95_max,
        "[perf-regression] {} P95 {:?} exceeds threshold {:?}",
        label, p95, p95_max
    );
}

fn call_api(kernel: &AIKernel, req: ApiRequest) -> plico::api::semantic::ApiResponse {
    kernel.handle_api_request(req)
}

fn register_agent(kernel: &AIKernel, name: &str) -> (String, String) {
    let resp = call_api(kernel, ApiRequest::RegisterAgent { name: name.to_string() });
    assert!(resp.ok, "agent registration failed: {:?}", resp.error);
    (resp.agent_id.expect("agent_id"), resp.token.expect("token"))
}

fn rand_idx() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

// ── HNSW Search Performance ──────────────────────────────────────────────────

fn setup_hnsw(dim: usize, n: usize) -> HnswBackend {
    let backend = HnswBackend::with_dim(dim);
    for i in 0..n {
        let cid = format!("cid_{:06}", i);
        let emb = sample_embedding(dim, i as f32 * 0.001 + 0.01);
        backend.upsert(&cid, &emb, make_meta(&cid, vec!["tag"]));
    }
    backend
}

#[test]
fn perf_hnsw_search_100() {
    let dim = 384;
    let backend = setup_hnsw(dim, 100);
    let query = sample_embedding(dim, 0.01);
    let filter = SearchFilter::default();

    let durations = bench_n(200, || {
        let results = backend.search(&query, 10, &filter);
        assert!(!results.is_empty());
    });

    assert_latency_ok("hnsw_search_100", &durations, Duration::from_millis(1), Duration::from_millis(5));
}

#[test]
fn perf_hnsw_search_1000() {
    let dim = 384;
    let backend = setup_hnsw(dim, 1000);
    let query = sample_embedding(dim, 0.01);
    let filter = SearchFilter::default();

    let durations = bench_n(200, || {
        let results = backend.search(&query, 10, &filter);
        assert!(!results.is_empty());
    });

    assert_latency_ok("hnsw_search_1000", &durations, Duration::from_millis(5), Duration::from_millis(15));
}

#[test]
fn perf_hnsw_search_5000() {
    let dim = 384;
    let backend = setup_hnsw(dim, 5000);
    let query = sample_embedding(dim, 0.01);
    let filter = SearchFilter::default();

    let durations = bench_n(100, || {
        let results = backend.search(&query, 10, &filter);
        assert!(!results.is_empty());
    });

    assert_latency_ok("hnsw_search_5000", &durations, Duration::from_millis(10), Duration::from_millis(30));
}

#[test]
fn perf_hnsw_search_with_filter_1000() {
    let dim = 384;
    let backend = setup_hnsw(dim, 1000);
    let query = sample_embedding(dim, 0.01);
    let filter = SearchFilter {
        require_tags: vec!["tag".to_string()],
        ..Default::default()
    };

    let durations = bench_n(200, || {
        let results = backend.search(&query, 10, &filter);
        assert!(!results.is_empty());
    });

    assert_latency_ok("hnsw_search_filtered_1000", &durations, Duration::from_millis(5), Duration::from_millis(15));
}

// ── HNSW Upsert/Delete Performance ──────────────────────────────────────────

#[test]
fn perf_hnsw_upsert() {
    let dim = 384;
    let backend = HnswBackend::with_dim(dim);
    let emb = sample_embedding(dim, 1.0);

    let durations = bench_n(500, || {
        let cid = format!("cid_{:06}", rand_idx());
        backend.upsert(&cid, &emb, make_meta(&cid, vec!["tag"]));
    });

    assert_latency_ok("hnsw_upsert", &durations, Duration::from_millis(1), Duration::from_millis(5));
}

#[test]
fn perf_hnsw_delete() {
    let dim = 384;
    let backend = HnswBackend::with_dim(dim);
    let emb = sample_embedding(dim, 1.0);

    for i in 0..1000 {
        let cid = format!("cid_{:06}", i);
        backend.upsert(&cid, &emb, make_meta(&cid, vec!["tag"]));
    }

    let mut idx = 0u64;
    let durations = bench_n(500, || {
        let cid = format!("cid_{:06}", idx % 1000);
        backend.upsert(&cid, &emb, make_meta(&cid, vec!["tag"]));
        backend.delete(&cid);
        idx += 1;
    });

    assert_latency_ok("hnsw_delete", &durations, Duration::from_millis(2), Duration::from_millis(10));
}

// ── Two-Stage Search Regression ──────────────────────────────────────────────
// This test specifically catches the binary_index deletion regression.
// With binary_index: two-stage (hamming coarse + cosine fine) should be fast.
// Without binary_index: falls back to usearch only — still works but different perf profile.

#[test]
fn perf_two_stage_search_5000() {
    let dim = 384;
    let backend = setup_hnsw(dim, 5000);
    let query = sample_embedding(dim, 0.01);
    let filter = SearchFilter::default();

    assert!(backend.len() >= 1000, "need >= 1000 entries for two-stage path");

    let durations = bench_n(100, || {
        let results = backend.search(&query, 10, &filter);
        assert!(!results.is_empty());
        assert!(results[0].score > 0.99, "exact match score should be ~1.0");
    });

    assert_latency_ok("two_stage_search_5000", &durations, Duration::from_millis(10), Duration::from_millis(30));
}

// ── CAS Write + Read Performance ─────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn perf_cas_write_read() {
    let (kernel, _dir) = make_kernel();
    let (agent_id, _) = register_agent(&kernel, "perf_agent");
    let content = "The quick brown fox jumps over the lazy dog. ".repeat(10);

    let durations = bench_n(100, || {
        let resp = call_api(&kernel, ApiRequest::Create {
            content: content.clone(),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["perf".to_string()],
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            api_version: None,
            intent: None,
        });
        assert!(resp.ok, "create failed: {:?}", resp.error);
        let cid = resp.cid.unwrap();

        let resp = call_api(&kernel, ApiRequest::Read {
            cid,
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
        });
        assert!(resp.ok, "read failed: {:?}", resp.error);
    });

    assert_latency_ok("cas_write_read", &durations, Duration::from_millis(20), Duration::from_millis(50));
}

// ── Memory Recall Performance ────────────────────────────────────────────────

#[test]
fn perf_memory_recall_100() {
    let (kernel, _dir) = make_kernel();
    let (agent_id, _) = register_agent(&kernel, "perf_agent");

    for i in 0..100 {
        call_api(&kernel, ApiRequest::Remember {
            agent_id: agent_id.clone(),
            content: format!("Memory item {}: important fact about topic {}", i, i % 10),
            tenant_id: None,
        });
    }

    let durations = bench_n(100, || {
        let resp = call_api(&kernel, ApiRequest::Recall {
            agent_id: agent_id.clone(),
            query: Some("important fact".to_string()),
            limit: Some(10),
            scope: None,
            tier: None,
        });
        assert!(resp.ok, "recall failed: {:?}", resp.error);
    });

    assert_latency_ok("memory_recall_100", &durations, Duration::from_millis(5), Duration::from_millis(20));
}

// ── Search Performance (full pipeline) ───────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn perf_search_pipeline_50() {
    let (kernel, _dir) = make_kernel();
    let (agent_id, _) = register_agent(&kernel, "perf_agent");

    for i in 0..50 {
        call_api(&kernel, ApiRequest::Create {
            content: format!("Document {} about rust programming", i),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["doc".to_string()],
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            api_version: None,
            intent: None,
        });
    }

    let durations = bench_n(100, || {
        let resp = call_api(&kernel, ApiRequest::Search {
            query: "rust programming".to_string(),
            agent_id: agent_id.clone(),
            tenant_id: None,
            agent_token: None,
            limit: Some(10),
            offset: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
            intent_context: None,
        });
        assert!(resp.ok, "search failed: {:?}", resp.error);
    });

    assert_latency_ok("search_pipeline_50", &durations, Duration::from_millis(20), Duration::from_millis(100));
}

// ── Batch Operations Performance ─────────────────────────────────────────────

#[test]
fn perf_batch_create_50() {
    let (kernel, _dir) = make_kernel();
    let (agent_id, _) = register_agent(&kernel, "perf_agent");

    let items: Vec<_> = (0..50)
        .map(|i| plico::api::dto::BatchCreateItem {
            content: format!("Batch document {}", i),
            content_encoding: ContentEncoding::Utf8,
            tags: vec!["batch".to_string()],
            intent: None,
        })
        .collect();

    let durations = bench_n(20, || {
        let resp = call_api(&kernel, ApiRequest::BatchCreate {
            agent_id: agent_id.clone(),
            items: items.clone(),
            tenant_id: None,
        });
        assert!(resp.ok, "batch_create failed: {:?}", resp.error);
    });

    assert_latency_ok("batch_create_50", &durations, Duration::from_millis(80), Duration::from_millis(200));
}

// ── KG Path Finding Performance ──────────────────────────────────────────────

#[test]
fn perf_kg_find_paths() {
    let (kernel, _dir) = make_kernel();
    let (agent_id, _) = register_agent(&kernel, "perf_agent");

    let hub = call_api(&kernel, ApiRequest::AddNode {
        agent_id: agent_id.clone(),
        label: "Hub".to_string(),
        node_type: KGNodeType::Entity,
        properties: serde_json::Value::Null,
        tenant_id: None,
    });
    assert!(hub.ok);
    let hub_id = hub.node_id.unwrap();

    let mut leaf_ids = Vec::new();
    for i in 0..20 {
        let resp = call_api(&kernel, ApiRequest::AddNode {
            agent_id: agent_id.clone(),
            label: format!("Leaf_{}", i),
            node_type: KGNodeType::Entity,
            properties: serde_json::Value::Null,
            tenant_id: None,
        });
        assert!(resp.ok);
        let leaf_id = resp.node_id.unwrap();
        leaf_ids.push(leaf_id.clone());

        call_api(&kernel, ApiRequest::AddEdge {
            agent_id: agent_id.clone(),
            src_id: hub_id.clone(),
            dst_id: leaf_id,
            edge_type: KGEdgeType::AssociatesWith,
            weight: None,
            tenant_id: None,
        });
    }

    let durations = bench_n(100, || {
        let resp = call_api(&kernel, ApiRequest::FindPaths {
            agent_id: agent_id.clone(),
            src_id: leaf_ids[0].clone(),
            dst_id: leaf_ids[1].clone(),
            max_depth: Some(4),
            weighted: false,
            tenant_id: None,
        });
        assert!(resp.ok, "find_paths failed: {:?}", resp.error);
        assert!(!resp.paths.as_ref().unwrap().is_empty(), "should find at least one path");
    });

    assert_latency_ok("kg_find_paths", &durations, Duration::from_millis(10), Duration::from_millis(30));
}
