//! Agent Execution Runtime — Intent → Running Agent
//!
//! Provides the dispatch loop that runs agents as async tasks.
//!
//! # Architecture
//!
//! ```text
//! AgentExecutor (trait)
//! └── TokioDispatchLoop  — tokio async loop over the scheduler queue
//! ```
//!
//! # Dispatch Flow
//!
//! ```text
//! Intent submitted
//!   → SchedulerQueue (priority heap)
//!   → TokioDispatchLoop polls queue every POLL_INTERVAL
//!   → AgentExecutor::execute() runs the agent
//!   → Agent state updated (Waiting → Running → Completed/Failed)
//!   → Result returned to caller
//! ```

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time::{interval, Instant};

use super::agent::{AgentId, AgentState, Intent, IntentId};
use super::queue::SchedulerQueue;
use super::AgentScheduler;

/// Interval between queue polls (500ms).
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Errors from agent execution.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("Intent not found: {0}")]
    IntentNotFound(IntentId),

    #[error("Agent not found: {0}")]
    AgentNotFound(AgentId),

    #[error("Execution timeout after {0}ms")]
    Timeout(u64),

    #[error("Agent {0} is not in a runnable state: {1:?}")]
    NotRunnable(AgentId, AgentState),

    #[error("Tokio runtime error: {0}")]
    Tokio(#[from] tokio::task::JoinError),

    #[error("Channel closed: {0}")]
    ChannelClosed(String),
}

/// Result of an agent execution attempt.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub intent_id: IntentId,
    pub agent_id: Option<AgentId>,
    pub success: bool,
    pub output: String,
    pub elapsed_ms: u64,
}

impl ExecutionResult {
    pub fn success(intent_id: IntentId, agent_id: Option<AgentId>, output: String, elapsed_ms: u64) -> Self {
        Self { intent_id, agent_id, success: true, output, elapsed_ms }
    }

    pub fn failure(intent_id: IntentId, agent_id: Option<AgentId>, output: String, elapsed_ms: u64) -> Self {
        Self { intent_id, agent_id, success: false, output, elapsed_ms }
    }
}

/// Thread-safe handle for interacting with the dispatch loop.
#[derive(Clone)]
pub struct DispatchHandle {
    /// Send a shutdown signal to the dispatch loop.
    shutdown_tx: broadcast::Sender<()>,
    /// Receive execution results.
    result_rx: Arc<RwLock<mpsc::Receiver<ExecutionResult>>>,
    /// Reference to the scheduler queue (shared with dispatch loop).
    queue: Arc<RwLock<SchedulerQueue>>,
    /// Background task handle — abort on drop to prevent runtime leak.
    task_handle: Arc<RwLock<Option<tokio::task::JoinHandle<()>>>>,
}

impl DispatchHandle {
    /// Send a shutdown signal and abort the background task.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
        if let Ok(mut handle) = self.task_handle.try_write() {
            if let Some(h) = handle.take() {
                h.abort();
            }
        }
    }

    /// Take all pending execution results.
    pub async fn drain_results(&self) -> Vec<ExecutionResult> {
        let mut rx = self.result_rx.write().await;
        let mut results = Vec::new();
        while let Ok(result) = rx.try_recv() {
            results.push(result);
        }
        results
    }

    /// Number of intents currently queued.
    pub async fn queue_len(&self) -> usize {
        self.queue.read().await.len()
    }
}

/// Trait for agent execution backends.
///
/// Implement this trait to provide different execution strategies:
/// - Local tokio task execution (MVP)
/// - Remote process execution (future)
/// - Container/VM isolation (future)
pub trait AgentExecutor: Send + Sync {
    /// Execute an intent as the given agent.
    ///
    /// Returns `Ok(output)` on success, `Err(message)` on failure.
    ///
    /// `cpu_time_limit_ms` — hard limit on CPU time for this execution.
    /// Implementations should enforce this limit.
    fn execute(
        &self,
        intent: &Intent,
        agent_id: Option<&AgentId>,
        cpu_time_limit_ms: u64,
    ) -> Result<String, String>;
}

/// Simple in-process executor — logs the intent and returns a stub result.
/// Used for testing the dispatch loop without a real kernel.
pub struct LocalExecutor;

impl AgentExecutor for LocalExecutor {
    fn execute(
        &self,
        intent: &Intent,
        _agent_id: Option<&AgentId>,
        _cpu_time_limit_ms: u64,
    ) -> Result<String, String> {
        tracing::info!(
            "LocalExecutor: intent {}: \"{}\" (priority={:?})",
            intent.id,
            intent.description,
            intent.priority
        );
        Ok(format!(
            "[Executed] Intent '{}' completed successfully.",
            intent.description
        ))
    }
}

