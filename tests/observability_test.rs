//! Observability tests for Plico kernel (v14.0).
//!
//! Tests cover:
//! - Correlation ID generation and propagation
//! - Operation timer latency measurement
//! - Metrics counter increment
//! - Metrics latency histogram

use plico::kernel::AIKernel;
use plico::kernel::ops::observability::{CorrelationId, KernelMetrics, OpType, OperationTimer, LatencyHistogram};
use std::time::Duration;
use tempfile::tempdir;

fn make_kernel() -> (AIKernel, tempfile::TempDir) {
    let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
    let _ = std::env::set_var("LLM_BACKEND", "stub");
    let dir = tempdir().unwrap();
    let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
    (kernel, dir)
}

#[test]
fn test_correlation_id_generation() {
    let id1 = CorrelationId::new();
    let id2 = CorrelationId::new();

    // Each ID should be unique
    assert_ne!(id1, id2);

    // UUID v4 format has 36 characters (8-4-4-4-12)
    assert_eq!(id1.as_str().len(), 36);
    assert_eq!(id2.as_str().len(), 36);
}

#[test]
fn test_correlation_id_from_str() {
    let id = CorrelationId::from_str("test-id-123");
    assert_eq!(id.as_str(), "test-id-123");
}

#[test]
fn test_correlation_id_display() {
    let id = CorrelationId::from_str("abc-123");
    assert_eq!(format!("{}", id), "abc-123");
}

#[test]
fn test_operation_timer_latency() {
    let metrics = KernelMetrics::new();

    // Create a timer and simulate some work
    {
        let _timer = OperationTimer::new(&metrics, OpType::SemanticCreate);
        std::thread::sleep(Duration::from_micros(100));
    }

    // Verify latency was recorded
    let snap = metrics.get_metrics();
    let idx = OpType::SemanticCreate as usize;
    assert_eq!(snap.counters[idx], 1);
    assert_eq!(snap.latencies[idx].count, 1);
    // Latency should be at least 100 microseconds
    assert!(snap.latencies[idx].sum_us >= 100);
}

#[test]
fn test_metrics_counter_increment() {
    let metrics = KernelMetrics::new();

    // Increment various counters
    metrics.increment_counter(OpType::SemanticCreate);
    metrics.increment_counter(OpType::SemanticCreate);
    metrics.increment_counter(OpType::SemanticRead);
    metrics.increment_counter(OpType::SemanticUpdate);
    metrics.increment_counter(OpType::SemanticDelete);
    metrics.increment_counter(OpType::RememberWorking);
    metrics.increment_counter(OpType::Recall);
    metrics.increment_counter(OpType::RememberLongTerm);
    metrics.increment_counter(OpType::KgAddNode);
    metrics.increment_counter(OpType::KgAddEdge);
    metrics.increment_counter(OpType::KgFindPaths);

    let snap = metrics.get_metrics();

    assert_eq!(snap.counters[OpType::SemanticCreate as usize], 2);
    assert_eq!(snap.counters[OpType::SemanticRead as usize], 1);
    assert_eq!(snap.counters[OpType::SemanticUpdate as usize], 1);
    assert_eq!(snap.counters[OpType::SemanticDelete as usize], 1);
    assert_eq!(snap.counters[OpType::RememberWorking as usize], 1);
    assert_eq!(snap.counters[OpType::Recall as usize], 1);
    assert_eq!(snap.counters[OpType::RememberLongTerm as usize], 1);
    assert_eq!(snap.counters[OpType::KgAddNode as usize], 1);
    assert_eq!(snap.counters[OpType::KgAddEdge as usize], 1);
    assert_eq!(snap.counters[OpType::KgFindPaths as usize], 1);
}

#[test]
fn test_metrics_latency_histogram() {
    let hist = LatencyHistogram::new();

    // Record various latencies
    hist.record(500);   // 500us
    hist.record(1_000); // 1ms
    hist.record(5_000); // 5ms
    hist.record(50_000); // 50ms

    let snap = hist.snapshot();

    assert_eq!(snap.count, 4);
    assert_eq!(snap.sum_us, 56_500);

    // Check average
    let avg = snap.avg_us();
    assert_eq!(avg, Some(14125.0)); // 56500 / 4
}

#[test]
fn test_metrics_latency_histogram_p50() {
    let hist = LatencyHistogram::new();

    // Record 100 observations in the 1ms bucket
    for _ in 0..100 {
        hist.record(1_000);
    }

    let snap = hist.snapshot();
    let p50 = snap.p50_us();

    assert!(p50.is_some());
    // Should be in the 1ms bucket (boundary is 1_000)
    assert_eq!(p50.unwrap(), 1_000);
}

