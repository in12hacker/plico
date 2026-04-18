//! Tool Registry — agent-discoverable capability catalog.
//!
//! Holds descriptors for all registered tools. The kernel populates
//! built-in tools at startup; external tools can be registered via API.
//!
//! Execution is NOT handled here — the kernel's `execute_tool()` method
//! dispatches to the appropriate handler. This avoids circular references
//! between registry → tool closures → kernel.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use super::{ToolDescriptor, ToolHandler};

struct RegistryEntry {
    descriptor: ToolDescriptor,
    handler: Option<Arc<dyn ToolHandler>>,
}

/// Central registry of all available tools.
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, RegistryEntry>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: RwLock::new(HashMap::new()) }
    }

    /// Register a tool descriptor (no handler — execution via builtin match).
    pub fn register(&self, desc: ToolDescriptor) {
        self.tools.write().unwrap().insert(desc.name.clone(), RegistryEntry {
            descriptor: desc,
            handler: None,
        });
    }

    /// Register a tool with a dynamic handler.
    pub fn register_with_handler(&self, desc: ToolDescriptor, handler: Arc<dyn ToolHandler>) {
        self.tools.write().unwrap().insert(desc.name.clone(), RegistryEntry {
            descriptor: desc,
            handler: Some(handler),
        });
    }

    /// Look up a tool handler by name.
    pub fn get_handler(&self, name: &str) -> Option<Arc<dyn ToolHandler>> {
        self.tools.read().unwrap().get(name).and_then(|e| e.handler.clone())
    }

    /// Look up a single tool by name.
    pub fn get(&self, name: &str) -> Option<ToolDescriptor> {
        self.tools.read().unwrap().get(name).map(|e| e.descriptor.clone())
    }

    /// List all registered tools (sorted by name for deterministic output).
    pub fn list(&self) -> Vec<ToolDescriptor> {
        let map = self.tools.read().unwrap();
        let mut tools: Vec<ToolDescriptor> = map.values().map(|e| e.descriptor.clone()).collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools
    }

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.read().unwrap().len()
    }

    /// Remove a tool by name. Returns true if it existed.
    pub fn unregister(&self, name: &str) -> bool {
        self.tools.write().unwrap().remove(name).is_some()
    }

    /// Check if a tool name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.tools.read().unwrap().contains_key(name)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use crate::tool::ToolResult;

    fn make_desc(name: &str) -> ToolDescriptor {
        ToolDescriptor {
            name: name.into(),
            description: format!("Test tool: {}", name),
            schema: json!({"type": "object"}),
        }
    }

    #[test]
    fn register_and_get() {
        let reg = ToolRegistry::new();
        reg.register(make_desc("cas.create"));
        assert!(reg.contains("cas.create"));
        let desc = reg.get("cas.create").unwrap();
        assert_eq!(desc.name, "cas.create");
    }

    #[test]
    fn list_returns_sorted() {
        let reg = ToolRegistry::new();
        reg.register(make_desc("memory.store"));
        reg.register(make_desc("cas.create"));
        reg.register(make_desc("agent.register"));
        let list = reg.list();
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["agent.register", "cas.create", "memory.store"]);
    }

    #[test]
    fn unregister_removes_tool() {
        let reg = ToolRegistry::new();
        reg.register(make_desc("x"));
        assert!(reg.unregister("x"));
        assert!(!reg.contains("x"));
        assert_eq!(reg.count(), 0);
    }

    #[test]
    fn get_missing_returns_none() {
        let reg = ToolRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn test_register_handler_and_execute() {
        let reg = ToolRegistry::new();
        let handler: Arc<dyn ToolHandler> = Arc::new(|_params: &serde_json::Value, _agent: &str| {
            ToolResult::ok(json!({"custom": true}))
        });
        reg.register_with_handler(make_desc("custom.tool"), handler);
        let h = reg.get_handler("custom.tool").expect("handler should exist");
        let result = h.execute(&json!({}), "test-agent");
        assert!(result.success);
        assert_eq!(result.output["custom"], true);
    }

    #[test]
    fn test_handler_fallback_to_builtin() {
        let reg = ToolRegistry::new();
        reg.register(make_desc("cas.create"));
        assert!(reg.get_handler("cas.create").is_none(), "descriptor-only should have no handler");
        assert!(reg.get("cas.create").is_some(), "descriptor should still exist");
    }

    #[test]
    fn test_closure_as_handler() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();
        let handler: Arc<dyn ToolHandler> = Arc::new(move |_p: &serde_json::Value, _a: &str| {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            ToolResult::ok(json!({"count": counter_clone.load(std::sync::atomic::Ordering::SeqCst)}))
        });
        let result = handler.execute(&json!({}), "agent");
        assert!(result.success);
        assert_eq!(result.output["count"], 1);
        let result2 = handler.execute(&json!({}), "agent");
        assert_eq!(result2.output["count"], 2);
    }

    #[test]
    fn test_handler_overrides_builtin() {
        let reg = ToolRegistry::new();
        reg.register(make_desc("tools.list"));
        assert!(reg.get_handler("tools.list").is_none());

        let handler: Arc<dyn ToolHandler> = Arc::new(|_p: &serde_json::Value, _a: &str| {
            ToolResult::ok(json!({"overridden": true}))
        });
        reg.register_with_handler(make_desc("tools.list"), handler);
        let h = reg.get_handler("tools.list").expect("should have handler now");
        let result = h.execute(&json!({}), "agent");
        assert!(result.success);
        assert_eq!(result.output["overridden"], true);
    }
}
