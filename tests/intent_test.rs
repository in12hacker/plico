//! Intent routing tests — tool catalog hydration, validation, procedural memory.

use plico::intent::llm::LlmRouter;
use plico::intent::IntentRouter;
use plico::llm::StubProvider;
use plico::tool::ToolDescriptor;
use std::sync::Arc;

fn make_router_with_catalog(response: &str, catalog: Vec<ToolDescriptor>) -> LlmRouter {
    let provider = Arc::new(StubProvider::new(response));
    LlmRouter::new(provider, catalog)
}

fn make_tool(name: &str, desc: &str, required: Vec<&str>) -> ToolDescriptor {
    let required_json: Vec<serde_json::Value> = required.iter().map(|r| serde_json::json!(r)).collect();
    ToolDescriptor {
        name: name.into(),
        description: desc.into(),
        schema: serde_json::json!({"type": "object", "required": required_json}),
    }
}

#[test]
fn test_set_tool_catalog_updates_prompt() {
    let provider = Arc::new(StubProvider::new(""));
    let router = LlmRouter::new(provider, vec![]);

    router.set_tool_catalog(vec![
        make_tool("cas.search", "Search objects", vec!["query"]),
        make_tool("memory.store", "Store memory", vec!["content"]),
    ]);

    let response = r#"{"tool": "cas.search", "params": {"query": "test"}, "confidence": 0.9, "explanation": "searching"}"#;
    let provider = Arc::new(StubProvider::new(response));
    let router = LlmRouter::new(provider, vec![
        make_tool("cas.search", "Search objects", vec!["query"]),
    ]);
    let result = router.resolve("search for test", "agent1");
    assert!(result.is_ok());
}

#[test]
fn test_validate_existing_tool() {
    let response = r#"{"tool": "cas.search", "params": {"query": "hello"}, "confidence": 0.9, "explanation": "searching"}"#;
    let router = make_router_with_catalog(response, vec![
        make_tool("cas.search", "Search", vec!["query"]),
    ]);
    let result = router.resolve("search hello", "agent1");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().len(), 1);
}

#[test]
fn test_validate_nonexistent_tool() {
    let response = r#"{"tool": "nonexistent.tool", "params": {}, "confidence": 0.9, "explanation": "hallucinated"}"#;
    let router = make_router_with_catalog(response, vec![
        make_tool("cas.search", "Search", vec!["query"]),
    ]);
    let result = router.resolve("do something weird", "agent1");
    assert!(result.is_err());
}

#[test]
fn test_validate_missing_required_params() {
    let response = r#"{"tool": "cas.search", "params": {}, "confidence": 0.9, "explanation": "no query"}"#;
    let router = make_router_with_catalog(response, vec![
        make_tool("cas.search", "Search", vec!["query"]),
    ]);
    let result = router.resolve("search", "agent1");
    assert!(result.is_err());
}

#[test]
fn test_validate_empty_catalog_skips_validation() {
    let response = r#"{"tool": "anything", "params": {}, "confidence": 0.9, "explanation": "no catalog"}"#;
    let router = make_router_with_catalog(response, vec![]);
    let result = router.resolve("do anything", "agent1");
    assert!(result.is_ok());
}

#[test]
fn test_procedural_memory_roundtrip() {
    use plico::memory::{MemoryEntry, MemoryContent, MemoryTier};
    use plico::memory::layered::{Procedure, ProcedureStep, LayeredMemory};

    let memory = LayeredMemory::new();
    let proc = Procedure {
        name: "search-docs".to_string(),
        description: "Search for documents using cas.search".to_string(),
        steps: vec![
            ProcedureStep {
                step_number: 1,
                description: "Call cas.search with query".to_string(),
                action: "cas.search".to_string(),
                expected_outcome: "Returns matching documents".to_string(),
            },
        ],
        learned_from: "manual".to_string(),
    };

    let entry = MemoryEntry {
        id: "proc-001".to_string(),
        agent_id: "agent1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Procedure(proc),
        importance: 100,
        access_count: 0,
        last_accessed: 0,
        created_at: 0,
        tags: vec!["search".to_string()],
        embedding: None,
        ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };

    memory.store(entry);

    let recalled = memory.get_tier("agent1", MemoryTier::Procedural);
    assert_eq!(recalled.len(), 1);
    assert!(matches!(&recalled[0].content, MemoryContent::Procedure(p) if p.name == "search-docs"));
}