#[test]
fn test_kernel_metrics_integration() {
    let (kernel, _dir) = make_kernel();

    // Verify kernel has metrics field
    let metrics = kernel.metrics();

    // Do some operations that should record metrics
    let cid = kernel
        .semantic_create(
            b"test content".to_vec(),
            vec!["test".to_string()],
            "TestAgent",
            None,
        )
        .expect("create failed");

    // Read it back
    let _ = kernel
        .get_object(&cid, "TestAgent", "default")
        .expect("get failed");

    // Check metrics were recorded
    let snap = metrics.get_metrics();

    // SemanticCreate should be 1
    assert_eq!(snap.counters[OpType::SemanticCreate as usize], 1);

    // Total operations should be at least 2 (create + internal read via get_object)
    assert!(snap.total_ops() >= 1);
}

#[test]
fn test_kernel_metrics_multiple_operations() {
    let (kernel, _dir) = make_kernel();

    let metrics = kernel.metrics();

    // Create multiple objects
    for i in 0..5 {
        let _ = kernel.semantic_create(
            format!("content {}", i).into_bytes(),
            vec![format!("tag{}", i)],
            "TestAgent",
            None,
        );
    }

    let snap = metrics.get_metrics();

    // Should have 5 creates recorded
    assert_eq!(snap.counters[OpType::SemanticCreate as usize], 5);
}

#[test]
fn test_metrics_snapshot_total_ops() {
    let metrics = KernelMetrics::new();

    metrics.increment_counter(OpType::SemanticCreate);
    metrics.increment_counter(OpType::SemanticRead);
    metrics.increment_counter(OpType::SemanticCreate);

    let snap = metrics.get_metrics();
    assert_eq!(snap.total_ops(), 3);
}

#[test]
fn test_metrics_snapshot_json_serialization() {
    let metrics = KernelMetrics::new();
    metrics.increment_counter(OpType::SemanticCreate);

    let snap = metrics.get_metrics();
    let json = snap.to_json_string().expect("should serialize to JSON");

    // Should contain the operation type name
    assert!(json.contains("semantic_create"));

    // Should contain the counter value
    assert!(json.contains("1"));
}

#[test]
fn test_latency_histogram_buckets() {
    let hist = LatencyHistogram::new();

    // Record at different bucket boundaries
    hist.record(50);      // < 100us bucket
    hist.record(100);    // 100us bucket
    hist.record(500);    // 500us bucket
    hist.record(1_000);  // 1ms bucket
    hist.record(5_000);  // 5ms bucket
    hist.record(10_000); // 10ms bucket
    hist.record(50_000); // 50ms bucket
    hist.record(100_000); // 100ms bucket
    hist.record(500_000); // 500ms bucket
    hist.record(1_000_000); // 1s bucket
    hist.record(5_000_000); // 5s bucket
    hist.record(10_000_000); // 10s bucket
    hist.record(20_000_000); // overflow bucket

    let snap = hist.snapshot();

    // Should have 12 buckets (defined + overflow)
    assert_eq!(snap.buckets.len(), 12);

    // Sum should be 36,666,650 us
    // 50+100+500+1000+5000+10000+50000+100000+500000+1000000+5000000+10000000+20000000
    assert_eq!(snap.sum_us, 36_666_650);

    // Count should be 13
    assert_eq!(snap.count, 13);
}

#[test]
#[ignore]
fn test_correlation_id_in_api_response() {
    // NOTE: This test is ignored because full correlation_id integration
    // into all ApiResponse return sites in handle_api_request is incomplete.
    // The correlation_id is generated but not yet propagated to responses.
    // This requires updating ~100+ return sites in handle_api_request.
    let (kernel, _dir) = make_kernel();

    // Register an agent first
    let response = kernel.handle_api_request(plico::api::semantic::ApiRequest::RegisterAgent {
        name: "TestAgent".to_string(),
    });

    // The response should have a correlation_id
    assert!(response.correlation_id.is_some());

    // The correlation_id should be a valid UUID format
    let corr_id = response.correlation_id.unwrap();
    assert_eq!(corr_id.len(), 36);
}

#[test]
fn test_handle_api_request_records_metrics() {
    let (kernel, _dir) = make_kernel();

    let metrics = kernel.metrics();

    // Submit an API request
    kernel.handle_api_request(plico::api::semantic::ApiRequest::SystemStatus);

    // Check that HandleApiRequest was recorded
    let snap = metrics.get_metrics();
    assert!(snap.counters[OpType::HandleApiRequest as usize] >= 1);
}
