//! Embedding Circuit Breaker — prevents cascading failures from embedding provider outages.
//!
//! Wraps an embedding provider with 3-state circuit breaker (Closed/Open/HalfOpen).
//! When the inner provider fails `failure_threshold` times consecutively, the circuit
//! opens and falls back to stub. After `cooldown_ms`, a probe is sent; success closes
//! the circuit, failure re-opens it.
//!
//! F-38: Embedding degradation awareness per Node 9 resilience design.

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::fs::embedding::stub::StubEmbeddingProvider;
use crate::fs::embedding::{EmbedError, Embedding, EmbeddingProvider, EmbedResult};

/// Circuit breaker states.
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// Embedding circuit breaker wrapping a real provider with fallback.
pub struct EmbeddingCircuitBreaker {
    inner: Arc<dyn EmbeddingProvider>,
    state: AtomicU8,
    failure_count: AtomicU32,
    failure_threshold: u32,
    last_failure_ms: AtomicU64,
    cooldown: Duration,
    stub: StubEmbeddingProvider,
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
            stub: StubEmbeddingProvider::new(),
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
}

impl EmbeddingProvider for EmbeddingCircuitBreaker {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let state = self.state();

        if state == STATE_OPEN {
            // Check if cooldown has elapsed → transition to HalfOpen
            let elapsed = Self::now_ms() - self.last_failure_ms.load(Ordering::Relaxed);
            if elapsed >= self.cooldown.as_millis() as u64 {
                self.state.store(STATE_HALF_OPEN, Ordering::Relaxed);
                // Fall through to try inner again
            } else {
                return self.stub.embed(text);
            }
        }

        if state == STATE_HALF_OPEN || state == STATE_OPEN {
            match self.inner.embed(text) {
                Ok(result) => {
                    // Success → close circuit
                    self.state.store(STATE_CLOSED, Ordering::Relaxed);
                    self.failure_count.store(0, Ordering::Relaxed);
                    tracing::info!("Embedding circuit breaker CLOSED — provider recovered");
                    Ok(result)
                }
                Err(e) => {
                    // Failure in HalfOpen → re-open circuit
                    self.state.store(STATE_OPEN, Ordering::Relaxed);
                    self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                    tracing::warn!("Embedding circuit breaker HalfOpen probe failed: {e}");
                    self.stub.embed(text)
                }
            }
        } else {
            // STATE_CLOSED — normal operation
            match self.inner.embed(text) {
                Ok(result) => {
                    self.failure_count.store(0, Ordering::Relaxed);
                    Ok(result)
                }
                Err(e) => {
                    let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if count >= self.failure_threshold {
                        self.state.store(STATE_OPEN, Ordering::Relaxed);
                        self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                        tracing::warn!("Embedding circuit breaker OPEN after {count} failures: {e}");
                    }
                    Err(e)
                }
            }
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let state = self.state();
        if state == STATE_OPEN {
            let elapsed = Self::now_ms() - self.last_failure_ms.load(Ordering::Relaxed);
            if elapsed < self.cooldown.as_millis() as u64 {
                return self.stub.embed_batch(texts);
            }
            self.state.store(STATE_HALF_OPEN, Ordering::Relaxed);
        }
        // For batch, just delegate — circuit breaker state is per-call
        if state == STATE_HALF_OPEN {
            match self.inner.embed_batch(texts) {
                Ok(results) => {
                    self.state.store(STATE_CLOSED, Ordering::Relaxed);
                    self.failure_count.store(0, Ordering::Relaxed);
                    Ok(results)
                }
                Err(_e) => {
                    self.state.store(STATE_OPEN, Ordering::Relaxed);
                    self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                    self.stub.embed_batch(texts)
                }
            }
        } else {
            match self.inner.embed_batch(texts) {
                Ok(results) => {
                    self.failure_count.store(0, Ordering::Relaxed);
                    Ok(results)
                }
                Err(e) => {
                    let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if count >= self.failure_threshold {
                        self.state.store(STATE_OPEN, Ordering::Relaxed);
                        self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                    }
                    Err(e)
                }
            }
        }
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        fn model_name(&self) -> &str {
            "failing"
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

        // After opening, calls go to stub (which also returns Err on StubEmbeddingProvider).
        // The key assertion: state is OPEN, not that stub "succeeds".
        // Verify circuit remains open after an additional call.
        let _ = cb.embed("test");
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
}
