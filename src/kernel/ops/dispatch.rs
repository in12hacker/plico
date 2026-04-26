//! Dispatch loop operations — start Tokio or local executor.

use std::sync::Arc;
use crate::scheduler::dispatch::{TokioDispatchLoop, LocalExecutor, KernelExecutor, AgentExecutor, DispatchHandle};

impl crate::kernel::AIKernel {
    /// Start the dispatch loop with KernelExecutor.
    ///
    /// `KernelExecutor` deserializes JSON action payloads and dispatches them to
    /// the kernel's API request handler. Falls back to `LocalExecutor` for
    /// intents without an action payload.
    pub fn start_dispatch_loop(self: &Arc<Self>) -> DispatchHandle {
        let kernel = Arc::clone(self);
        let executor: Arc<dyn AgentExecutor> = Arc::new(KernelExecutor::new(
            move |action_json: &str, _agent_id: Option<&str>| {
                use crate::api::semantic::{ApiRequest, ApiResponse};
                let req: ApiRequest = match serde_json::from_str(action_json) {
                    Ok(r) => r,
                    Err(e) => return serde_json::to_string(
                        &ApiResponse::error(format!("Invalid action JSON: {e}"))
                    ).unwrap_or_default(),
                };
                let resp = kernel.handle_api_request(req);
                serde_json::to_string(&resp).unwrap_or_default()
            }
        ));
        let loop_ = TokioDispatchLoop::new(Arc::clone(&self.scheduler), executor, 60_000);
        loop_.spawn()
    }

    /// Start the dispatch loop with LocalExecutor (test-only, no kernel dispatch).
    pub fn start_dispatch_loop_local(&self) -> DispatchHandle {
        let executor: Arc<dyn AgentExecutor> = Arc::new(LocalExecutor);
        let loop_ = TokioDispatchLoop::new(Arc::clone(&self.scheduler), executor, 60_000);
        loop_.spawn()
    }

    /// Start a result consumer that drains execution results and emits events.
    ///
    /// Per soul alignment: the kernel records execution outcomes as events
    /// (mechanism), but does NOT auto-store working memory (policy). Application
    /// layers decide what to learn from execution results.
    pub fn start_result_consumer(self: &Arc<Self>, handle: &DispatchHandle) -> tokio::task::JoinHandle<()> {
        let kernel = Arc::clone(self);
        let handle = handle.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let results = handle.drain_results().await;
                for result in results {
                    let agent_id = result.agent_id.as_ref()
                        .map(|a| a.0.as_str())
                        .unwrap_or("system");

                    kernel.event_bus.emit(crate::kernel::event_bus::KernelEvent::IntentCompleted {
                        intent_id: result.intent_id.0.clone(),
                        success: result.success,
                    });

                    let output_preview: String = result.output.chars().take(120).collect();
                    let label = format!(
                        "dispatch:{}:{}",
                        if result.success { "ok" } else { "fail" },
                        result.intent_id.0,
                    );
                    let tags = if result.success {
                        vec!["execution-success".to_string(), "dispatch".to_string()]
                    } else {
                        vec!["execution-failure".to_string(), "dispatch".to_string()]
                    };

                    let _ = kernel.create_event(crate::fs::semantic_fs::events::CreateEventParams {
                        label: &label,
                        event_type: crate::fs::EventType::Work,
                        start_time: Some(crate::memory::layered::now_ms().saturating_sub(result.elapsed_ms)),
                        end_time: Some(crate::memory::layered::now_ms()),
                        location: None,
                        tags,
                        agent_id,
                    });

                    tracing::debug!(
                        intent_id = %result.intent_id.0,
                        success = result.success,
                        elapsed_ms = result.elapsed_ms,
                        output = %output_preview,
                        "Dispatch result recorded as event",
                    );
                }
            }
        })
    }
}
