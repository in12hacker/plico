//! LLM Intent Router — sends NL + tool catalog to an LLM for resolution.
//!
//! Model-agnostic — uses `LlmProvider` trait. Falls back gracefully when unavailable.
//! The LLM receives a system prompt with the full tool catalog and
//! returns structured JSON describing the intended actions.

use std::sync::{Arc, RwLock};
use super::{IntentRouter, ResolvedIntent, IntentError, RoutingAction};
use crate::api::semantic::ApiRequest;
use crate::tool::ToolDescriptor;
use crate::llm::{LlmProvider, ChatMessage, ChatOptions};

pub struct LlmRouter {
    provider: Arc<dyn LlmProvider>,
    tool_catalog: RwLock<Vec<ToolDescriptor>>,
}

impl LlmRouter {
    pub fn new(provider: Arc<dyn LlmProvider>, tool_catalog: Vec<ToolDescriptor>) -> Self {
        Self { provider, tool_catalog: RwLock::new(tool_catalog) }
    }

    pub fn set_tool_catalog(&self, catalog: Vec<ToolDescriptor>) {
        *self.tool_catalog.write().unwrap() = catalog;
    }

    fn build_system_prompt(&self) -> String {
        let catalog = self.tool_catalog.read().unwrap();
        let mut tools_desc = String::new();
        for tool in catalog.iter() {
            tools_desc.push_str(&format!("- {}: {}\n", tool.name, tool.description));
        }

        format!(
r#"You are a Plico AI-OS intent resolver. Given a user's natural language request,
determine which API action to take.

Available tools:
{tools_desc}

Respond with ONLY a JSON object:
{{"tool": "<tool_name>", "params": {{}}, "confidence": 0.9, "explanation": "..."}}

If you need to call cas.search, use: {{"tool": "cas.search", "params": {{"query": "...", "limit": 10}}, ...}}
If you need to call memory.store, use: {{"tool": "memory.store", "params": {{"content": "..."}}, ...}}
If the request cannot be mapped to any tool, respond: {{"tool": "none", "confidence": 0.0, "explanation": "..."}}

IMPORTANT: Return ONLY valid JSON, no markdown, no extra text."#
        )
    }

    fn validate_tool_call(&self, tool_name: &str, params: &serde_json::Value) -> Result<(), IntentError> {
        let catalog = self.tool_catalog.read().unwrap();
        if catalog.is_empty() {
            return Ok(());
        }
        let tool = catalog.iter().find(|t| t.name == tool_name)
            .ok_or_else(|| IntentError::Unresolvable(
                format!("Unknown tool '{}' — not in registry", tool_name),
            ))?;

        if let Some(required) = tool.schema.get("required").and_then(|r| r.as_array()) {
            for req in required {
                if let Some(field) = req.as_str() {
                    if params.get(field).is_none() {
                        return Err(IntentError::Unresolvable(
                            format!("Tool '{}' requires parameter '{}' but it was not provided", tool_name, field),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn parse_llm_response(&self, body: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError> {
        let json_str = body.trim().trim_start_matches("```json").trim_end_matches("```").trim();

        let parsed: serde_json::Value = serde_json::from_str(json_str)
            .map_err(|e| IntentError::LlmUnavailable(format!("LLM response parse error: {}", e)))?;

        let tool_name = parsed.get("tool").and_then(|v| v.as_str()).unwrap_or("none");
        let confidence = parsed.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let explanation = parsed.get("explanation").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let params = parsed.get("params").cloned().unwrap_or(serde_json::Value::Object(Default::default()));

        if tool_name == "none" || confidence < 0.1 {
            return Err(IntentError::Unresolvable(explanation));
        }

        self.validate_tool_call(tool_name, &params)?;

        let action = ApiRequest::ToolCall {
            tool: tool_name.to_string(),
            params,
            agent_id: agent_id.to_string(),
        };

        Ok(vec![ResolvedIntent {
            routing_action: RoutingAction::SingleAction,
            confidence,
            action,
            explanation,
        }])
    }
}

impl IntentRouter for LlmRouter {
    fn resolve(&self, text: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError> {
        let system_prompt = self.build_system_prompt();

        let messages = vec![
            ChatMessage::system(system_prompt),
            ChatMessage::user(text),
        ];
        let options = ChatOptions { temperature: 0.1, max_tokens: None };

        let content = self.provider
            .chat(&messages, &options)
            .map_err(|e| IntentError::LlmUnavailable(format!("LLM request failed: {}", e)))?;

        self.parse_llm_response(&content, agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StubProvider;

    fn make_router(response: &str) -> LlmRouter {
        let provider = Arc::new(StubProvider::new(response));
        LlmRouter::new(provider, vec![])
    }

    #[test]
    fn test_parse_llm_response_valid() {
        let router = make_router("");
        let json = r#"{"tool": "cas.search", "params": {"query": "test"}, "confidence": 0.9, "explanation": "searching"}"#;
        let result = router.parse_llm_response(json, "agent1");
        assert!(result.is_ok());
        let intents = result.unwrap();
        assert_eq!(intents.len(), 1);
        assert!(intents[0].confidence > 0.8);
    }

    #[test]
    fn test_parse_llm_response_none() {
        let router = make_router("");
        let json = r#"{"tool": "none", "confidence": 0.0, "explanation": "cannot resolve"}"#;
        let result = router.parse_llm_response(json, "agent1");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_llm_response_with_markdown_wrapper() {
        let router = make_router("");
        let json = "```json\n{\"tool\": \"memory.store\", \"params\": {\"content\": \"hello\"}, \"confidence\": 0.85, \"explanation\": \"storing\"}\n```";
        let result = router.parse_llm_response(json, "agent1");
        assert!(result.is_ok());
    }

    #[test]
    fn test_system_prompt_contains_tools() {
        let tools = vec![
            ToolDescriptor {
                name: "cas.search".into(),
                description: "Search objects".into(),
                schema: serde_json::Value::Null,
            },
        ];
        let provider = Arc::new(StubProvider::new(""));
        let router = LlmRouter::new(provider, tools);
        let prompt = router.build_system_prompt();
        assert!(prompt.contains("cas.search"));
    }

    #[test]
    fn test_resolve_delegates_to_provider() {
        let response = r#"{"tool": "cas.search", "params": {"query": "hello"}, "confidence": 0.95, "explanation": "search"}"#;
        let router = make_router(response);
        let result = router.resolve("find hello", "agent1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap()[0].confidence, 0.95);
    }
}
