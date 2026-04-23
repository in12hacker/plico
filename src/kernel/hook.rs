//! Hook Registry — lifecycle interception for tool calls.
//!
//! Provides PreToolCall/PostToolCall interception points so agents can
//! block or audit tool execution. This is a mechanism (Soul 2.0 Axiom 5:
//! mechanism, not policy) — the interception strategy is defined by agents.

use serde_json::Value;
use std::sync::{Arc, RwLock};

/// Lifecycle hook points where interception can occur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookPoint {
    /// Before a tool is executed. Return Block to prevent execution.
    PreToolCall,
    /// After a tool completes (success or failure).
    PostToolCall,
    /// Before a session starts.
    PreSessionStart,
    /// Before a write operation (CAS create/update).
    PreWrite,
    /// Before a delete operation.
    PreDelete,
}

/// Result of running hooks at a given point.
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Continue with the operation.
    Continue,
    /// Block the operation with a reason.
    Block { reason: String },
}

/// Context passed to hook handlers at each hook point.
#[derive(Debug, Clone)]
pub struct HookContext {
    /// The agent requesting the operation.
    pub agent_id: String,
    /// The tool or operation name.
    pub tool_name: String,
    /// The parameters/input for the operation.
    pub params: Value,
    /// Wall-clock time in milliseconds since epoch.
    pub timestamp_ms: u64,
}

impl HookContext {
    pub fn new(agent_id: &str, tool_name: &str, params: Value) -> Self {
        Self {
            agent_id: agent_id.to_string(),
            tool_name: tool_name.to_string(),
            params,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }
}

/// Trait for hook handlers. Implement this to define interception logic.
pub trait HookHandler: Send + Sync {
    /// Handle a hook event. Return Continue to proceed, Block to reject.
    fn handle(&self, point: HookPoint, context: &HookContext) -> HookResult;
}

/// Hook registry — manages registration and execution of lifecycle hooks.
///
/// Hooks are stored as (point, priority, handler) tuples and executed in
/// priority order (lower priority numbers run first). First Block wins.
pub struct HookRegistry {
    hooks: RwLock<Vec<(HookPoint, i32, Arc<dyn HookHandler>)>>,
}

impl Default for HookRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: RwLock::new(Vec::new()),
        }
    }

    /// Register a hook handler at a specific point with a priority.
    /// Lower priority numbers execute first. Multiple handlers can be
    /// registered at the same point with different priorities.
    pub fn register(&self, point: HookPoint, priority: i32, handler: Arc<dyn HookHandler>) {
        let mut hooks = self.hooks.write().unwrap();
        hooks.push((point, priority, handler));
    }

    /// Run all hooks at a given point. Returns HookResult::Continue if
    /// no hooks blocked, or HookResult::Block from the first blocking hook.
    pub fn run_hooks(&self, point: HookPoint, context: &HookContext) -> HookResult {
        let hooks = self.hooks.read().unwrap();
        let mut relevant: Vec<_> = hooks
            .iter()
            .filter(|(p, _, _)| *p == point)
            .collect();
        relevant.sort_by_key(|(_, prio, _)| *prio);

        for (_, _, handler) in relevant {
            match handler.handle(point.clone(), context) {
                HookResult::Block { reason } => return HookResult::Block { reason },
                HookResult::Continue => {}
            }
        }
        HookResult::Continue
    }

    /// Clear all registered hooks. Useful for testing.
    #[cfg(test)]
    pub fn clear(&self) {
        let mut hooks = self.hooks.write().unwrap();
        hooks.clear();
    }

    /// Number of registered hooks. Useful for testing.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.hooks.read().unwrap().len()
    }
}

// ─── No-op Default Handler ───────────────────────────────────────────────────

/// A no-op hook handler that always returns Continue.
pub struct NoOpHookHandler;

