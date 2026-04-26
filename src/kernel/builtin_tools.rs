//! Built-in Tool Registration and Execution
//!
//! Registers all kernel capabilities as discoverable tools ("Everything is a Tool")
//! and dispatches tool calls to the appropriate kernel methods.

use crate::tool::{ToolDescriptor, ToolResult};

use super::AIKernel;

impl AIKernel {
    /// Register all built-in kernel capabilities as discoverable tools.
    pub(crate) fn register_builtin_tools(&self) {
        use serde_json::json;
        let reg = &self.tool_registry;

        reg.register(ToolDescriptor {
            name: "cas.create".into(),
            description: "Create a CAS object with content and semantic tags".into(),
            schema: json!({"type":"object","properties":{"content":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"intent":{"type":"string"}},"required":["content","tags"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.read".into(),
            description: "Read a CAS object by its content ID".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.search".into(),
            description: "Semantic search across stored objects".into(),
            schema: json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"},"require_tags":{"type":"array","items":{"type":"string"}},"exclude_tags":{"type":"array","items":{"type":"string"}},"since":{"type":"integer"},"until":{"type":"integer"}},"required":["query"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.update".into(),
            description: "Update an existing CAS object".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"content":{"type":"string"},"new_tags":{"type":"array","items":{"type":"string"}}},"required":["cid","content"]}),
        });
        reg.register(ToolDescriptor {
            name: "cas.delete".into(),
            description: "Soft-delete a CAS object (moves to recycle bin)".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "memory.store".into(),
            description: "Store a memory entry for an agent".into(),
            schema: json!({"type":"object","properties":{"content":{"type":"string"},"tier":{"type":"string","description":"Tier: ephemeral/working/long-term/procedural (default: working)"},"tags":{"type":"array","items":{"type":"string"}},"importance":{"type":"number"}},"required":["content"]}),
        });
        reg.register(ToolDescriptor {
            name: "memory.recall".into(),
            description: "Retrieve all memories for an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string","description":"Agent ID to query (defaults to calling agent)"},"tier":{"type":"string","description":"Filter by tier: ephemeral/working/long-term/procedural"},"limit":{"type":"integer"}}}),
        });
        reg.register(ToolDescriptor {
            name: "memory.forget".into(),
            description: "Evict ephemeral memories for an agent".into(),
            schema: json!({"type":"object","properties":{}}),
        });
        reg.register(ToolDescriptor {
            name: "kg.add_node".into(),
            description: "Create a knowledge graph node".into(),
            schema: json!({"type":"object","properties":{"label":{"type":"string"},"type":{"type":"string","enum":["entity","fact","document","agent","memory"]},"properties":{"type":"object"}},"required":["label"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.add_edge".into(),
            description: "Create a knowledge graph edge between two nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"type":{"type":"string"},"weight":{"type":"number"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.explore".into(),
            description: "Explore knowledge graph neighbors of a node".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"edge_type":{"type":"string"},"depth":{"type":"integer"}},"required":["cid"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.paths".into(),
            description: "Find paths between two knowledge graph nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"depth":{"type":"integer"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.get_node".into(),
            description: "Get a single knowledge graph node by ID".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.list_edges".into(),
            description: "List knowledge graph edges, optionally filtered by node".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}}}),
        });
        reg.register(ToolDescriptor {
            name: "kg.remove_node".into(),
            description: "Remove a knowledge graph node and all its edges".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.remove_edge".into(),
            description: "Remove an edge between two knowledge graph nodes".into(),
            schema: json!({"type":"object","properties":{"src":{"type":"string"},"dst":{"type":"string"},"type":{"type":"string"}},"required":["src","dst"]}),
        });
        reg.register(ToolDescriptor {
            name: "kg.update_node".into(),
            description: "Update a knowledge graph node's label and/or properties".into(),
            schema: json!({"type":"object","properties":{"node_id":{"type":"string"},"label":{"type":"string"},"properties":{"type":"object"}},"required":["node_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.complete".into(),
            description: "Mark an agent as completed (terminal state)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.fail".into(),
            description: "Mark an agent as failed with a reason (terminal state)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"reason":{"type":"string"}},"required":["agent_id","reason"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.register".into(),
            description: "Register a new AI agent".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.status".into(),
            description: "Query the state of an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.suspend".into(),
            description: "Suspend a running agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.resume".into(),
            description: "Resume a suspended agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.terminate".into(),
            description: "Permanently terminate an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "tools.list".into(),
            description: "List all available tools with their schemas".into(),
            schema: json!({"type":"object","properties":{}}),
        });
        reg.register(ToolDescriptor {
            name: "tools.describe".into(),
            description: "Get the schema and description of a specific tool".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}),
        });
        reg.register(ToolDescriptor {
            name: "agent.set_resources".into(),
            description: "Set an agent's resource limits (memory quota, CPU time, tool allowlist)".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"memory_quota":{"type":"integer"},"cpu_time_quota":{"type":"integer"},"allowed_tools":{"type":"array","items":{"type":"string"}}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "message.send".into(),
            description: "Send a message to another agent".into(),
            schema: json!({"type":"object","properties":{"to":{"type":"string"},"payload":{"type":"object"}},"required":["to","payload"]}),
        });
        reg.register(ToolDescriptor {
            name: "message.read".into(),
            description: "Read messages for an agent".into(),
            schema: json!({"type":"object","properties":{"unread_only":{"type":"boolean"}}}),
        });
        reg.register(ToolDescriptor {
            name: "message.ack".into(),
            description: "Acknowledge a message (mark as read)".into(),
            schema: json!({"type":"object","properties":{"message_id":{"type":"string"}},"required":["message_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "context.load".into(),
            description: "Load context at L0 (summary), L1 (key sections), or L2 (full content) for a CID".into(),
            schema: json!({"type":"object","properties":{"cid":{"type":"string"},"layer":{"type":"string","enum":["L0","L1","L2"]}},"required":["cid","layer"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.grant".into(),
            description: "Grant a permission action to an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string","enum":["Read","ReadAny","Write","Delete","Network","Execute","SendMessage","All"]},"scope":{"type":"string"},"expires_at":{"type":"integer"}},"required":["agent_id","action"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.revoke".into(),
            description: "Revoke a specific permission from an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string"}},"required":["agent_id","action"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.list".into(),
            description: "List all permission grants for an agent".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"}},"required":["agent_id"]}),
        });
        reg.register(ToolDescriptor {
            name: "permission.check".into(),
            description: "Check if an agent has permission for a specific action".into(),
            schema: json!({"type":"object","properties":{"agent_id":{"type":"string"},"action":{"type":"string"}},"required":["agent_id","action"]}),
        });
        self.tool_registry.register(ToolDescriptor {
            name: "memory.store_procedure".into(),
            description: "Store a learned procedure (workflow/skill) in procedural memory (L3)".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string"},"description":{"type":"string"},"steps":{"type":"array","items":{"type":"object","properties":{"description":{"type":"string"},"action":{"type":"string"},"expected_outcome":{"type":"string"}},"required":["description","action"]}},"learned_from":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}}},"required":["name","description","steps"]}),
        });
        self.tool_registry.register(ToolDescriptor {
            name: "memory.recall_procedure".into(),
            description: "Retrieve procedural memories (learned workflows/skills) for an agent".into(),
            schema: json!({"type":"object","properties":{"name":{"type":"string","description":"Optional: filter by procedure name"}},"required":[]}),
        });
    }

    /// Execute a tool by name with JSON parameters.
    ///
    /// Enforces agent tool allowlist: if the agent has a non-empty `allowed_tools`
    /// list, only those tools can be called. Returns error for blocked/unknown tools.
    pub fn execute_tool(&self, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
        let ctx = crate::kernel::hook::HookContext::new(agent_id, name, params.clone());
        if let crate::kernel::hook::HookResult::Block { reason } =
            self.hook_registry.run_hooks(crate::kernel::hook::HookPoint::PreToolCall, &ctx)
        {
            return ToolResult::error(format!("blocked by hook: {}", reason));
        }

        let result = self.execute_tool_impl(name, params, agent_id);

        let mut post_ctx = ctx;
        post_ctx.params = serde_json::to_value(&result).unwrap_or(params.clone());
        self.hook_registry.run_hooks(crate::kernel::hook::HookPoint::PostToolCall, &post_ctx);

        result
    }

    /// Internal tool dispatch — routes to category-specific handler modules.
    fn execute_tool_impl(&self, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
        let aid = crate::scheduler::AgentId(agent_id.to_string());
        if let Some(resources) = self.scheduler.get_resources(&aid) {
            if !resources.allowed_tools.is_empty()
                && !resources.allowed_tools.iter().any(|t| t == name)
            {
                return ToolResult::error(format!(
                    "Tool '{}' not in agent's allowed list: {:?}",
                    name, resources.allowed_tools
                ));
            }
        }

        self.scheduler.record_tool_call(&aid);

        if let Some(handler) = self.tool_registry.get_handler(name) {
            let ctx = crate::api::permission::PermissionContext::new(agent_id.to_string(), "default".to_string());
            let scope = format!("tool:{}", name);
            if self.permissions.check_scoped(&ctx, crate::api::permission::PermissionAction::Execute, Some(&scope)).is_err() {
                return ToolResult::error(format!(
                    "Agent '{}' lacks Execute permission for external tool '{}'", agent_id, name
                ));
            }
            return handler.execute(params, agent_id);
        }

        match name {
            n if n.starts_with("cas.") => super::tools::cas::handle(self, name, params, agent_id),
            n if n.starts_with("memory.") => super::tools::memory::handle(self, name, params, agent_id),
            n if n.starts_with("kg.") => super::tools::graph::handle(self, name, params, agent_id),
            n if n.starts_with("agent.") => super::tools::agent::handle(self, name, params, agent_id),
            n if n.starts_with("tools.") => super::tools::system::handle(self, name, params, agent_id),
            n if n.starts_with("message.") => super::tools::messaging::handle(self, name, params, agent_id),
            n if n.starts_with("context.") => super::tools::system::handle(self, name, params, agent_id),
            n if n.starts_with("permission.") => super::tools::permission::handle(self, name, params, agent_id),
            _ => ToolResult::error(format!("unknown tool: {}", name)),
        }
    }

    /// Number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tool_registry.count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    fn dispatch(kernel: &AIKernel, name: &str, params: serde_json::Value, agent_id: &str) -> ToolResult {
        kernel.execute_tool(name, &params, agent_id)
    }

    // ─── CAS Tools ─────────────────────────────────────────────────────────────

    #[test]
    fn test_cas_create_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "test data", "tags": ["unit-test"]}),
            "kernel");
        assert!(result.success, "cas.create should succeed: {:?}", result.error);
        assert!(result.output["cid"].is_string());
        let cid = result.output["cid"].as_str().unwrap();
        assert!(!cid.is_empty());
    }

    #[test]
    fn test_cas_create_empty_content_rejected() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "", "tags": []}),
            "kernel");
        if !result.success {
            assert!(result.error.is_some(), "error should be set on failure");
        }
    }

    #[test]
    fn test_cas_read_existing() {
        let (kernel, _dir) = make_kernel();
        let create = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "read me", "tags": ["test"]}),
            "kernel");
        let cid = create.output["cid"].as_str().unwrap();

        let result = dispatch(&kernel, "cas.read",
            serde_json::json!({"cid": cid}),
            "kernel");
        assert!(result.success, "cas.read should succeed: {:?}", result.error);
        assert_eq!(result.output["data"].as_str().unwrap(), "read me");
    }

    #[test]
    fn test_cas_read_nonexistent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.read",
            serde_json::json!({"cid": "0000000000000000000000000000000000000000000000000000000000000000"}),
            "kernel");
        assert!(!result.success, "cas.read of nonexistent should fail");
    }

    #[test]
    fn test_cas_delete_existing() {
        let (kernel, _dir) = make_kernel();
        let create = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "to delete", "tags": ["test"]}),
            "kernel");
        let cid = create.output["cid"].as_str().unwrap();

        let result = dispatch(&kernel, "cas.delete",
            serde_json::json!({"cid": cid}),
            "kernel");
        assert!(result.success, "cas.delete should succeed: {:?}", result.error);
    }

    #[test]
    fn test_cas_delete_nonexistent_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.delete",
            serde_json::json!({"cid": "0000000000000000000000000000000000000000000000000000000000000000"}),
            "kernel");
        assert!(!result.success, "cas.delete of nonexistent should fail");
    }

    // ─── Memory Tools ───────────────────────────────────────────────────────────

    #[test]
    fn test_memory_store_working() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "memory.store",
            serde_json::json!({"content": "working memory", "tier": "working", "importance": 50}),
            "TestAgent");
        assert!(result.success, "memory.store working should succeed: {:?}", result.error);
        assert_eq!(result.output["tier"].as_str().unwrap(), "working");
    }

    #[test]
    fn test_memory_recall_agent_name_resolution() {
        let (kernel, _dir) = make_kernel();
        kernel.register_agent("RecallAgent".to_string());
        dispatch(&kernel, "memory.store",
            serde_json::json!({"content": "recallable", "tier": "working"}),
            "RecallAgent");

        let result = dispatch(&kernel, "memory.recall",
            serde_json::json!({"agent_id": "RecallAgent"}),
            "RecallAgent");
        assert!(result.success, "memory.recall by name should resolve: {:?}", result.error);
    }

    #[test]
    fn test_memory_recall_nonexistent_agent() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "memory.recall",
            serde_json::json!({"agent_id": "DoesNotExist"}),
            "kernel");
        assert!(!result.success, "memory.recall for nonexistent agent should fail");
    }

    // ─── KG Tools ──────────────────────────────────────────────────────────────

    #[test]
    fn test_kg_add_node_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "TestNode", "type": "entity", "properties": {}}),
            "kernel");
        assert!(result.success, "kg.add_node should succeed: {:?}", result.error);
        assert!(result.output["node_id"].is_string());
    }

    #[test]
    fn test_kg_add_edge_dispatch() {
        let (kernel, _dir) = make_kernel();
        let n1 = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "Node1", "type": "entity"}),
            "kernel");
        let n2 = dispatch(&kernel, "kg.add_node",
            serde_json::json!({"label": "Node2", "type": "entity"}),
            "kernel");
        let node1 = n1.output["node_id"].as_str().unwrap();
        let node2 = n2.output["node_id"].as_str().unwrap();

        let result = dispatch(&kernel, "kg.add_edge",
            serde_json::json!({"src": node1, "dst": node2, "type": "related_to"}),
            "kernel");
        assert!(result.success, "kg.add_edge should succeed: {:?}", result.error);
    }

    // ─── Agent Tools ───────────────────────────────────────────────────────────

    #[test]
    fn test_agent_register_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "agent.register",
            serde_json::json!({"name": "DispatchTestAgent"}),
            "kernel");
        assert!(result.success, "agent.register should succeed: {:?}", result.error);
        assert!(result.output["agent_id"].is_string());
    }

    #[test]
    fn test_agent_status_dispatch() {
        let (kernel, _dir) = make_kernel();
        let reg = dispatch(&kernel, "agent.register",
            serde_json::json!({"name": "StatusTestAgent"}),
            "kernel");
        let agent_id = reg.output["agent_id"].as_str().unwrap();

        let result = dispatch(&kernel, "agent.status",
            serde_json::json!({"agent_id": agent_id}),
            "kernel");
        assert!(result.success, "agent.status should succeed: {:?}", result.error);
        assert_eq!(result.output["agent_id"].as_str().unwrap(), agent_id);
    }

    // ─── Tool Registry ─────────────────────────────────────────────────────────

    #[test]
    fn test_tools_list_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "tools.list",
            serde_json::json!({}),
            "kernel");
        assert!(result.success, "tools.list should succeed: {:?}", result.error);
        assert!(result.output.is_array());
    }

    #[test]
    fn test_tools_describe_unknown_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "tools.describe",
            serde_json::json!({"name": "nonexistent.tool"}),
            "kernel");
        assert!(!result.success, "tools.describe for unknown tool should fail");
    }

    #[test]
    fn test_unknown_tool_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "nonexistent.tool",
            serde_json::json!({}),
            "kernel");
        assert!(!result.success, "unknown tool should return error");
    }

    // ─── Hook Integration Tests ───────────────────────────────────────────────

    struct BlockHook;
    impl crate::kernel::hook::HookHandler for BlockHook {
        fn handle(&self, _point: crate::kernel::hook::HookPoint, _ctx: &crate::kernel::hook::HookContext) -> crate::kernel::hook::HookResult {
            crate::kernel::hook::HookResult::Block { reason: "blocked by test hook".into() }
        }
    }

    struct NoOpTestHook;
    impl crate::kernel::hook::HookHandler for NoOpTestHook {
        fn handle(&self, _point: crate::kernel::hook::HookPoint, _ctx: &crate::kernel::hook::HookContext) -> crate::kernel::hook::HookResult {
            crate::kernel::hook::HookResult::Continue
        }
    }

    struct RecordingHook {
        calls: std::sync::Arc<std::sync::Mutex<Vec<(crate::kernel::hook::HookPoint, String)>>>,
    }
    impl RecordingHook {
        fn new() -> Self {
            Self { calls: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())) }
        }
    }
    impl crate::kernel::hook::HookHandler for RecordingHook {
        fn handle(&self, point: crate::kernel::hook::HookPoint, ctx: &crate::kernel::hook::HookContext) -> crate::kernel::hook::HookResult {
            self.calls.lock().unwrap().push((point, ctx.tool_name.clone()));
            crate::kernel::hook::HookResult::Continue
        }
    }

    #[test]
    fn test_hook_registry_block_prevents_tool_call() {
        let (kernel, _dir) = make_kernel();
        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PreToolCall,
            0,
            std::sync::Arc::new(BlockHook),
        );

        let result = dispatch(&kernel, "cas.delete",
            serde_json::json!({"cid": "nonexistent"}),
            "kernel");

        assert!(!result.success, "blocked tool should fail");
        let err_msg = result.error.unwrap_or_default();
        assert!(err_msg.contains("blocked by hook"),
            "error should mention hook blocking: {}", err_msg);
    }

    #[test]
    fn test_hook_registry_continue_allows_execution() {
        let (kernel, _dir) = make_kernel();
        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PreToolCall,
            0,
            std::sync::Arc::new(NoOpTestHook),
        );

        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "hook test", "tags": ["test"]}),
            "kernel");

        assert!(result.success, "noop hook should allow tool call: {:?}", result.error);
    }

    #[test]
    fn test_post_tool_call_receives_result() {
        let (kernel, _dir) = make_kernel();
        let recorder = std::sync::Arc::new(RecordingHook::new());
        let recorder_clone = recorder.clone();

        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PostToolCall,
            0,
            recorder_clone,
        );

        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "post hook test", "tags": []}),
            "kernel");

        assert!(result.success);
        let calls = recorder.calls.lock().unwrap();
        assert!(!calls.is_empty(), "PostToolCall should have been called");
        let (_point, tool_name) = &calls[0];
        assert_eq!(tool_name, "cas.create", "PostToolCall should receive tool name");
    }

    #[test]
    fn test_multiple_hooks_first_block_wins_in_execute() {
        let (kernel, _dir) = make_kernel();
        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PreToolCall,
            5,
            std::sync::Arc::new(NoOpTestHook),
        );
        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PreToolCall,
            0,
            std::sync::Arc::new(BlockHook),
        );

        let result = dispatch(&kernel, "cas.read",
            serde_json::json!({"cid": "abc"}),
            "kernel");

        assert!(!result.success, "first blocking hook should win");
        assert!(result.error.unwrap_or_default().contains("blocked by hook"));
    }

    #[test]
    fn test_hook_registry_empty_passes_through() {
        let (kernel, _dir) = make_kernel();
        let result = dispatch(&kernel, "cas.create",
            serde_json::json!({"content": "no hooks", "tags": ["test"]}),
            "kernel");

        assert!(result.success, "no hooks should not affect execution: {:?}", result.error);
    }

    #[test]
    fn test_hook_receives_correct_context() {
        let (kernel, _dir) = make_kernel();
        let recorder = std::sync::Arc::new(RecordingHook::new());
        let recorder_clone = recorder.clone();

        kernel.hook_registry.register(
            crate::kernel::hook::HookPoint::PreToolCall,
            0,
            recorder_clone,
        );

        let _ = dispatch(&kernel, "cas.search",
            serde_json::json!({"query": "test query", "limit": 5}),
            "agent-42");

        let calls = recorder.calls.lock().unwrap();
        let (point, tool_name) = &calls[0];
        assert_eq!(*point, crate::kernel::hook::HookPoint::PreToolCall);
        assert_eq!(tool_name, "cas.search");
    }
}
