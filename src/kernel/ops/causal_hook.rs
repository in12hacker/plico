//! Causal Hook Handler (F-3, Node 20) — Hook events write KG因果边.
//!
//! On PostToolCall, automatically creates KG nodes and `CausedBy` edges
//! connecting tool calls to their triggering intents.

use std::sync::Arc;
use crate::fs::graph::{KnowledgeGraph, KGEdgeType, KGNode, KGNodeType};
use crate::kernel::hook::{HookContext, HookHandler, HookPoint, HookResult};
use crate::kernel::ops::session::SessionStore;

/// Causal Hook Handler — writes KG edges on PostToolCall events.
///
/// Tracks "this tool was called because of this intent" causal chains.
/// Registered as a PostToolCall hook at low priority (runs after tool completes).
pub struct CausalHookHandler {
    /// Reference to the knowledge graph for edge creation.
    kg: Arc<dyn KnowledgeGraph>,
    /// Reference to session store to look up current intent.
    session_store: Arc<SessionStore>,
}

impl CausalHookHandler {
    pub fn new(kg: Arc<dyn KnowledgeGraph>, session_store: Arc<SessionStore>) -> Self {
        Self { kg, session_store }
    }

    /// Record causal chain: tool call was caused by intent.
    fn record_causal_chain(&self, context: &HookContext) -> HookResult {
        // Get current intent for this agent from session store
        let intent = self.get_current_intent(&context.agent_id);

        // Create tool call node
        let tool_node_id = self.upsert_tool_call_node(context);

        // If we have an intent, create intent node and edge
        if let Some(intent_text) = intent {
            let intent_node_id = self.upsert_intent_node(&intent_text, &context.agent_id);
            self.add_caused_by_edge(&tool_node_id, &intent_node_id);
        }

        HookResult::Continue
    }

    /// Get current intent for an agent from session store.
    fn get_current_intent(&self, agent_id: &str) -> Option<String> {
        // Find the active session for this agent and get its current_intent
        let sessions = self.session_store.list();
        sessions
            .into_iter()
            .filter(|s| s.agent_id == agent_id)
            .find_map(|s| s.current_intent)
    }

    /// Upsert a tool call node in KG.
    fn upsert_tool_call_node(&self, context: &HookContext) -> String {
        // Use agent_id:tool_name:timestamp as unique node ID
        let node_id = format!(
            "toolcall:{}:{}:{}",
            context.agent_id, context.tool_name, context.timestamp_ms
        );

        let node = KGNode::with_content(
            format!("ToolCall: {}", context.tool_name),
            KGNodeType::Fact,
            context.params.to_string(), // Store params as content
            context.agent_id.clone(),
            crate::DEFAULT_TENANT.to_string(),
        );

        // Only add if node doesn't exist (ignore error if it already exists)
        if let Ok(None) = self.kg.get_node(&node_id) {
            if let Err(e) = self.kg.add_node(node) {
                tracing::debug!("causal hook: add_node failed: {}", e);
            }
        }

        node_id
    }

    /// Upsert an intent node in KG.
    fn upsert_intent_node(&self, intent: &str, agent_id: &str) -> String {
        // Use hashed intent as node ID for consistency
        let node_id = format!("intent:{}", simple_hash(intent));

        let node = KGNode::with_content(
            format!("Intent: {}", intent),
            KGNodeType::Fact,
            intent.to_string(),
            agent_id.to_string(),
            crate::DEFAULT_TENANT.to_string(),
        );

        // Only add if node doesn't exist
        if let Ok(None) = self.kg.get_node(&node_id) {
            if let Err(e) = self.kg.add_node(node) {
                tracing::debug!("causal hook: add_node failed: {}", e);
            }
        }

        node_id
    }

    /// Add CausedBy edge from tool node to intent node.
    fn add_caused_by_edge(&self, tool_node_id: &str, intent_node_id: &str) {
        let edge = crate::fs::graph::KGEdge::new(
            tool_node_id.to_string(),
            intent_node_id.to_string(),
            KGEdgeType::CausedBy,
            1.0, // High confidence for direct causal relationship
        );

        if let Err(e) = self.kg.add_edge(edge) {
            tracing::debug!("causal hook: add_edge failed: {}", e);
        }
    }
}

impl HookHandler for CausalHookHandler {
    fn handle(&self, point: HookPoint, context: &HookContext) -> HookResult {
        if point == HookPoint::PostToolCall {
            self.record_causal_chain(context);
        }
        HookResult::Continue
    }
}

