//! Observability infrastructure for Plico kernel.
//!
//! Provides:
//! - CorrelationId: UUID-based request tracing
//! - KernelMetrics: atomic counters and latency histograms
//! - OperationTimer: RAII guard for measuring operation latency
//!
//! All metrics are in-memory (no external metrics server required).
//! Uses the existing `tracing` crate for structured logs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A correlation ID for distributed tracing.
/// Generated as a UUID v4 for uniqueness.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Generate a new unique correlation ID.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Parse from a string slice (for incoming requests).
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self(s.to_string())
    }

    /// Return the underlying string representation.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Latency histogram with fixed buckets (in microseconds).
///
/// Buckets: 100us, 500us, 1ms, 5ms, 10ms, 50ms, 100ms, 500ms, 1s, 5s, 10s
#[derive(Debug)]
pub struct LatencyHistogram {
    /// Count of observations in each bucket.
    /// Bucket indices: 0=100us, 1=500us, 2=1ms, 3=5ms, 4=10ms, 5=50ms,
    ///                 6=100ms, 7=500ms, 8=1s, 9=5s, 10=10s, 11=overflow
    buckets: [AtomicU64; 12],
    sum: AtomicU64,
    count: AtomicU64,
}

impl Clone for LatencyHistogram {
    fn clone(&self) -> Self {
        // Note: This creates a snapshot clone, not a deep clone of atomic state
        // For metrics collection, we typically snapshot rather than clone
        Self {
            buckets: [
                AtomicU64::new(self.buckets[0].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[1].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[2].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[3].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[4].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[5].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[6].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[7].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[8].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[9].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[10].load(Ordering::Relaxed)),
                AtomicU64::new(self.buckets[11].load(Ordering::Relaxed)),
            ],
            sum: AtomicU64::new(self.sum.load(Ordering::Relaxed)),
            count: AtomicU64::new(self.count.load(Ordering::Relaxed)),
        }
    }
}

impl LatencyHistogram {
    /// Bucket boundaries in microseconds.
    const BUCKET_BOUNDARIES: [u64; 11] = [
        100,     // 100us
        500,     // 500us
        1_000,   // 1ms
        5_000,   // 5ms
        10_000,  // 10ms
        50_000,  // 50ms
        100_000, // 100ms
        500_000, // 500ms
        1_000_000, // 1s
        5_000_000, // 5s
        10_000_000, // 10s
    ];

