//! Tool Registry — agent-discoverable capability catalog.
//!
//! Holds descriptors for all registered tools. The kernel populates
//! built-in tools at startup; external tools can be registered via API.
//!
//! Execution is NOT handled here — the kernel's `execute_tool()` method
//! dispatches to the appropriate handler. This avoids circular references
//! between registry → tool closures → kernel.

use std::collections::HashMap;
use std::sync::RwLock;

use super::ToolDescriptor;

/// Central registry of all available tools.
pub struct ToolRegistry {
    tools: RwLock<HashMap<String, ToolDescriptor>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: RwLock::new(HashMap::new()) }
    }

    /// Register a tool descriptor. Overwrites if name already exists.
    pub fn register(&self, desc: ToolDescriptor) {
        self.tools.write().unwrap().insert(desc.name.clone(), desc);
    }

    /// Look up a single tool by name.
    pub fn get(&self, name: &str) -> Option<ToolDescriptor> {
        self.tools.read().unwrap().get(name).cloned()
    }

    /// List all registered tools (sorted by name for deterministic output).
    pub fn list(&self) -> Vec<ToolDescriptor> {
        let map = self.tools.read().unwrap();
        let mut tools: Vec<ToolDescriptor> = map.values().cloned().collect();
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
}