/// Kernel-backed executor — deserializes the intent's `action` as an
/// `ApiRequest`, dispatches it through the kernel, and returns the JSON
/// response. Falls back to LocalExecutor behavior if no action is present.
pub struct KernelExecutor {
    /// Callback that executes an ApiRequest JSON and returns an ApiResponse JSON.
    /// Uses a boxed closure so the executor doesn't depend on kernel types directly,
    /// preserving the dependency direction (scheduler never imports kernel).
    handler: Box<dyn Fn(&str) -> String + Send + Sync>,
}

impl KernelExecutor {
    /// Create a KernelExecutor with a request handler closure.
    ///
    /// The closure receives a JSON-serialized `ApiRequest` and must return
    /// a JSON-serialized `ApiResponse`.
    pub fn new(handler: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        Self { handler: Box::new(handler) }
    }
}

impl AgentExecutor for KernelExecutor {
    fn execute(
        &self,
        intent: &Intent,
        _agent_id: Option<&AgentId>,
        _cpu_time_limit_ms: u64,
    ) -> Result<String, String> {
        let Some(ref action_json) = intent.action else {
            tracing::info!(
                "KernelExecutor: no action for intent {}, treating as descriptive",
                intent.id
            );
            return Ok(format!("[No action] Intent '{}' acknowledged.", intent.description));
        };

        tracing::info!(
            "KernelExecutor: executing intent {} action",
            intent.id
        );

        let response_json = (self.handler)(action_json);
        Ok(response_json)
    }
}

/// The dispatch loop — polls the scheduler queue and executes intents.
///
/// Runs as a background tokio task. Multiple dispatch loops can run
/// concurrently for parallel execution.
pub struct TokioDispatchLoop {
    scheduler: Arc<AgentScheduler>,
    executor: Arc<dyn AgentExecutor>,
    cpu_time_limit_ms: u64,
    poll_interval: Duration,
}

impl TokioDispatchLoop {
    /// Create a new dispatch loop.
    ///
    /// `scheduler` — the agent scheduler (provides the queue and agent registry).
    /// `executor` — the execution backend (e.g. `LocalExecutor`).
    /// `cpu_time_limit_ms` — hard limit on CPU time per intent.
    pub fn new(
        scheduler: Arc<AgentScheduler>,
        executor: Arc<dyn AgentExecutor>,
        cpu_time_limit_ms: u64,
    ) -> Self {
        Self {
            scheduler,
            executor,
            cpu_time_limit_ms,
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Run the dispatch loop as a background tokio task.
    /// Returns a `DispatchHandle` for controlling the loop.
    pub fn spawn(self) -> DispatchHandle {
        let (shutdown_tx, _) = broadcast::channel(1);
        let (result_tx, result_rx) = mpsc::channel(100);
        let queue = Arc::new(RwLock::new(SchedulerQueue::new()));
        let queue_clone = Arc::clone(&queue);

        // Mirror scheduler queue into our local queue
        // (In a full implementation, this would use a shared channel)
        let scheduler_clone = Arc::clone(&self.scheduler);
        let executor_clone = Arc::clone(&self.executor);
        let cpu_limit = self.cpu_time_limit_ms;
        let poll_interval = self.poll_interval;
        let shutdown_rx = shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            dispatch_loop(
                scheduler_clone,
                executor_clone,
                queue_clone,
                result_tx,
                shutdown_rx,
                cpu_limit,
                poll_interval,
            )
            .await;
        });

        let dispatch_handle = DispatchHandle {
            shutdown_tx,
            result_rx: Arc::new(RwLock::new(result_rx)),
            queue,
            task_handle: Arc::new(RwLock::new(Some(handle))),
        };
        dispatch_handle
    }
}