    pub fn new() -> Self {
        Self {
            buckets: Default::default(),
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Record a latency observation (in microseconds).
    pub fn record(&self, us: u64) {
        let bucket = Self::BUCKET_BOUNDARIES
            .iter()
            .position(|&b| us <= b)
            .unwrap_or(11); // overflow bucket

        self.buckets[bucket].fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(us, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get a snapshot of the histogram state.
    pub fn snapshot(&self) -> LatencySnapshot {
        let buckets: Vec<u64> = self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).collect();
        LatencySnapshot {
            buckets,
            sum_us: self.sum.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }

    /// Get the average latency in microseconds.
    pub fn avg_us(&self) -> Option<u64> {
        let count = self.count.load(Ordering::Relaxed);
        let sum = self.sum.load(Ordering::Relaxed);
        (count > 0).then_some(sum / count)
    }
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of a latency histogram.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LatencySnapshot {
    /// Bucket counts [100us, 500us, 1ms, 5ms, 10ms, 50ms, 100ms, 500ms, 1s, 5s, 10s, overflow]
    pub buckets: Vec<u64>,
    /// Sum of all observations in microseconds.
    pub sum_us: u64,
    /// Total observation count.
    pub count: u64,
}

impl LatencySnapshot {
    /// Get average latency in microseconds.
    pub fn avg_us(&self) -> Option<f64> {
        (self.count > 0).then_some(self.sum_us as f64 / self.count as f64)
    }

    /// Get p50 latency in microseconds (approximate).
    pub fn p50_us(&self) -> Option<u64> {
        self.percentile(0.5)
    }

    /// Get p99 latency in microseconds (approximate).
    pub fn p99_us(&self) -> Option<u64> {
        self.percentile(0.99)
    }

    fn percentile(&self, p: f64) -> Option<u64> {
        if self.count == 0 {
            return None;
        }
        let target = (self.count as f64 * p).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, &count) in self.buckets.iter().enumerate() {
            cumulative += count;
            if cumulative >= target {
                // Return the upper boundary of this bucket
                if i < LatencyHistogram::BUCKET_BOUNDARIES.len() {
                    return Some(LatencyHistogram::BUCKET_BOUNDARIES[i]);
                } else {
                    return Some(LatencyHistogram::BUCKET_BOUNDARIES.last().copied().unwrap_or(u64::MAX));
                }
            }
        }
        Some(LatencyHistogram::BUCKET_BOUNDARIES.last().copied().unwrap_or(u64::MAX))
    }
}

/// Operation counter for tracking operation counts by type.
#[derive(Debug)]
pub struct OpCounter {
    counts: Vec<AtomicU64>,
}

impl OpCounter {
    pub fn new(num_ops: usize) -> Self {
        Self {
            counts: (0..num_ops).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    pub fn increment(&self, op: usize) {
        if op < self.counts.len() {
            self.counts[op].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn get(&self, op: usize) -> u64 {
        self.counts.get(op).map(|c| c.load(Ordering::Relaxed)).unwrap_or(0)
    }

    pub fn snapshot(&self) -> Vec<u64> {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).collect()
    }
}

impl Clone for OpCounter {
    fn clone(&self) -> Self {
        Self {
            counts: self.counts.iter().map(|c| AtomicU64::new(c.load(Ordering::Relaxed))).collect(),
        }
    }
}

/// Operation type identifiers for metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum OpType {
    SemanticCreate = 0,
    SemanticRead = 1,
    SemanticUpdate = 2,
    SemanticDelete = 3,
    RememberWorking = 4,
    Recall = 5,
    RememberLongTerm = 6,
    KgAddNode = 7,
    KgAddEdge = 8,
    KgFindPaths = 9,
    HandleApiRequest = 10,
    // Batch operations (v15.0)
    BatchCreate = 11,
    BatchMemoryStore = 12,
    BatchSubmitIntent = 13,
    BatchQuery = 14,
    // Add new types above this line
    Count,
}

impl OpType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpType::SemanticCreate => "semantic_create",
            OpType::SemanticRead => "semantic_read",
            OpType::SemanticUpdate => "semantic_update",
            OpType::SemanticDelete => "semantic_delete",
            OpType::RememberWorking => "remember_working",
            OpType::Recall => "recall",
            OpType::RememberLongTerm => "remember_long_term",
            OpType::KgAddNode => "kg_add_node",
            OpType::KgAddEdge => "kg_add_edge",
            OpType::KgFindPaths => "kg_find_paths",
            OpType::HandleApiRequest => "handle_api_request",
            OpType::BatchCreate => "batch_create",
            OpType::BatchMemoryStore => "batch_memory_store",
            OpType::BatchSubmitIntent => "batch_submit_intent",
            OpType::BatchQuery => "batch_query",
            OpType::Count => "count", // sentinel value, not a real operation
        }
    }

    /// Total number of operation types.
    pub const NUM_OPS: usize = 15;
}

/// Kernel-level metrics collector.
///
/// Collects:
/// - Operation counts (per operation type)
/// - Latency histograms (per operation type)
pub struct KernelMetrics {
    /// Operation counters.
    counters: OpCounter,
    /// Latency histograms (one per operation type).
    histograms: Vec<LatencyHistogram>,
}

impl KernelMetrics {
    /// Create a new metrics collector.
    pub fn new() -> Self {
        let num_ops = OpType::NUM_OPS;
        Self {
            counters: OpCounter::new(num_ops),
            histograms: (0..num_ops).map(|_| LatencyHistogram::new()).collect(),
        }
    }

    /// Record an operation latency.
    pub fn record_latency(&self, op: OpType, duration: Duration) {
        let idx = op as usize;
        if idx < self.histograms.len() {
            self.histograms[idx].record(duration.as_micros() as u64);
        }
    }

    /// Increment the counter for an operation.
    pub fn increment_counter(&self, op: OpType) {
        self.counters.increment(op as usize);
    }

    /// Get a snapshot of all metrics.
    pub fn get_metrics(&self) -> MetricsSnapshot {
        let num_ops = OpType::NUM_OPS;
        let mut histograms = Vec::with_capacity(num_ops);
        let mut counters = Vec::with_capacity(num_ops);

        for i in 0..num_ops {
            histograms.push(self.histograms[i].snapshot());
            counters.push(self.counters.get(i));
        }

        // Map indices to OpType names
        let op_types: Vec<String> = (0..num_ops)
            .map(|i| {
                let op = match i {
                    0 => OpType::SemanticCreate,
                    1 => OpType::SemanticRead,
                    2 => OpType::SemanticUpdate,
                    3 => OpType::SemanticDelete,
                    4 => OpType::RememberWorking,
                    5 => OpType::Recall,
                    6 => OpType::RememberLongTerm,
                    7 => OpType::KgAddNode,
                    8 => OpType::KgAddEdge,
                    9 => OpType::KgFindPaths,
                    10 => OpType::HandleApiRequest,
                    _ => OpType::Count,
                };
                op.as_str().to_string()
            })
            .collect();

        MetricsSnapshot {
            op_types,
            counters,
            latencies: histograms,
        }
    }

    /// Reset all metrics (useful for testing).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

impl Default for KernelMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of all kernel metrics.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsSnapshot {
    /// Operation type names.
    pub op_types: Vec<String>,
    /// Operation counts (aligned with op_types).
    pub counters: Vec<u64>,
    /// Latency histograms (aligned with op_types).
    pub latencies: Vec<LatencySnapshot>,
}

impl MetricsSnapshot {
    /// Get the total operation count.
    pub fn total_ops(&self) -> u64 {
        self.counters.iter().sum()
    }

    /// Format as JSON string for logging.
    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// RAII guard for measuring operation latency.
///
/// Created by `KernelMetrics::start_timer()` or `OperationTimer::new()`.
/// Automatically records the elapsed time when dropped.
#[must_use]
pub struct OperationTimer<'a> {
    start: Instant,
    metrics: &'a KernelMetrics,
    op: OpType,
}

impl<'a> OperationTimer<'a> {
    /// Create a new timer and immediately start measuring.
    pub fn new(metrics: &'a KernelMetrics, op: OpType) -> Self {
        metrics.increment_counter(op);
        Self {
            start: Instant::now(),
            metrics,
            op,
        }
    }

    /// Get the elapsed duration without stopping the timer.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }
}

impl Drop for OperationTimer<'_> {
    fn drop(&mut self) {
        self.metrics.record_latency(self.op, self.start.elapsed());
    }
}

/// Context for passing correlation ID through a request.
#[derive(Debug, Clone, Default)]
pub struct ObservabilityContext {
    /// Correlation ID for distributed tracing.
    pub correlation_id: Option<CorrelationId>,
}

impl ObservabilityContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_correlation_id(cid: CorrelationId) -> Self {
        Self {
            correlation_id: Some(cid),
        }
    }
}

// ── F-5: SessionObserver ────────────────────────────────────────────────────

/// Session observation data — per-session cost, hit rate, and feedback aggregation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionObservation {
    pub session_id: String,
    pub agent_id: String,
    pub started_at_ms: u64,
    pub duration_ms: u64,
    pub total_tokens: u64,
    pub cache_hits: u32,
    pub cache_misses: u32,
    pub cache_hit_rate: f32,
    pub verifications_passed: u32,
    pub verifications_failed: u32,
    pub improvement_suggestions: Vec<String>,
}

impl SessionObservation {
    /// Generate improvement suggestions based on observation data.
    pub fn generate_suggestions(obs: &SessionObservation) -> Vec<String> {
        let mut suggestions = Vec::new();
        if obs.cache_hit_rate < 0.1 {
            suggestions.push("Cache hit rate <10%: consider registering common intents".into());
        }
        if obs.verifications_failed > 0 {
            suggestions.push(format!("{} verification failures: review tool call parameters", obs.verifications_failed));
        }
        if obs.total_tokens > 100_000 {
            suggestions.push("High token usage: consider using L1 context layer for better compression".into());
        }
        suggestions
    }
}

// ── F-6: HealthScoreDecomposition ─────────────────────────────────────────

/// Decomposed health score — breaks down overall health into actionable components.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecomposedHealth {
    pub overall: f32,
    pub cache_effectiveness: f32,
    pub feedback_loop_active: bool,
    pub cost_visibility: bool,
    pub actionable_items: Vec<HealthAction>,
}

