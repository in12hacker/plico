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
            move |action_json: &str| {
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
}
