//! LLM Circuit Breaker — prevents cascading failures from LLM provider outages.
//!
//! Wraps an LLM provider with 3-state circuit breaker (Closed/Open/HalfOpen).
//! When the inner provider fails `failure_threshold` times consecutively, the circuit
//! opens and returns LlmError::Unavailable immediately. After `cooldown_ms`, a probe
//! is sent; success closes the circuit, failure re-opens it.
//!
//! F-2: LLM断路哨兵 — Node 19哨兵层设计。

use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::{LlmError, LlmProvider, ChatMessage, ChatOptions};

/// Circuit breaker states.
const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

/// LLM circuit breaker wrapping a real provider.
/// When the circuit is open, returns LlmError::Unavailable immediately (fail-fast).
pub struct CircuitBreakerLlmProvider {
    inner: Arc<dyn LlmProvider>,
    state: AtomicU8,
    failure_count: AtomicU32,
    failure_threshold: u32,
    last_failure_ms: AtomicU64,
    cooldown: Duration,
}

impl CircuitBreakerLlmProvider {
    pub fn new(inner: Arc<dyn LlmProvider>, failure_threshold: u32, cooldown_ms: u64) -> Self {
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

    /// Returns a human-readable status string.
    pub fn status(&self) -> &'static str {
        match self.state.load(Ordering::Relaxed) {
            STATE_CLOSED => "active",
            STATE_OPEN => "degraded",
            STATE_HALF_OPEN => "probing",
            _ => "unknown",
        }
    }
}

impl LlmProvider for CircuitBreakerLlmProvider {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
        let state = self.state();

        if state == STATE_OPEN {
            // Check if cooldown has elapsed → transition to HalfOpen
            let elapsed = Self::now_ms() - self.last_failure_ms.load(Ordering::Relaxed);
            if elapsed >= self.cooldown.as_millis() as u64 {
                self.state.store(STATE_HALF_OPEN, Ordering::Relaxed);
            } else {
                return Err(LlmError::Unavailable("LLM circuit open".into()));
            }
        }

        if state == STATE_HALF_OPEN || state == STATE_OPEN {
            // Already checked OPEN case above; here we handle HALF_OPEN transitioning from OPEN
            match self.inner.chat(messages, options) {
                Ok((r, in_tok, out_tok)) => {
                    self.state.store(STATE_CLOSED, Ordering::Relaxed);
                    self.failure_count.store(0, Ordering::Relaxed);
                    tracing::info!("LLM circuit breaker CLOSED — provider recovered");
                    Ok((r, in_tok, out_tok))
                }
                Err(e) => {
                    self.state.store(STATE_OPEN, Ordering::Relaxed);
                    self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                    tracing::warn!("LLM circuit breaker HalfOpen probe failed: {e}");
                    Err(LlmError::Unavailable(format!("LLM circuit open (probe failed): {e}")))
                }
            }
        } else {
            // STATE_CLOSED — normal operation
            match self.inner.chat(messages, options) {
                Ok((r, in_tok, out_tok)) => {
                    self.failure_count.store(0, Ordering::Relaxed);
                    Ok((r, in_tok, out_tok))
                }
                Err(e) => {
                    let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
                    if count >= self.failure_threshold {
                        self.state.store(STATE_OPEN, Ordering::Relaxed);
                        self.last_failure_ms.store(Self::now_ms(), Ordering::Relaxed);
                        tracing::warn!("LLM circuit breaker OPEN after {count} failures: {e}");
                    }
                    Err(e)
                }
            }
        }
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    struct FailingLlm {
        fail_count: AtomicU32,
        fail_for: AtomicU32,
    }

    impl FailingLlm {
        fn new(fail_count: u32) -> Self {
            Self {
                fail_count: AtomicU32::new(0),
                fail_for: AtomicU32::new(fail_count),
            }
        }
    }

    impl LlmProvider for FailingLlm {
        fn chat(&self, _messages: &[ChatMessage], _options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
            let n = self.fail_count.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= self.fail_for.load(Ordering::Relaxed) {
                Err(LlmError::Api("test failure".into()))
            } else {
                Ok(("success response".into(), 0, 0))
            }
        }
        fn model_name(&self) -> &str { "failing-llm" }
    }

    #[test]
    fn test_llm_circuit_breaker_opens_after_threshold() {
        let inner = Arc::new(FailingLlm::new(5));
        let cb = CircuitBreakerLlmProvider::new(Arc::clone(&inner) as Arc<dyn LlmProvider>, 3, 100);

        // First 3 calls fail → circuit opens
        for _ in 0..3 {
            cb.chat(&[], &ChatOptions::default()).unwrap_err();
        }
        assert_eq!(cb.state(), STATE_OPEN);

        // Additional calls fail fast (circuit open)
        let result = cb.chat(&[], &ChatOptions::default());
        assert!(matches!(result, Err(LlmError::Unavailable(_))));
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn test_llm_circuit_breaker_recovery() {
        let inner = Arc::new(FailingLlm::new(1));
        let cb = CircuitBreakerLlmProvider::new(Arc::clone(&inner) as Arc<dyn LlmProvider>, 1, 50);

        // First call fails → circuit opens
        cb.chat(&[], &ChatOptions::default()).unwrap_err();
        assert_eq!(cb.state(), STATE_OPEN);

        // Wait for cooldown
        std::thread::sleep(Duration::from_millis(60));

        // Next call goes to HalfOpen → probe succeeds → closes
        let result = cb.chat(&[], &ChatOptions::default());
        assert!(result.is_ok(), "recovery probe should succeed");
        assert_eq!(cb.state(), STATE_CLOSED);
    }

    #[test]
    fn test_llm_circuit_breaker_recovery_failure() {
        // FailingLlm fails for first 2 calls then succeeds
        let inner = Arc::new(FailingLlm::new(2));
        let cb = CircuitBreakerLlmProvider::new(Arc::clone(&inner) as Arc<dyn LlmProvider>, 1, 50);

        // First call fails → circuit opens
        cb.chat(&[], &ChatOptions::default()).unwrap_err();

        std::thread::sleep(Duration::from_millis(60));

        // Probe call fails again → stays open
        cb.chat(&[], &ChatOptions::default()).unwrap_err();
        assert_eq!(cb.state(), STATE_OPEN);
    }

    #[test]
    fn test_llm_circuit_breaker_status_reports_correctly() {
        let inner = Arc::new(FailingLlm::new(0)); // never fails
        let cb = CircuitBreakerLlmProvider::new(inner, 3, 100);

        assert_eq!(cb.status(), "active");

        // Force open state
        cb.state.store(STATE_OPEN, Ordering::Relaxed);
        assert_eq!(cb.status(), "degraded");

        cb.state.store(STATE_HALF_OPEN, Ordering::Relaxed);
        assert_eq!(cb.status(), "probing");
    }
}