/// A specific health action an agent can take.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HealthAction {
    pub component: String,
    pub severity: String,  // "critical" | "warning" | "info"
    pub suggestion: String,
}

impl DecomposedHealth {
    /// Calculate decomposed health from cache stats and other indicators.
    pub fn from_cache_and_profile(
        cache_stats: &crate::kernel::ops::prefetch_cache::IntentCacheStats,
        profile_len: usize,
    ) -> Self {
        let cache_hit_rate = if cache_stats.total_lookups > 0 {
            cache_stats.hits as f32 / cache_stats.total_lookups as f32
        } else {
            0.0
        };

        let cache_effectiveness = if cache_stats.entries > 0 {
            (cache_hit_rate * 0.6 + (cache_stats.entries as f32 / 64.0).min(1.0) * 0.4)
        } else {
            0.0
        };

        let feedback_loop_active = profile_len > 0;
        let cost_visibility = true; // Always true once cost ledger is integrated

        let mut overall = cache_effectiveness * 0.4;
        if feedback_loop_active { overall += 0.3; }
        if cost_visibility { overall += 0.3; }
        overall = overall.min(1.0);

        let mut actionable_items = Vec::new();

        if cache_hit_rate < 0.1 && cache_stats.total_lookups > 10 {
            actionable_items.push(HealthAction {
                component: "intent_cache".into(),
                severity: "warning".into(),
                suggestion: format!(
                    "Cache hit rate {:.1}% with {} lookups — warm up cache with repeated intents",
                    cache_hit_rate * 100.0, cache_stats.total_lookups
                ),
            });
        }

        if !feedback_loop_active {
            actionable_items.push(HealthAction {
                component: "feedback_loop".into(),
                severity: "warning".into(),
                suggestion: "No agent profile activity detected — use remember_long_term to build profile".into(),
            });
        }

        if cache_effectiveness < 0.3 {
            actionable_items.push(HealthAction {
                component: "prefetch".into(),
                severity: "info".into(),
                suggestion: "Cache effectiveness low — prefetch is warming but not yet hitting".into(),
            });
        }

        Self {
            overall,
            cache_effectiveness,
            feedback_loop_active,
            cost_visibility,
            actionable_items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlation_id_generation() {
        let id1 = CorrelationId::new();
        let id2 = CorrelationId::new();
        assert_ne!(id1, id2);
        assert_eq!(id1.as_str().len(), 36); // UUID v4 format
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
    fn test_latency_histogram_record() {
        let hist = LatencyHistogram::new();
        hist.record(500); // 500us
        hist.record(1_000); // 1ms
        hist.record(5_000); // 5ms

        let snap = hist.snapshot();
        assert_eq!(snap.count, 3);
        assert_eq!(snap.sum_us, 6_500);
    }

    #[test]
    fn test_latency_histogram_avg() {
        let hist = LatencyHistogram::new();
        hist.record(1000);
        hist.record(2000);
        hist.record(3000);

        let avg = hist.avg_us();
        assert_eq!(avg, Some(2000));
    }

    #[test]
    fn test_latency_histogram_p50() {
        let hist = LatencyHistogram::new();
        // Record 100 observations in the 1ms bucket
        for _ in 0..100 {
            hist.record(1_000);
        }

        let snap = hist.snapshot();
        let p50 = snap.p50_us();
        assert!(p50.is_some());
        assert_eq!(p50.unwrap(), 1_000); // Should be in the 1ms bucket
    }

    #[test]
    fn test_op_counter() {
        let counter = OpCounter::new(3);
        counter.increment(0);
        counter.increment(0);
        counter.increment(1);

        assert_eq!(counter.get(0), 2);
        assert_eq!(counter.get(1), 1);
        assert_eq!(counter.get(2), 0);
    }

    #[test]
    fn test_kernel_metrics_record_latency() {
        let metrics = KernelMetrics::new();
        metrics.record_latency(OpType::SemanticCreate, Duration::from_millis(5));
        metrics.increment_counter(OpType::SemanticCreate);

        let snap = metrics.get_metrics();
        let idx = OpType::SemanticCreate as usize;
        assert_eq!(snap.counters[idx], 1);
        assert_eq!(snap.latencies[idx].count, 1);
    }

    #[test]
    fn test_operation_timer() {
        let metrics = KernelMetrics::new();
        {
            let _timer = OperationTimer::new(&metrics, OpType::SemanticRead);
            std::thread::sleep(Duration::from_micros(100));
        }

        let snap = metrics.get_metrics();
        let idx = OpType::SemanticRead as usize;
        assert_eq!(snap.counters[idx], 1);
        assert_eq!(snap.latencies[idx].count, 1);
        // Latency should be at least 100us
        assert!(snap.latencies[idx].sum_us >= 100);
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
    fn test_observability_context() {
        let ctx = ObservabilityContext::new();
        assert!(ctx.correlation_id.is_none());

        let id = CorrelationId::new();
        let ctx_with_id = ObservabilityContext::with_correlation_id(id.clone());
        assert_eq!(ctx_with_id.correlation_id, Some(id));
    }

    #[test]
    fn test_decomposed_health_all_green() {
        use crate::kernel::ops::prefetch_cache::IntentCacheStats;
        let cache_stats = IntentCacheStats {
            entries: 10,
            memory_bytes: 1024,
            hits: 80,
            total_lookups: 100,
        };
        let health = DecomposedHealth::from_cache_and_profile(&cache_stats, 5);
        assert!(health.overall > 0.5);
        assert!(health.feedback_loop_active);
    }

    #[test]
    fn test_decomposed_health_cache_cold() {
        use crate::kernel::ops::prefetch_cache::IntentCacheStats;
        let cache_stats = IntentCacheStats {
            entries: 0,
            memory_bytes: 0,
            hits: 0,
            total_lookups: 0,
        };
        let health = DecomposedHealth::from_cache_and_profile(&cache_stats, 0);
        assert!(health.overall < 0.5);
        assert!(!health.feedback_loop_active);
    }

    #[test]
    fn test_decomposed_health_generates_actions() {
        use crate::kernel::ops::prefetch_cache::IntentCacheStats;
        let cache_stats = IntentCacheStats {
            entries: 0,
            memory_bytes: 0,
            hits: 0,
            total_lookups: 50,  // cold cache with lookups = warning
        };
        let health = DecomposedHealth::from_cache_and_profile(&cache_stats, 0);
        assert!(!health.actionable_items.is_empty());
    }

    #[test]
    fn test_session_observation_suggestions() {
        let obs = SessionObservation {
            session_id: "s1".into(),
            agent_id: "a1".into(),
            started_at_ms: 0,
            duration_ms: 5000,
            total_tokens: 200_000,
            cache_hits: 0,
            cache_misses: 10,
            cache_hit_rate: 0.0,
            verifications_passed: 5,
            verifications_failed: 2,
            improvement_suggestions: vec![],
        };
        let suggestions = SessionObservation::generate_suggestions(&obs);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().any(|s| s.contains("verification")));
        assert!(suggestions.iter().any(|s| s.contains("token")));
    }
}

// ── Growth Report (F-13) ───────────────────────────────────────────────────────

use crate::api::semantic::{GrowthPeriod, GrowthReport};
use crate::kernel::event_bus::{EventBus, KernelEvent};
use crate::kernel::ops::session::SessionStore;
use crate::kernel::ops::prefetch::IntentPrefetcher;

/// Handle a query_growth_report request — generates a growth report for an agent.
///
/// This is a read-only statistical report showing:
/// - Session counts and token efficiency
/// - Intent cache hit rate
/// - Knowledge accumulation (memories stored/shared)
/// - KG growth (nodes/edges created)
///
/// OS presents the data; Agent decides whether to adjust strategy.
pub fn handle_query_growth_report(
    agent_id: &str,
    period: GrowthPeriod,
    session_store: &SessionStore,
    event_bus: &EventBus,
    prefetch: &IntentPrefetcher,
    kg: Option<&dyn crate::fs::KnowledgeGraph>,
) -> GrowthReport {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    // Calculate period cutoff time
    let period_ms: Option<u64> = match period {
        GrowthPeriod::Last7Days => Some(7 * 24 * 60 * 60 * 1000),
        GrowthPeriod::Last30Days => Some(30 * 24 * 60 * 60 * 1000),
        GrowthPeriod::AllTime => None,
    };
    let cutoff_ms = period_ms.map(|p| now_ms - p);

    // 1. Session statistics
    let completed_sessions = session_store.get_completed_sessions(agent_id, period_ms);
    // L-5: Also count active sessions (not just completed) for accurate growth reporting
    let active_sessions = session_store.get_active_sessions(agent_id, cutoff_ms);
    let sessions_total = (completed_sessions.len() + active_sessions.len()) as u64;

    // Calculate average tokens per session for first 5 and last 5 (from completed only)
    let token_averages = calculate_token_averages(&completed_sessions);

    // 2. Intent cache hit rate
    let cache_stats = prefetch.intent_cache_stats();
    let intent_cache_hit_rate = if cache_stats.total_lookups > 0 {
        cache_stats.hits as f32 / cache_stats.total_lookups as f32
    } else {
        0.0
    };

    // 3. Knowledge accumulation from EventBus
    let agent_events = event_bus.events_by_agent(agent_id);
    let relevant_events: Vec<_> = if let Some(cutoff) = cutoff_ms {
        agent_events.into_iter()
            .filter(|e| e.timestamp_ms >= cutoff)
            .collect()
    } else {
        agent_events
    };

    let mut memories_stored = 0u64;
    let mut memories_shared = 0u64;
    let mut procedures_learned = 0u64;

    for evt in &relevant_events {
        match &evt.event {
            KernelEvent::MemoryStored { agent_id: _, tier } => {
                memories_stored += 1;
                if tier == "procedural" {
                    procedures_learned += 1;
                }
            }
            KernelEvent::KnowledgeShared { .. } => {
                memories_shared += 1;
            }
            _ => {}
        }
    }

    // 4. KG growth (nodes/edges created in period)
    let (kg_nodes_created, kg_edges_created) = if let Some(kgraph) = kg {
        count_kg_growth(kgraph, agent_id, cutoff_ms)
    } else {
        (0, 0)
    };

    GrowthReport {
        agent_id: agent_id.to_string(),
        period,
        sessions_total,
        avg_tokens_per_session_first_5: token_averages.first_5_avg,
        avg_tokens_per_session_last_5: token_averages.last_5_avg,
        token_efficiency_ratio: token_averages.efficiency_ratio,
        intent_cache_hit_rate,
        memories_stored,
        memories_shared,
        procedures_learned,
        kg_nodes_created,
        kg_edges_created,
    }
}

/// Token averages for growth report calculation.
struct TokenAverages {
    first_5_avg: usize,
    last_5_avg: usize,
    efficiency_ratio: f32,
}

/// Calculate average tokens per session for first 5 and last 5 sessions.
fn calculate_token_averages(sessions: &[super::session::CompletedSession]) -> TokenAverages {
    if sessions.is_empty() {
        return TokenAverages {
            first_5_avg: 0,
            last_5_avg: 0,
            efficiency_ratio: 0.0,
        };
    }

    // Sort by creation time (oldest first)
    let mut sorted = sessions.to_vec();
    sorted.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms));

    // Calculate first 5 average
    let first_5: Vec<usize> = sorted.iter().take(5).map(|s| s.tokens_used).collect();
    let first_5_avg = if first_5.is_empty() {
        0
    } else {
        first_5.iter().sum::<usize>() / first_5.len()
    };

    // Calculate last 5 average
    let last_5: Vec<usize> = sorted.iter().rev().take(5).map(|s| s.tokens_used).collect();
    let last_5_avg = if last_5.is_empty() {
        0
    } else {
        last_5.iter().sum::<usize>() / last_5.len()
    };

    // Calculate efficiency ratio (last_5 / first_5, lower is better)
    // If first_5_avg is 0, we can't compute a meaningful ratio
    let efficiency_ratio = if first_5_avg > 0 {
        last_5_avg as f32 / first_5_avg as f32
    } else {
        // If first was 0 but last isn't, the ratio is undefined (infinite improvement)
        // We return 0.0 as a sentinel indicating "no baseline to compare against"
        // If both are 0, return 1.0 (equal, neither better nor worse)
        if last_5_avg > 0 { 0.0 } else { 1.0 }
    };

    TokenAverages {
        first_5_avg,
        last_5_avg,
        efficiency_ratio,
    }
}

/// Count KG nodes and edges created by an agent within a time period.
fn count_kg_growth(
    kg: &dyn crate::fs::KnowledgeGraph,
    agent_id: &str,
    cutoff_ms: Option<u64>,
) -> (u64, u64) {
    let nodes = match kg.list_nodes(agent_id, None) {
        Ok(n) => n,
        Err(_) => return (0, 0),
    };

    let nodes_created = if let Some(cutoff) = cutoff_ms {
        nodes.into_iter().filter(|n| n.created_at >= cutoff).count() as u64
    } else {
        nodes.len() as u64
    };

    let edges = match kg.list_edges(agent_id) {
        Ok(e) => e,
        Err(_) => return (nodes_created, 0),
    };

    let edges_created = if let Some(cutoff) = cutoff_ms {
        edges.into_iter().filter(|e| e.created_at >= cutoff).count() as u64
    } else {
        edges.len() as u64
    };

    (nodes_created, edges_created)
}
