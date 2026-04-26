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
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Mutex as TokioMutex, RwLock};
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
type RequestHandler = Box<dyn Fn(&str, Option<&str>) -> String + Send + Sync>;

pub struct KernelExecutor {
    /// Callback that executes an ApiRequest JSON and returns an ApiResponse JSON.
    /// Uses a boxed closure so the executor doesn't depend on kernel types directly,
    /// preserving the dependency direction (scheduler never imports kernel).
    handler: RequestHandler,
}

impl KernelExecutor {
    /// Create a KernelExecutor with a request handler closure.
    ///
    /// The closure receives a JSON-serialized `ApiRequest` and an optional
    /// agent ID, and must return a JSON-serialized `ApiResponse`.
    pub fn new(handler: impl Fn(&str, Option<&str>) -> String + Send + Sync + 'static) -> Self {
        Self { handler: Box::new(handler) }
    }
}

impl AgentExecutor for KernelExecutor {
    fn execute(
        &self,
        intent: &Intent,
        agent_id: Option<&AgentId>,
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

        let aid_str = agent_id.map(|a| a.0.as_str());
        let response_json = (self.handler)(action_json, aid_str);
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

        
        DispatchHandle {
            shutdown_tx,
            result_rx: Arc::new(RwLock::new(result_rx)),
            queue,
            task_handle: Arc::new(RwLock::new(Some(handle))),
        }
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

    let agent_locks: Arc<StdMutex<HashMap<String, Arc<TokioMutex<()>>>>> =
        Arc::new(StdMutex::new(HashMap::new()));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                tracing::info!("Dispatch loop shutting down");
                break;
            }

            _ = poll_timer.tick() => {
                while let Some(intent) = scheduler.dequeue() {
                    tracing::debug!("Draining intent {} from scheduler", intent.id);
                    queue.write().await.push(intent);
                }

                let mut intents = Vec::new();
                {
                    let mut q = queue.write().await;
                    while let Some(intent) = q.pop() {
                        intents.push(intent);
                    }
                }

                for intent in intents {
                    let agent_key = intent.agent_id.as_ref()
                        .map(|a| a.0.clone())
                        .unwrap_or_else(|| format!("_anon_{}", intent.id.0));

                    let lock = {
                        let mut locks = agent_locks.lock().unwrap();
                        locks.entry(agent_key)
                            .or_insert_with(|| Arc::new(TokioMutex::new(())))
                            .clone()
                    };

                    let scheduler = Arc::clone(&scheduler);
                    let executor = Arc::clone(&executor);
                    let result_tx = result_tx.clone();

                    tokio::spawn(async move {
                        let _guard = lock.lock().await;

                        let start = Instant::now();
                        let intent_id = intent.id.clone();
                        let agent_id = intent.agent_id.clone();

                        if let Some(ref aid) = intent.agent_id {
                            if let Err(e) = scheduler.update_state(aid, AgentState::Running) {
                                let elapsed_ms = start.elapsed().as_millis() as u64;
                                let result = ExecutionResult::failure(
                                    intent_id,
                                    agent_id,
                                    format!("Cannot execute: {}", e),
                                    elapsed_ms,
                                );
                                let _ = result_tx.send(result).await;
                                return;
                            }
                        }

                        let limit_ms = if let Some(ref aid) = intent.agent_id {
                            scheduler.get(aid)
                                .map(|a| a.resources().cpu_time_quota)
                                .unwrap_or(cpu_time_limit_ms)
                        } else {
                            cpu_time_limit_ms
                        };

                        let agent_id_exec = intent.agent_id.clone();
                        let executor_ref = Arc::clone(&executor);
                        let intent_ref = intent.clone();

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
                                ExecutionResult::success(intent_id, agent_id.clone(), out, elapsed_ms)
                            }
                            Err(msg) => {
                                ExecutionResult::failure(intent_id, agent_id.clone(), msg, elapsed_ms)
                            }
                        };

                        if let Some(ref aid) = agent_id {
                            let next_state = if result.success {
                                AgentState::Waiting
                            } else {
                                AgentState::Failed
                            };
                            let _ = scheduler.update_state(aid, next_state);
                        }

                        let _ = result_tx.send(result).await;
                    });
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

    #[tokio::test]
    async fn test_kernel_executor_falls_back_without_action() {
        // M1: When intent.action is None, KernelExecutor should acknowledge without error
        let executor = KernelExecutor::new(|_, _| {
            r#"{"ok":true}"#.to_string()
        });

        let intent = Intent::new(crate::scheduler::agent::IntentPriority::High, "descriptive intent".into());
        let result = executor.execute(&intent, None, 5000);

        assert!(result.is_ok());
        assert!(result.unwrap().contains("acknowledged"));
    }

