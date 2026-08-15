//! Embedding Circuit Breaker — prevents cascading failures from embedding provider outages.
//!
//! Wraps an embedding provider with 3-state circuit breaker (Closed/Open/HalfOpen).
//! When the inner provider fails `failure_threshold` times consecutively, the circuit
//! opens and rejects embedding work. After `cooldown_ms`, exactly one probe is sent; success closes
//! the circuit, failure re-opens it.
//!
//! F-38: Embedding degradation awareness per Node 9 resilience design.

use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fs::embedding::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingInputOperation,
    EmbeddingProvider,
};

/// Circuit breaker states.
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Embedding circuit breaker wrapping a real provider without synthetic-vector fallback.
pub struct EmbeddingCircuitBreaker {
    inner: Arc<dyn EmbeddingProvider>,
    state: AtomicU8,
    failure_count: AtomicU32,
    failure_threshold: u32,
    last_failure_ms: AtomicU64,
    cooldown: Duration,
}

impl EmbeddingCircuitBreaker {
    pub fn new(inner: Arc<dyn EmbeddingProvider>, failure_threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            inner,
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            failure_threshold,
            last_failure_ms: AtomicU64::new(0),
            cooldown: Duration::from_millis(cooldown_ms),
        }
    }

    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    /// Returns a human-readable status string for observability.
    pub fn status(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            STATE_CLOSED => "active",
            STATE_OPEN => "degraded",
            STATE_HALF_OPEN => "probing",
            _ => "unknown",
        }
    }

    fn invoke(
        provider: &dyn EmbeddingProvider,
        operation: EmbeddingInputOperation,
        text: &str,
    ) -> Result<EmbedResult, EmbedError> {
        match operation {
            EmbeddingInputOperation::Generic => provider.embed(text),
            EmbeddingInputOperation::Query => provider.embed_query(text),
            EmbeddingInputOperation::Document => provider.embed_document(text),
        }
    }

    fn embed_operation(&self, operation: EmbeddingInputOperation, text: &str) -> Result<EmbedResult, EmbedError> {
        let state = self.state();
        let mut owns_probe = false;

        if state == STATE_OPEN {
            let elapsed = Self::now_ms() - self.last_failure_ms.load(Ordering::Relaxed);
            if elapsed >= self.cooldown.as_millis() as u64 {
                owns_probe = self
                    .state
                    .compare_exchange(STATE_OPEN, STATE_HALF_OPEN, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok();
                if !owns_probe {
                    return Err(EmbedError::ServerUnavailable(
                        "embedding circuit probe in progress".into(),
                    ));
                }
            } else {
                return Err(EmbedError::ServerUnavailable("embedding circuit open".into()));
            }
        } else if state == STATE_HALF_OPEN {
            return Err(EmbedError::ServerUnavailable(
                "embedding circuit probe in progress".into(),
            ));
        }

        if owns_probe {
            match Self::invoke(self.inner.as_ref(), operation, text) {
                Ok(result) => {
                    self.state.store(STATE_CLOSED, Ordering::Relaxed);
                    self.failure_count.store(0, Ordering::Relaxed);
                    tracing::info!(phase = "recovered", "embedding circuit breaker closed");
                    Ok(result)
                }
                Err(error) => {
                    self.state.store(STATE_OPEN, Ordering::Relaxed);
                    self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                    tracing::warn!(
                        error_category = error.category(),
                        phase = "half_open_probe",
                        "embedding circuit breaker probe failed"
                    );
                    Err(error)
                }
            }
        } else {
            match Self::invoke(self.inner.as_ref(), operation, text) {
                Ok(result) => {
                    self.failure_count.store(0, Ordering::Relaxed);
                    Ok(result)
                }
                Err(error) => {
                    if !matches!(error, EmbedError::InputTooLarge(_)) {
                        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                        if count >= self.failure_threshold {
                            self.state.store(STATE_OPEN, Ordering::Relaxed);
                            self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                            tracing::warn!(
                                error_category = error.category(),
                                failure_count = count,
                                phase = "open",
                                "embedding circuit breaker opened"
                            );
                        }
                    }
                    Err(error)
                }
            }
        }
    }
}