/// Simple string hash for generating consistent node IDs.
fn simple_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::hook::{HookContext, HookResult};
    use std::sync::Arc;

    struct FakeKG {
        nodes: std::sync::Mutex<std::collections::HashMap<String, KGNode>>,
        edges: std::sync::Mutex<Vec<crate::fs::graph::KGEdge>>,
    }

    impl FakeKG {
        fn new() -> Self {
            Self {
                nodes: std::sync::Mutex::new(std::collections::HashMap::new()),
                edges: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl KnowledgeGraph for FakeKG {
        fn add_node(&self, node: KGNode) -> Result<(), crate::fs::graph::KGError> {
            self.nodes.lock().unwrap().insert(node.id.clone(), node);
            Ok(())
        }
        fn add_edge(&self, edge: crate::fs::graph::KGEdge) -> Result<(), crate::fs::graph::KGError> {
            self.edges.lock().unwrap().push(edge);
            Ok(())
        }
        fn get_node(&self, id: &str) -> Result<Option<KGNode>, crate::fs::graph::KGError> {
            Ok(self.nodes.lock().unwrap().get(id).cloned())
        }
        fn list_edges(&self, _agent_id: &str) -> Result<Vec<crate::fs::graph::KGEdge>, crate::fs::graph::KGError> {
            Ok(self.edges.lock().unwrap().clone())
        }
        fn get_neighbors(&self, _id: &str, _edge_type: Option<KGEdgeType>, _depth: u8) -> Result<Vec<(KGNode, crate::fs::graph::KGEdge)>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn find_paths(&self, _src: &str, _dst: &str, _max_depth: u8) -> Result<Vec<Vec<KGNode>>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn find_weighted_path(&self, _src: &str, _dst: &str, _max_depth: u8) -> Result<Option<Vec<KGNode>>, crate::fs::graph::KGError> {
            Ok(None)
        }
        fn list_nodes(&self, _agent_id: &str, _node_type: Option<KGNodeType>) -> Result<Vec<KGNode>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn remove_node(&self, _id: &str) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
        fn remove_edge(&self, _src: &str, _dst: &str, _edge_type: Option<KGEdgeType>) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
        fn update_node(&self, _id: &str, _label: Option<&str>, _properties: Option<serde_json::Value>) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
        fn all_node_ids(&self) -> Vec<String> {
            Vec::new()
        }
        fn upsert_document(&self, _cid: &str, _tags: &[String], _agent_id: &str) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
        fn authority_score(&self, _node_id: &str) -> Result<f32, crate::fs::graph::KGError> {
            Ok(0.0)
        }
        fn node_count(&self) -> Result<usize, crate::fs::graph::KGError> {
            Ok(self.nodes.lock().unwrap().len())
        }
        fn edge_count(&self) -> Result<usize, crate::fs::graph::KGError> {
            Ok(self.edges.lock().unwrap().len())
        }
        fn get_valid_edges_at(&self, _t: u64) -> Result<Vec<crate::fs::graph::KGEdge>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn get_valid_edge_between(&self, _src: &str, _dst: &str, _edge_type: Option<KGEdgeType>, _t: u64) -> Result<Option<crate::fs::graph::KGEdge>, crate::fs::graph::KGError> {
            Ok(None)
        }
        fn invalidate_conflicts(&self, _new_edge: &crate::fs::graph::KGEdge) -> Result<usize, crate::fs::graph::KGError> {
            Ok(0)
        }
        fn edge_history(&self, _src: &str, _dst: &str, _edge_type: Option<KGEdgeType>) -> Result<Vec<crate::fs::graph::KGEdge>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn get_valid_nodes_at(&self, _agent_id: &str, _node_type: Option<KGNodeType>, _t: u64) -> Result<Vec<KGNode>, crate::fs::graph::KGError> {
            Ok(Vec::new())
        }
        fn save_to_disk(&self, _path: &std::path::Path) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
        fn load_from_disk(&self, _path: &std::path::Path) -> Result<(), crate::fs::graph::KGError> {
            Ok(())
        }
    }

    #[test]
    fn causal_hook_handler_creates_caused_by_edge() {
        let kg = Arc::new(FakeKG::new());
        let session_store = Arc::new(SessionStore::new());

        // Start a session with a current intent
        let _session_id = session_store.start_session(
            "session-1".to_string(),
            "agent-1".to_string(),
            0,
        );
        session_store.set_current_intent("agent-1", Some("fix authentication bug".to_string()));

        let handler = CausalHookHandler::new(kg.clone(), session_store.clone());

        let context = HookContext::new(
            "agent-1",
            "cas.search",
            serde_json::json!({ "query": "auth" }),
        );

        let result = handler.handle(HookPoint::PostToolCall, &context);
        assert!(matches!(result, HookResult::Continue));

        // Verify edge was created
        let edges = kg.edges.lock().unwrap();
        assert!(!edges.is_empty(), "expected at least one edge");
        assert_eq!(edges[0].edge_type, KGEdgeType::CausedBy);
    }

    #[test]
    fn causal_hook_handler_no_intent_no_edge() {
        let kg = Arc::new(FakeKG::new());
        let session_store = Arc::new(SessionStore::new());

        // No session with intent
        let handler = CausalHookHandler::new(kg.clone(), session_store.clone());

        let context = HookContext::new(
            "agent-1",
            "cas.search",
            serde_json::json!({ "query": "auth" }),
        );

        let result = handler.handle(HookPoint::PostToolCall, &context);
        assert!(matches!(result, HookResult::Continue));

        // Verify no edges were created (no intent)
        let edges = kg.edges.lock().unwrap();
        assert!(edges.is_empty(), "expected no edges without intent");
    }

    #[test]
    fn causal_hook_does_not_block_tool_execution() {
        let kg = Arc::new(FakeKG::new());
        let session_store = Arc::new(SessionStore::new());
        let handler = CausalHookHandler::new(kg.clone(), session_store.clone());

        let context = HookContext::new(
            "agent-1",
            "cas.create",
            serde_json::json!({}),
        );

        // Should always return Continue, never Block
        let result = handler.handle(HookPoint::PostToolCall, &context);
        assert!(matches!(result, HookResult::Continue));

        let result_pre = handler.handle(HookPoint::PreToolCall, &context);
        assert!(matches!(result_pre, HookResult::Continue)); // Should also be Continue for PreToolCall
    }
}