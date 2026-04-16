//! LLM Intent Router — sends NL + tool catalog to an LLM for resolution.
//!
//! Optional — requires Ollama. Falls back gracefully when unavailable.
//! The LLM receives a system prompt with the full tool catalog and
//! returns structured JSON describing the intended actions.

use super::{IntentRouter, ResolvedIntent, IntentError};
use crate::api::semantic::ApiRequest;
use crate::tool::ToolDescriptor;

pub struct LlmRouter {
    ollama_url: String,
    model: String,
    tool_catalog: Vec<ToolDescriptor>,
}

impl LlmRouter {
    pub fn new(ollama_url: &str, model: &str, tool_catalog: Vec<ToolDescriptor>) -> Self {
        Self {
            ollama_url: ollama_url.to_string(),
            model: model.to_string(),
            tool_catalog,
        }
    }

    fn build_system_prompt(&self) -> String {
        let mut tools_desc = String::new();
        for tool in &self.tool_catalog {
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

        let action = ApiRequest::ToolCall {
            tool: tool_name.to_string(),
            params,
            agent_id: agent_id.to_string(),
        };

        Ok(vec![ResolvedIntent {
            confidence,
            action,
            explanation,
        }])
    }
}

impl IntentRouter for LlmRouter {
    fn resolve(&self, text: &str, agent_id: &str) -> Result<Vec<ResolvedIntent>, IntentError> {
        let system_prompt = self.build_system_prompt();

        let request_body = serde_json::json!({
            "model": self.model,
            "messages": [
                {"role": "system", "content": system_prompt},
                {"role": "user", "content": text}
            ],
            "stream": false,
            "options": {
                "temperature": 0.1
            }
        });

        let url = format!("{}/api/chat", self.ollama_url);

        let client = match reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
            Ok(c) => c,
            Err(e) => return Err(IntentError::LlmUnavailable(format!("HTTP client error: {}", e))),
        };

        let response = client.post(&url)
            .json(&request_body)
            .send()
            .map_err(|e| IntentError::LlmUnavailable(format!("Ollama request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(IntentError::LlmUnavailable(
                format!("Ollama returned status {}", response.status()),
            ));
        }

        let resp_json: serde_json::Value = response.json()
            .map_err(|e| IntentError::LlmUnavailable(format!("Response parse error: {}", e)))?;

        let content = resp_json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("");

        self.parse_llm_response(content, agent_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_llm_response_valid() {
        let router = LlmRouter::new("http://localhost:11434", "llama3.2", vec![]);
        let json = r#"{"tool": "cas.search", "params": {"query": "test"}, "confidence": 0.9, "explanation": "searching"}"#;
        let result = router.parse_llm_response(json, "agent1");
        assert!(result.is_ok());
        let intents = result.unwrap();
        assert_eq!(intents.len(), 1);
        assert!(intents[0].confidence > 0.8);
    }

    #[test]
    fn test_parse_llm_response_none() {
        let router = LlmRouter::new("http://localhost:11434", "llama3.2", vec![]);
        let json = r#"{"tool": "none", "confidence": 0.0, "explanation": "cannot resolve"}"#;
        let result = router.parse_llm_response(json, "agent1");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_llm_response_with_markdown_wrapper() {
        let router = LlmRouter::new("http://localhost:11434", "llama3.2", vec![]);
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
        let router = LlmRouter::new("http://localhost:11434", "llama3.2", tools);
        let prompt = router.build_system_prompt();
        assert!(prompt.contains("cas.search"));
    }
}