    #[test]
    fn test_kernel_executor_valid_json_returns_response() {
        // M1: Verify KernelExecutor correctly processes valid ApiRequest JSON
        use crate::api::semantic::{ApiRequest, ApiResponse};

        // Use a mock handler that echoes the request back as JSON
        let executor = KernelExecutor::new(|action_json: &str, _agent_id: Option<&str>| {
            let req: Result<ApiRequest, _> = serde_json::from_str(action_json);
            match req {
                Ok(_r) => {
                    // Return a valid success response
                    serde_json::to_string(&ApiResponse::ok()).unwrap_or_default()
                }
                Err(e) => {
                    serde_json::to_string(&ApiResponse::error(format!("Invalid action JSON: {e}")))
                        .unwrap_or_default()
                }
            }
        });

        // Valid ApiRequest JSON: create object (requires agent_id)
        let action_json = r#"{"method":"create","content":"hello world","tags":["test"],"agent_id":"test-agent"}"#;
        let intent = Intent::new(crate::scheduler::agent::IntentPriority::High, "test".into())
            .with_action(action_json.to_string());

        let result = executor.execute(&intent, None, 5000);
        assert!(result.is_ok(), "execute should succeed");
        let resp_text = result.unwrap();

        // Response should be valid JSON containing ok=true
        let resp: ApiResponse = serde_json::from_str(&resp_text)
            .expect("response should be valid ApiResponse JSON");
        assert!(resp.ok, "expected ok=true, got ok=false with error: {:?}", resp.error);
    }