impl EmbeddingProvider for EmbeddingCircuitBreaker {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_operation(EmbeddingInputOperation::Generic, text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed(text)).collect()
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_operation(EmbeddingInputOperation::Query, text)
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_operation(EmbeddingInputOperation::Document, text)
    }

    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn raw_dimension(&self) -> usize {
        self.inner.raw_dimension()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        self.inner.builder_identity()
    }

    fn has_plico_adaptive_transform(&self) -> bool {
        self.inner.has_plico_adaptive_transform()
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Barrier, Mutex};

    #[derive(Clone, Default)]
    struct CapturedFields {
        fields: Arc<Mutex<Vec<(String, String)>>>,
        next_span: Arc<AtomicU64>,
    }

    struct FieldVisitor<'a>(&'a mut Vec<(String, String)>);

    impl tracing::field::Visit for FieldVisitor<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push((field.name().to_string(), format!("{value:?}")));
        }

        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.push((field.name().to_string(), value.to_string()));
        }
    }

    impl tracing::Subscriber for CapturedFields {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }

        fn register_callsite(&self, _metadata: &'static tracing::Metadata<'static>) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::sometimes()
        }

        fn max_level_hint(&self) -> Option<tracing::metadata::LevelFilter> {
            Some(tracing::metadata::LevelFilter::TRACE)
        }

        fn new_span(&self, attributes: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            attributes.record(&mut FieldVisitor(&mut self.fields.lock().unwrap()));
            tracing::span::Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
        }

        fn record(&self, _span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            values.record(&mut FieldVisitor(&mut self.fields.lock().unwrap()));
        }

        fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut FieldVisitor(&mut self.fields.lock().unwrap()));
        }

        fn enter(&self, _span: &tracing::span::Id) {}

        fn exit(&self, _span: &tracing::span::Id) {}
    }

    struct FailingProvider {
        calls: std::sync::atomic::AtomicU32,
        fail_for: std::sync::atomic::AtomicU32,
    }

    impl FailingProvider {
        fn new(fail_count: u32) -> Self {
            Self {
                calls: AtomicU32::new(0),
                fail_for: AtomicU32::new(fail_count),
            }
        }
    }

    impl EmbeddingProvider for FailingProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.fail_for.load(Ordering::Relaxed) > 0 {
                self.fail_for.fetch_sub(1, Ordering::Relaxed);
                Err(EmbedError::ServerUnavailable("test".into()))
            } else {
                Ok(EmbedResult::new(vec![0.1; 384], 10))
            }
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimension(&self) -> usize {
            384
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "failing",
                384,
                "failing-v1",
            ))
        }

        fn model_name(&self) -> String {
            "failing".into()
        }
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let inner = Arc::new(FailingProvider::new(5)); // Fail 5 times then succeed
        let cb = EmbeddingCircuitBreaker::new(Arc::clone(&inner) as Arc<dyn EmbeddingProvider>, 3, 100);

        // First 3 calls fail → circuit opens
        for _ in 0..3 {
            cb.embed("test").unwrap_err();
        }
        assert_eq!(cb.state(), STATE_OPEN);

        assert!(cb.embed_document("test").is_err());
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn test_circuit_breaker_recovery() {
        let inner = Arc::new(FailingProvider::new(1)); // Fail once then succeed
        let cb = EmbeddingCircuitBreaker::new(Arc::clone(&inner) as Arc<dyn EmbeddingProvider>, 1, 50);

        // First call fails → circuit opens
        cb.embed("test").unwrap_err();
        assert_eq!(cb.state(), STATE_OPEN);

        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(60));

        // Next call goes to HalfOpen → probe succeeds → closes
        let result = cb.embed("test");
        assert!(result.is_ok());
        assert_eq!(cb.state(), STATE_CLOSED);
    }

    #[test]
    fn test_circuit_breaker_stays_closed_on_success() {
        let inner = Arc::new(FailingProvider::new(0));
        let cb = EmbeddingCircuitBreaker::new(inner, 3, 100);

        for _ in 0..5 {
            assert!(cb.embed("test").is_ok());
        }
        assert_eq!(cb.state(), STATE_CLOSED);
    }

    #[test]
    fn test_circuit_breaker_resets_count_on_success() {
        let inner = Arc::new(FailingProvider::new(2));
        let cb = EmbeddingCircuitBreaker::new(inner, 3, 100);

        cb.embed("test").unwrap_err();
        cb.embed("test").unwrap_err();
        assert!(cb.embed("test").is_ok());
        assert_eq!(cb.state(), STATE_CLOSED);
    }

    #[test]
    fn test_circuit_breaker_embed_batch() {
        let inner = Arc::new(FailingProvider::new(0));
        let cb = EmbeddingCircuitBreaker::new(inner, 3, 100);

        let result = cb.embed_batch(&["hello", "world"]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 2);
    }

    #[test]
    fn test_circuit_breaker_embed_batch_opens() {
        let inner = Arc::new(FailingProvider::new(10));
        let cb = EmbeddingCircuitBreaker::new(inner, 2, 100);

        let _ = cb.embed_batch(&["a"]);
        let _ = cb.embed_batch(&["b"]);
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn test_circuit_breaker_dimension_delegation() {
        let inner = Arc::new(FailingProvider::new(0));
        let cb = EmbeddingCircuitBreaker::new(inner, 3, 100);
        assert_eq!(cb.dimension(), 384);
        assert_eq!(cb.raw_dimension(), 384);
        assert_eq!(cb.model_name(), "failing");
    }

    #[test]
    fn test_circuit_breaker_embed_query_and_document() {
        let inner = Arc::new(FailingProvider::new(0));
        let cb = EmbeddingCircuitBreaker::new(inner, 3, 100);
        assert!(cb.embed_query("test").is_ok());
        assert!(cb.embed_document("test").is_ok());
    }

    #[test]
    fn test_circuit_breaker_ignores_input_too_large() {
        struct TooLargeProvider;
        impl EmbeddingProvider for TooLargeProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                Err(EmbedError::InputTooLarge("2855 tokens > 2048 limit".into()))
            }
            fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                Err(EmbedError::InputTooLarge("batch too large".into()))
            }
            fn dimension(&self) -> usize {
                384
            }
            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                Ok(EmbeddingBuilderIdentity::test_deterministic(
                    "too-large",
                    384,
                    "too-large-v1",
                ))
            }
            fn model_name(&self) -> String {
                "too-large".into()
            }
        }

        let inner = Arc::new(TooLargeProvider);
        let cb = EmbeddingCircuitBreaker::new(inner, 1, 100); // threshold=1, would trip on any real failure

        // InputTooLarge should NOT trip the breaker even with threshold=1
        for _ in 0..5 {
            let _ = cb.embed("huge document");
        }
        assert_eq!(cb.state(), STATE_CLOSED, "InputTooLarge must not trip circuit breaker");

        // Same for batch
        for _ in 0..5 {
            let _ = cb.embed_batch(&["huge document"]);
        }
        assert_eq!(
            cb.state(),
            STATE_CLOSED,
            "InputTooLarge in batch must not trip circuit breaker"
        );
    }

    #[test]
    fn test_circuit_breaker_half_open_probe_fails() {
        let inner = Arc::new(FailingProvider::new(5));
        let cb = EmbeddingCircuitBreaker::new(inner, 1, 50);

        cb.embed("test").unwrap_err();
        assert_eq!(cb.state(), STATE_OPEN);

        std::thread::sleep(Duration::from_millis(60));
        let _ = cb.embed("test");
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn half_open_allows_exactly_one_concurrent_probe() {
        struct GatedProvider {
            calls: AtomicU32,
            entered: Arc<Barrier>,
            release: Arc<Barrier>,
        }

        impl EmbeddingProvider for GatedProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    return Err(EmbedError::ServerUnavailable("initial failure".into()));
                }
                self.entered.wait();
                self.release.wait();
                Ok(EmbedResult::new(vec![1.0, 1.0], 1))
            }

            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                texts.iter().map(|text| self.embed(text)).collect()
            }

            fn dimension(&self) -> usize {
                2
            }

            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                Ok(EmbeddingBuilderIdentity::test_deterministic("gated", 2, "gated-v1"))
            }

            fn model_name(&self) -> String {
                "gated".into()
            }
        }

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let inner = Arc::new(GatedProvider {
            calls: AtomicU32::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let breaker = Arc::new(EmbeddingCircuitBreaker::new(inner.clone(), 1, 0));
        assert!(breaker.embed_document("initial").is_err());

        let owner = Arc::clone(&breaker);
        let owner_thread = std::thread::spawn(move || owner.embed_document("probe"));
        entered.wait();
        let contender = Arc::clone(&breaker);
        let contender_thread = std::thread::spawn(move || contender.embed_document("contender"));
        let contender_result = contender_thread.join().unwrap();
        assert!(contender_result.is_err());
        release.wait();
        assert!(owner_thread.join().unwrap().is_ok());
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2);
        assert_eq!(breaker.state(), STATE_CLOSED);
    }

    #[test]
    fn wrapper_preserves_each_input_operation() {
        struct OperationProvider(Mutex<Vec<EmbeddingInputOperation>>);

        impl OperationProvider {
            fn record(&self, operation: EmbeddingInputOperation) -> EmbedResult {
                self.0.lock().unwrap().push(operation);
                EmbedResult::new(vec![1.0, 1.0], 1)
            }
        }

        impl EmbeddingProvider for OperationProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                Ok(self.record(EmbeddingInputOperation::Generic))
            }

            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                texts.iter().map(|text| self.embed(text)).collect()
            }

            fn embed_query(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                Ok(self.record(EmbeddingInputOperation::Query))
            }

            fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                Ok(self.record(EmbeddingInputOperation::Document))
            }

            fn dimension(&self) -> usize {
                2
            }

            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                Ok(EmbeddingBuilderIdentity::test_deterministic(
                    "operation",
                    2,
                    "operation-v1",
                ))
            }

            fn model_name(&self) -> String {
                "operation".into()
            }
        }

        let inner = Arc::new(OperationProvider(Mutex::new(Vec::new())));
        let breaker = EmbeddingCircuitBreaker::new(inner.clone(), 2, 1);
        breaker.embed("generic").unwrap();
        breaker.embed_batch(&["generic-batch"]).unwrap();
        breaker.embed_query("query").unwrap();
        breaker.embed_document("document").unwrap();
        breaker.embed_document_batch(&["document-batch"]).unwrap();
        assert_eq!(
            *inner.0.lock().unwrap(),
            vec![
                EmbeddingInputOperation::Generic,
                EmbeddingInputOperation::Generic,
                EmbeddingInputOperation::Query,
                EmbeddingInputOperation::Document,
                EmbeddingInputOperation::Document,
            ]
        );
    }

    #[test]
    fn failure_trace_contract_child() {
        if std::env::var_os("PLICO_EMBEDDING_FAILURE_TRACE_CHILD").is_none() {
            return;
        }

        struct PrivateFailureProvider;

        impl EmbeddingProvider for PrivateFailureProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                Err(EmbedError::ServerUnavailable(
                    "PRIVATE_ENDPOINT_CANARY PRIVATE_BODY_CANARY PRIVATE_INPUT_CANARY".into(),
                ))
            }

            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                texts.iter().map(|text| self.embed(text)).collect()
            }

            fn dimension(&self) -> usize {
                2
            }

            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                Ok(EmbeddingBuilderIdentity::test_deterministic(
                    "private-failure",
                    2,
                    "private-failure-v1",
                ))
            }

            fn model_name(&self) -> String {
                "private-failure".into()
            }
        }

        let _trace_guard = crate::TRACE_CAPTURE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let captured = CapturedFields::default();
        tracing::subscriber::with_default(captured.clone(), || {
            tracing::callsite::rebuild_interest_cache();
            let breaker = EmbeddingCircuitBreaker::new(Arc::new(PrivateFailureProvider), 1, 1);
            assert!(breaker.embed_document("PRIVATE_INPUT_CANARY").is_err());
        });
        let fields = captured.fields.lock().unwrap().clone();
        assert!(fields
            .iter()
            .any(|(name, value)| { name == "error_category" && value.contains("server_unavailable") }));
        let values = fields.iter().map(|(_, value)| value.as_str()).collect::<String>();
        for sentinel in ["PRIVATE_ENDPOINT_CANARY", "PRIVATE_BODY_CANARY", "PRIVATE_INPUT_CANARY"] {
            assert!(!values.contains(sentinel));
        }
    }

    #[test]
    fn failure_trace_contains_only_stable_category() {
        let executable = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(executable)
            .arg("--exact")
            .arg("fs::embedding::circuit_breaker::tests::failure_trace_contract_child")
            .arg("--nocapture")
            .env("PLICO_EMBEDDING_FAILURE_TRACE_CHILD", "1")
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "embedding failure trace child failed");
                return;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("embedding failure trace child exceeded deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