async fn dispatch_loop(
    scheduler: Arc<AgentScheduler>,
    executor: Arc<dyn AgentExecutor>,
    queue: Arc<RwLock<SchedulerQueue>>,
    result_tx: mpsc::Sender<ExecutionResult>,
    mut shutdown_rx: broadcast::Receiver<()>,
    cpu_time_limit_ms: u64,
    poll_interval: Duration,
) {
    let mut poll_timer = interval(poll_interval);
    poll_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Shutdown signal received
            _ = shutdown_rx.recv() => {
                tracing::info!("Dispatch loop shutting down");
                break;
            }

            // Poll timer tick
            _ = poll_timer.tick() => {
                // Drain pending intents from the scheduler into our queue
                while let Some(intent) = scheduler.dequeue() {
                    tracing::debug!("Draining intent {} from scheduler", intent.id);
                    queue.write().await.push(intent);
                }

                // Execute next intent if any
                let intent = {
                    let mut q = queue.write().await;
                    q.pop()
                };

                if let Some(intent) = intent {
                    let start = Instant::now();
                    let intent_id = intent.id.clone();
                    let agent_id = intent.agent_id.clone();

                    // Update agent state to Running
                    if let Some(ref aid) = intent.agent_id {
                        scheduler.update_state(aid, AgentState::Running);
                    }

                    // Run executor in a blocking thread so it doesn't block the tokio runtime.
                    // Wrap with timeout so a slow/hung intent can't hold the dispatch loop
                    // indefinitely. cpu_time_limit_ms=0 means no limit.
                    let agent_id_exec = intent.agent_id.clone();
                    let executor_ref = Arc::clone(&executor);
                    let intent_ref = intent.clone();
                    let limit_ms = cpu_time_limit_ms;

                    let output = async {
                        tokio::task::spawn_blocking(move || {
                            executor_ref.execute(&intent_ref, agent_id_exec.as_ref(), limit_ms)
                        }).await
                    };

                    let output = if limit_ms > 0 {
                        match tokio::time::timeout(Duration::from_millis(limit_ms), output).await {
                            Ok(Ok(result)) => result,
                            Ok(Err(e)) => Err(format!("Task panicked: {e}")),
                            Err(_) => Err(format!("Timeout after {limit_ms}ms")),
                        }
                    } else {
                        match output.await {
                            Ok(result) => result,
                            Err(e) => Err(format!("Task panicked: {e}")),
                        }
                    };

                    let elapsed_ms = start.elapsed().as_millis() as u64;

                    let result = match output {
                        Ok(out) => {
                            ExecutionResult::success(intent_id.clone(), agent_id.clone(), out, elapsed_ms)
                        }
                        Err(msg) => {
                            ExecutionResult::failure(intent_id.clone(), agent_id.clone(), msg, elapsed_ms)
                        }
                    };

                    // After execution, set agent to Waiting (ready for more intents)
                    // unless it failed. Terminal states (Completed/Terminated) are
                    // only set explicitly by lifecycle API.
                    if let Some(ref aid) = agent_id {
                        let next_state = if result.success {
                            AgentState::Waiting
                        } else {
                            AgentState::Failed
                        };
                        scheduler.update_state(aid, next_state);
                    }

                    if result_tx.send(result).await.is_err() {
                        tracing::warn!("Dispatch loop: result receiver dropped");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::agent::IntentPriority;

    #[test]
    fn test_execution_result_success() {
        let r = ExecutionResult::success(
            IntentId::new(),
            None,
            "done".to_string(),
            100,
        );
        assert!(r.success);
        assert_eq!(r.elapsed_ms, 100);
    }

    #[test]
    fn test_execution_result_failure() {
        let r = ExecutionResult::failure(
            IntentId::new(),
            None,
            "crashed".to_string(),
            50,
        );
        assert!(!r.success);
        assert!(r.output.contains("crashed"));
    }

    #[test]
    fn test_local_executor_executes() {
        let executor = LocalExecutor;
        let intent = Intent::new(IntentPriority::High, "Analyze PR #42".into());
        let result = executor.execute(&intent, None, 5000);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Analyze PR #42"));
    }

    /// Executor that sleeps longer than the timeout.
    struct SlowExecutor(u64);
    impl AgentExecutor for SlowExecutor {
        fn execute(&self, _intent: &Intent, _agent_id: Option<&AgentId>, _cpu_time_limit_ms: u64) -> Result<String, String> {
            std::thread::sleep(std::time::Duration::from_millis(self.0));
            Ok("done".into())
        }
    }

    #[tokio::test]
    async fn test_executor_timeout_enforced() {
        use tokio::time::Duration;
        // Executor sleeps 500ms, but limit is 50ms — should timeout
        let executor: Arc<dyn AgentExecutor> = Arc::new(SlowExecutor(500));
        let intent = Intent::new(crate::scheduler::agent::IntentPriority::High, "slow task".into());
        let agent_id_exec = intent.agent_id.clone();
        let intent_ref = intent.clone();
        let executor_ref = Arc::clone(&executor);
        let limit_ms = 50u64;

        let output = async {
            tokio::task::spawn_blocking(move || {
                executor_ref.execute(&intent_ref, agent_id_exec.as_ref(), limit_ms)
            }).await
        };

        let result = tokio::time::timeout(Duration::from_millis(limit_ms), output).await;
        assert!(result.is_err(), "should have timed out but got: {:?}", result);
    }

    #[tokio::test]
    async fn test_dispatch_handle_shutdown() {
        let scheduler = Arc::new(AgentScheduler::new());
        let executor = Arc::new(LocalExecutor);
        let loop_ = TokioDispatchLoop::new(scheduler, executor, 5000);
        let dispatch = loop_.spawn();

        // Shutdown should not panic
        dispatch.shutdown();
        let len = dispatch.queue_len().await;
        assert_eq!(len, 0);
    }
}