    #[test]
    fn test_kernel_executor_invalid_json_returns_error() {
        // M1: Invalid JSON in intent.action returns error response
        use crate::api::semantic::{ApiRequest, ApiResponse};

        let executor = KernelExecutor::new(|action_json: &str, _agent_id: Option<&str>| {
            let req: Result<ApiRequest, _> = serde_json::from_str(action_json);
            match req {
                Ok(_r) => serde_json::to_string(&ApiResponse::ok()).unwrap_or_default(),
                Err(e) => serde_json::to_string(&ApiResponse::error(format!("Invalid action JSON: {e}")))
                    .unwrap_or_default(),
            }
        });

        let intent = Intent::new(crate::scheduler::agent::IntentPriority::High, "test".into())
            .with_action(r#"not valid json"#.to_string());

        let result = executor.execute(&intent, None, 5000);
        assert!(result.is_ok());
        let resp_text = result.unwrap();

        // Should contain the error message from our handler
        assert!(resp_text.contains("Invalid action JSON"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cpu_time_quota_enforced_on_dispatch() {
        // M2: Agent with cpu_time_quota=50ms is killed after 50ms
        use crate::scheduler::agent::{Agent, AgentResources, IntentPriority};

        let scheduler = Arc::new(AgentScheduler::new());

        // Create agent with 50ms CPU quota
        let mut agent = Agent::new("quota-test-agent".to_string());
        let agent_id = agent.id().clone();
        {
            let mut resources = AgentResources::default();
            resources.cpu_time_quota = 50; // 50ms limit
            agent.set_resources(resources);
        }
        scheduler.register(agent);

        // Create executor that sleeps longer than quota
        let executor: Arc<dyn AgentExecutor> = Arc::new(SlowExecutor(200)); // 200ms > 50ms quota
        let loop_ = TokioDispatchLoop::new(Arc::clone(&scheduler), executor, 60_000); // hardcoded 60s
        let dispatch = loop_.spawn();

        // Submit intent from the quota-limited agent
        let intent = Intent::new(IntentPriority::High, "slow task".to_string())
            .with_agent(agent_id.clone());
        scheduler.submit(intent);

        // Wait for execution
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let results = dispatch.drain_results().await;
        dispatch.shutdown();

        // Result should show timeout/failure
        assert!(!results.is_empty(), "should have a result");
        let result = &results[0];
        assert!(!result.success, "execution should have failed due to timeout");
        assert!(result.output.contains("Timeout") || result.output.contains("timeout"),
            "should mention timeout, got: {}", result.output);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_cpu_time_quota_zero_means_unlimited() {
        // M2: Agent with cpu_time_quota=0 runs without timeout (backward compatible)
        use crate::scheduler::agent::{Agent, IntentPriority};

        let scheduler = Arc::new(AgentScheduler::new());

        // Create agent with unlimited quota (0 = default)
        let agent = Agent::new("unlimited-agent".to_string());
        let agent_id = agent.id().clone();
        // cpu_time_quota is 0 by default (unlimited)
        scheduler.register(agent);

        // Verify default is unlimited
        let resources = scheduler.get(&agent_id).map(|a| a.resources().clone()).unwrap();
        assert_eq!(resources.cpu_time_quota, 0, "default should be unlimited");

        // Executor that would timeout if quota was enforced
        let executor: Arc<dyn AgentExecutor> = Arc::new(SlowExecutor(100));
        let loop_ = TokioDispatchLoop::new(Arc::clone(&scheduler), executor, 60_000);
        let dispatch = loop_.spawn();

        let intent = Intent::new(IntentPriority::High, "slow but allowed".to_string())
            .with_agent(agent_id.clone());
        scheduler.submit(intent);

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        let results = dispatch.drain_results().await;
        dispatch.shutdown();

        // With unlimited quota (0), slow executor should succeed
        assert!(!results.is_empty(), "should have a result");
        let result = &results[0];
        assert!(result.success, "unlimited agent should succeed, got: {}", result.output);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_concurrent_different_agents_run_in_parallel() {
        use crate::scheduler::agent::{Agent, IntentPriority};

        let scheduler = Arc::new(AgentScheduler::new());
        let agent_a = Agent::new("agent-a".into());
        let aid_a = agent_a.id().clone();
        let agent_b = Agent::new("agent-b".into());
        let aid_b = agent_b.id().clone();
        scheduler.register(agent_a);
        scheduler.register(agent_b);

        let executor: Arc<dyn AgentExecutor> = Arc::new(SlowExecutor(200));
        let loop_ = TokioDispatchLoop::new(Arc::clone(&scheduler), executor, 60_000);
        let dispatch = loop_.spawn();

        let intent_a = Intent::new(IntentPriority::High, "task-a".into())
            .with_agent(aid_a.clone());
        let intent_b = Intent::new(IntentPriority::High, "task-b".into())
            .with_agent(aid_b.clone());
        scheduler.submit(intent_a);
        scheduler.submit(intent_b);

        let start = Instant::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(800)).await;
        let elapsed = start.elapsed().as_millis() as u64;

        let results = dispatch.drain_results().await;
        dispatch.shutdown();

        assert_eq!(results.len(), 2, "both agents should complete");
        assert!(results.iter().all(|r| r.success), "both should succeed");
        let max_elapsed = results.iter().map(|r| r.elapsed_ms).max().unwrap_or(0);
        assert!(max_elapsed < 400, "parallel execution should complete in ~200ms, not {}ms", max_elapsed);
        let _ = elapsed;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_same_agent_intents_serialize() {
        use crate::scheduler::agent::{Agent, IntentPriority};
        use std::sync::atomic::{AtomicU32, Ordering};

        let scheduler = Arc::new(AgentScheduler::new());
        let agent = Agent::new("serial-agent".into());
        let aid = agent.id().clone();
        scheduler.register(agent);

        let call_count = Arc::new(AtomicU32::new(0));
        let cc = Arc::clone(&call_count);

        struct CountingExecutor {
            count: Arc<AtomicU32>,
            delay_ms: u64,
        }
        impl AgentExecutor for CountingExecutor {
            fn execute(&self, intent: &Intent, _agent_id: Option<&AgentId>, _cpu_time_limit_ms: u64) -> Result<String, String> {
                let n = self.count.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(self.delay_ms));
                Ok(format!("exec-{}-{}", n, intent.description))
            }
        }

        let executor: Arc<dyn AgentExecutor> = Arc::new(CountingExecutor { count: cc, delay_ms: 100 });
        let loop_ = TokioDispatchLoop::new(Arc::clone(&scheduler), executor, 60_000);
        let dispatch = loop_.spawn();

        let intent1 = Intent::new(IntentPriority::High, "first".into())
            .with_agent(aid.clone());
        let intent2 = Intent::new(IntentPriority::High, "second".into())
            .with_agent(aid.clone());
        scheduler.submit(intent1);
        scheduler.submit(intent2);

        tokio::time::sleep(tokio::time::Duration::from_millis(900)).await;

        let results = dispatch.drain_results().await;
        dispatch.shutdown();

        assert_eq!(results.len(), 2, "both intents should complete");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        let total_elapsed: u64 = results.iter().map(|r| r.elapsed_ms).sum();
        assert!(total_elapsed >= 180, "serialized execution should take ~200ms total, got {}ms", total_elapsed);
    }
}
