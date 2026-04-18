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
            move |action_json: &str, agent_id: Option<&str>| {
                use crate::api::semantic::{ApiRequest, ApiResponse};
                let req: ApiRequest = match serde_json::from_str(action_json) {
                    Ok(r) => r,
                    Err(e) => return serde_json::to_string(
                        &ApiResponse::error(format!("Invalid action JSON: {e}"))
                    ).unwrap_or_default(),
                };
                let resp = kernel.handle_api_request(req);
                if resp.ok {
                    if let Some(aid) = agent_id {
                        let preview: String = action_json.chars().take(80).collect();
                        let summary = format!(
                            "Executed: {} → {}",
                            preview,
                            resp.data.as_deref().unwrap_or("ok")
                        );
                        let _ = kernel.remember_working(
                            aid,
                            summary,
                            vec!["execution-result".to_string()],
                        );
                    }
                }
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

    /// Start a result consumer that drains execution results from the dispatch
    /// handle and feeds outcomes into the memory system for autonomous learning.
    ///
    /// On success: stores structured outcome in working memory with "execution-success" tag.
    /// On failure: stores failure in working memory with "execution-failure" tag.
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
                        .map(|a| a.0.as_str());
                    let Some(aid) = agent_id else { continue };

                    let tags = if result.success {
                        vec!["execution-success".to_string(), "dispatch".to_string()]
                    } else {
                        vec!["execution-failure".to_string(), "dispatch".to_string()]
                    };

                    let output_preview: String = result.output.chars().take(120).collect();
                    let summary = format!(
                        "Dispatch {}: {} ({}ms) → {}",
                        if result.success { "success" } else { "failure" },
                        result.intent_id.0,
                        result.elapsed_ms,
                        output_preview,
                    );

                    let _ = kernel.remember_working(aid, summary, tags);
                }
            }
        })
    }
}
