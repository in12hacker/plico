//! ProcedureToolProvider — learned workflows exposed as callable tools.
//!
//! Implements "Everything is a Tool" for procedural memory: shared procedures
//! are auto-discovered and exposed through the standard ToolRegistry.
//! Any agent can invoke a learned workflow without knowing who learned it.

use std::sync::Arc;
use crate::tool::{ExternalToolProvider, ToolDescriptor, ToolResult, ToolSchema};
use crate::memory::{LayeredMemory, MemoryContent, MemoryTier};
use crate::api::semantic::ApiRequest;

pub struct ProcedureToolProvider {
    memory: Arc<LayeredMemory>,
    api_dispatch: Arc<dyn Fn(ApiRequest) -> bool + Send + Sync>,
}

impl ProcedureToolProvider {
    pub fn new(
        memory: Arc<LayeredMemory>,
        api_dispatch: Arc<dyn Fn(ApiRequest) -> bool + Send + Sync>,
    ) -> Self {
        Self { memory, api_dispatch }
    }
}

impl ExternalToolProvider for ProcedureToolProvider {
    fn provider_name(&self) -> &str {
        "procedures"
    }

    fn discover_tools(&self) -> Vec<ToolDescriptor> {
        let shared = self.memory.get_shared(MemoryTier::Procedural);

        shared.iter().filter_map(|entry| {
            if !entry.tags.iter().any(|t| t == "verified") {
                return None;
            }
            match &entry.content {
                MemoryContent::Procedure(proc) => {
                    let schema: ToolSchema = serde_json::json!({
                        "type": "object",
                        "properties": {
                            "agent_id": {
                                "type": "string",
                                "description": "The agent invoking this procedure"
                            }
                        },
                        "required": ["agent_id"]
                    });
                    Some(ToolDescriptor {
                        name: proc.name.clone(),
                        description: format!(
                            "{} ({} steps, learned by {})",
                            proc.description,
                            proc.steps.len(),
                            entry.agent_id,
                        ),
                        schema,
                    })
                }
                _ => None,
            }
        }).collect()
    }

    fn call_tool(&self, name: &str, _params: &serde_json::Value) -> ToolResult {
        let shared = self.memory.get_shared(MemoryTier::Procedural);

        let procedure = shared.iter().find_map(|entry| {
            match &entry.content {
                MemoryContent::Procedure(proc) if proc.name == name => {
                    if entry.tags.iter().any(|t| t == "verified") {
                        Some(proc.clone())
                    } else {
                        None
                    }
                }
                _ => None,
            }
        });

        let Some(proc) = procedure else {
            return ToolResult::error(format!("Procedure '{}' not found or not verified", name));
        };

        let mut results = Vec::new();
        let mut all_ok = true;

        for step in &proc.steps {
            let action: Result<ApiRequest, _> = serde_json::from_str(&step.action);
            match action {
                Ok(req) => {
                    let ok = (self.api_dispatch)(req);
                    if !ok { all_ok = false; }
                    results.push(serde_json::json!({
                        "step": step.step_number,
                        "description": step.description,
                        "success": ok,
                    }));
                }
                Err(e) => {
                    all_ok = false;
                    results.push(serde_json::json!({
                        "step": step.step_number,
                        "description": step.description,
                        "success": false,
                        "error": e.to_string(),
                    }));
                }
            }
        }

        if all_ok {
            ToolResult::ok(serde_json::json!({
                "procedure": name,
                "steps_executed": results.len(),
                "results": results,
            }))
        } else {
            ToolResult {
                success: false,
                output: serde_json::json!({
                    "procedure": name,
                    "steps_executed": results.len(),
                    "results": results,
                }),
                error: Some("One or more steps failed".into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::{MemoryScope};
    use crate::memory::layered::{MemoryEntry, Procedure, ProcedureStep, now_ms};

    fn make_shared_procedure(name: &str, agent: &str) -> MemoryEntry {
        MemoryEntry {
            id: format!("proc-{}", name),
            agent_id: agent.into(),
            tenant_id: "default".to_string(),
            tier: MemoryTier::Procedural,
            content: MemoryContent::Procedure(Procedure {
                name: name.into(),
                description: format!("Test procedure: {}", name),
                steps: vec![
                    ProcedureStep {
                        step_number: 1,
                        description: "Search for reports".into(),
                        action: serde_json::json!({
                            "method": "search",
                            "query": "report",
                            "agent_id": "test"
                        }).to_string(),
                        expected_outcome: "results found".into(),
                    },
                ],
                learned_from: "test".into(),
            }),
            importance: 100,
            access_count: 0,
            last_accessed: now_ms(),
            created_at: now_ms(),
            tags: vec!["verified".into(), "auto-learned".into()],
            embedding: None,
            ttl_ms: None,
            original_ttl_ms: None,
            scope: MemoryScope::Shared,
        }
    }

    #[test]
    fn test_discover_shared_procedures() {
        let memory = Arc::new(LayeredMemory::new());
        memory.store(make_shared_procedure("deploy-workflow", "agent-a"));

        let unverified = MemoryEntry {
            tags: vec!["auto-learned".into()],
            ..make_shared_procedure("unverified-proc", "agent-b")
        };
        memory.store(unverified);

        let private_proc = MemoryEntry {
            scope: MemoryScope::Private,
            ..make_shared_procedure("private-proc", "agent-c")
        };
        memory.store(private_proc);

        let dispatch = Arc::new(|_req: ApiRequest| true);
        let provider = ProcedureToolProvider::new(memory, dispatch);

        let tools = provider.discover_tools();
        assert_eq!(tools.len(), 1, "only verified+shared procedures become tools");
        assert_eq!(tools[0].name, "deploy-workflow");
        assert!(tools[0].description.contains("agent-a"));
    }

    #[test]
    fn test_call_procedure_tool() {
        let memory = Arc::new(LayeredMemory::new());
        memory.store(make_shared_procedure("test-proc", "agent-a"));

        let dispatch = Arc::new(|_req: ApiRequest| true);
        let provider = ProcedureToolProvider::new(memory, dispatch);

        let result = provider.call_tool("test-proc", &serde_json::json!({"agent_id": "agent-b"}));
        assert!(result.success, "procedure call should succeed");
        assert_eq!(result.output["steps_executed"], 1);
    }

    #[test]
    fn test_call_missing_procedure() {
        let memory = Arc::new(LayeredMemory::new());
        let dispatch = Arc::new(|_req: ApiRequest| true);
        let provider = ProcedureToolProvider::new(memory, dispatch);

        let result = provider.call_tool("nonexistent", &serde_json::json!({}));
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[test]
    fn test_discover_returns_empty_when_no_shared() {
        let memory = Arc::new(LayeredMemory::new());
        let dispatch = Arc::new(|_req: ApiRequest| true);
        let provider = ProcedureToolProvider::new(memory, dispatch);

        assert!(provider.discover_tools().is_empty());
    }
}