impl HookHandler for NoOpHookHandler {
    fn handle(&self, _point: HookPoint, _context: &HookContext) -> HookResult {
        HookResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestHandler {
        result: HookResult,
    }
    impl HookHandler for TestHandler {
        fn handle(&self, _point: HookPoint, _context: &HookContext) -> HookResult {
            self.result.clone()
        }
    }

    #[test]
    fn test_hook_registry_empty_returns_continue() {
        let registry = HookRegistry::new();
        let ctx = HookContext::new("agent1", "cas.create", serde_json::json!({}));
        let result = registry.run_hooks(HookPoint::PreToolCall, &ctx);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_hook_registry_single_block() {
        let registry = HookRegistry::new();
        registry.register(
            HookPoint::PreToolCall,
            0,
            Arc::new(TestHandler { result: HookResult::Block { reason: "denied".into() } }),
        );
        let ctx = HookContext::new("agent1", "cas.delete", serde_json::json!({}));
        let result = registry.run_hooks(HookPoint::PreToolCall, &ctx);
        match result {
            HookResult::Block { reason } => assert_eq!(reason, "denied"),
            HookResult::Continue => panic!("expected Block"),
        }
    }

    #[test]
    fn test_hook_registry_continue_passes_through() {
        let registry = HookRegistry::new();
        registry.register(
            HookPoint::PreToolCall,
            0,
            Arc::new(NoOpHookHandler),
        );
        let ctx = HookContext::new("agent1", "cas.read", serde_json::json!({}));
        let result = registry.run_hooks(HookPoint::PreToolCall, &ctx);
        assert!(matches!(result, HookResult::Continue));
    }

    #[test]
    fn test_hook_priority_ordering() {
        let registry = HookRegistry::new();
        let call_order = Arc::new(std::sync::Mutex::new(Vec::new()));

        let call_order_clone = call_order.clone();
        struct OrderHandler(Arc<std::sync::Mutex<Vec<i32>>>, i32);
        impl HookHandler for OrderHandler {
            fn handle(&self, _point: HookPoint, _context: &HookContext) -> HookResult {
                self.0.lock().unwrap().push(self.1);
                HookResult::Continue
            }
        }

        registry.register(HookPoint::PreToolCall, 10, Arc::new(OrderHandler(call_order.clone(), 10)));
        registry.register(HookPoint::PreToolCall, 0, Arc::new(OrderHandler(call_order.clone(), 0)));
        registry.register(HookPoint::PreToolCall, 5, Arc::new(OrderHandler(call_order.clone(), 5)));

        let ctx = HookContext::new("a", "t", serde_json::json!({}));
        registry.run_hooks(HookPoint::PreToolCall, &ctx);

        let order = call_order.lock().unwrap();
        assert_eq!(*order, vec![0, 5, 10], "hooks should run in priority order (low to high)");
    }

    #[test]
    fn test_hook_multiple_hooks_first_block_wins() {
        let registry = HookRegistry::new();

        registry.register(
            HookPoint::PreToolCall,
            0,
            Arc::new(NoOpHookHandler),
        );
        registry.register(
            HookPoint::PreToolCall,
            5,
            Arc::new(TestHandler { result: HookResult::Block { reason: "blocked at priority 5".into() } }),
        );
        registry.register(
            HookPoint::PreToolCall,
            10,
            Arc::new(TestHandler { result: HookResult::Block { reason: "never reached".into() } }),
        );

        let ctx = HookContext::new("a", "t", serde_json::json!({}));
        let result = registry.run_hooks(HookPoint::PreToolCall, &ctx);
        match result {
            HookResult::Block { reason } => assert_eq!(reason, "blocked at priority 5"),
            HookResult::Continue => panic!("expected Block"),
        }
    }

    #[test]
    fn test_hook_different_points_independent() {
        let registry = HookRegistry::new();
        registry.register(
            HookPoint::PreToolCall,
            0,
            Arc::new(TestHandler { result: HookResult::Block { reason: "pre blocked".into() } }),
        );
        registry.register(
            HookPoint::PostToolCall,
            0,
            Arc::new(NoOpHookHandler),
        );

        let pre_ctx = HookContext::new("a", "t", serde_json::json!({}));
        let post_ctx = HookContext::new("a", "t", serde_json::json!({"result": "ok"}));

        let pre_result = registry.run_hooks(HookPoint::PreToolCall, &pre_ctx);
        let post_result = registry.run_hooks(HookPoint::PostToolCall, &post_ctx);

        assert!(matches!(pre_result, HookResult::Block { .. }));
        assert!(matches!(post_result, HookResult::Continue));
    }
}
