//! OpenAI-compatible LLM provider — calls any `/v1/chat/completions` endpoint.
//!
//! Covers: OpenAI, DeepSeek, Groq, Together, Mistral, vLLM, SGLang,
//! TensorRT-LLM, llama.cpp server, Ollama (/v1 endpoint), and any future
//! service exposing the OpenAI chat completions format.

use std::sync::Arc;
use tokio::runtime::Runtime;

use super::{LlmProvider, ChatMessage, ChatOptions, LlmError};

pub struct OpenAICompatibleProvider {
    rt: Arc<Runtime>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAICompatibleProvider {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, LlmError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(LlmError::Http)?;
        Ok(Self {
            rt: Arc::new(rt),
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            api_key,
        })
    }

    pub(crate) fn build_request_body(
        &self,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> serde_json::Value {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": api_messages,
            "temperature": options.temperature
        });

        if let Some(max_tokens) = options.max_tokens {
            body["max_tokens"] = serde_json::json!(max_tokens);
        }

        body
    }

    async fn chat_async(
        &self,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<String, LlmError> {
        let body = self.build_request_body(messages, options);

        let mut req = self
            .client
            .post(format!("{}/chat/completions", self.base_url))
            .json(&body);

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_connect() {
                LlmError::Unavailable(format!("cannot connect to {}", self.base_url))
            } else {
                LlmError::Http(e)
            }
        })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(LlmError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            if status == reqwest::StatusCode::UNAUTHORIZED {
                return Err(LlmError::Api(format!(
                    "401 Unauthorized (is OPENAI_API_KEY set?) body={body_str}"
                )));
            }
            return Err(LlmError::Api(format!("status={status} body={body_str}")));
        }

        parse_response(&body_bytes)
    }
}

pub(crate) fn parse_response(body: &[u8]) -> Result<String, LlmError> {
    #[derive(serde::Deserialize)]
    struct ChatCompletionResponse {
        choices: Vec<Choice>,
    }
    #[derive(serde::Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(serde::Deserialize)]
    struct ChoiceMessage {
        content: String,
    }

    let parsed: ChatCompletionResponse = serde_json::from_slice(body)
        .map_err(|e| LlmError::Parse(format!("response parse error: {e}")))?;

    parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content.trim().to_string())
        .ok_or_else(|| LlmError::Parse("empty choices array".into()))
}

impl LlmProvider for OpenAICompatibleProvider {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<String, LlmError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.chat_async(messages, options)))
            }
            Err(_) => self.rt.block_on(self.chat_async(messages, options)),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OpenAICompatibleProvider {
    fn clone(&self) -> Self {
        Self {
            rt: Arc::clone(&self.rt),
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_response_valid() {
        let json = br#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":" hello world "}}]}"#;
        let result = parse_response(json);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "hello world");
    }

    #[test]
    fn test_parse_response_empty_choices() {
        let json = br#"{"id":"chatcmpl-1","choices":[]}"#;
        let result = parse_response(json);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("empty choices"));
    }

    #[test]
    fn test_parse_response_malformed() {
        let json = br#"{"not":"valid"}"#;
        let result = parse_response(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_body_with_max_tokens() {
        let provider = OpenAICompatibleProvider::new("http://localhost:8000/v1", "gpt-4", None).unwrap();
        let msgs = vec![ChatMessage::user("hi")];
        let opts = ChatOptions { temperature: 0.5, max_tokens: Some(512) };
        let body = provider.build_request_body(&msgs, &opts);
        assert_eq!(body["max_tokens"], 512);
        assert_eq!(body["model"], "gpt-4");
        assert_eq!(body["temperature"], 0.5);
    }

    #[test]
    fn test_build_body_without_max_tokens() {
        let provider = OpenAICompatibleProvider::new("http://localhost:8000/v1", "llama3.2", None).unwrap();
        let msgs = vec![ChatMessage::system("sys"), ChatMessage::user("hi")];
        let opts = ChatOptions { temperature: 0.7, max_tokens: None };
        let body = provider.build_request_body(&msgs, &opts);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_model_name() {
        let provider = OpenAICompatibleProvider::new("http://localhost:8000/v1", "deepseek-chat", None).unwrap();
        assert_eq!(provider.model_name(), "deepseek-chat");
    }

    // ─── Integration tests against local llama-server ─────────────────────────

    const LLAMA_SERVER_URL: &str = "http://127.0.0.1:18920/v1";
    const LLAMA_MODEL: &str = "qwen2.5-0.5b-instruct-q4_k_m.gguf";

    #[test]
    fn test_openai兼容_provider_llama_server_simple() {
        // Smoke test: send a single-user message and verify we get a non-empty reply.
        let provider = OpenAICompatibleProvider::new(LLAMA_SERVER_URL, LLAMA_MODEL, None)
            .expect("provider should be constructible");
        let msgs = vec![ChatMessage::user("Say hello in 3 words")];
        let opts = ChatOptions { temperature: 0.7, max_tokens: Some(20) };
        let result = provider.chat(&msgs, &opts);
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let reply = result.unwrap();
        assert!(!reply.is_empty(), "reply should not be empty, got: {:?}", reply);
        println!("[llama-server] simple reply: {:?}", reply);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_system_prompt() {
        // Verify system prompt is passed (model may not strictly obey, so just check
        // the reply is non-empty and the API call succeeds).
        let provider = OpenAICompatibleProvider::new(LLAMA_SERVER_URL, LLAMA_MODEL, None)
            .expect("provider should be constructible");
        let msgs = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("What is 1+1?"),
        ];
        let opts = ChatOptions { temperature: 0.5, max_tokens: Some(20) };
        let result = provider.chat(&msgs, &opts);
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let reply = result.unwrap();
        assert!(!reply.is_empty(), "reply should not be empty, got: {:?}", reply);
        println!("[llama-server] system-prompt reply: {:?}", reply);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_multi_turn() {
        // Two messages: model should consider context from first in second response.
        let provider = OpenAICompatibleProvider::new(LLAMA_SERVER_URL, LLAMA_MODEL, None)
            .expect("provider should be constructible");
        let msgs = vec![
            ChatMessage::user("My favorite color is blue."),
            ChatMessage::user("What is my favorite color?"),
        ];
        let opts = ChatOptions { temperature: 0.0, max_tokens: Some(30) };
        let result = provider.chat(&msgs, &opts);
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let reply = result.unwrap().to_lowercase();
        // 0.5B model may not have perfect context but should mention blue.
        assert!(reply.contains("blue"), "reply should mention 'blue', got: {:?}", reply);
        println!("[llama-server] multi-turn reply: {:?}", reply);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_temperature_zero() {
        // Temperature=0 should produce consistent output for same input.
        let provider = OpenAICompatibleProvider::new(LLAMA_SERVER_URL, LLAMA_MODEL, None)
            .expect("provider should be constructible");
        let msgs = vec![ChatMessage::user("What is 1+1?")];
        let opts = ChatOptions { temperature: 0.0, max_tokens: Some(10) };
        let r1 = provider.chat(&msgs, &opts);
        let r2 = provider.chat(&msgs, &opts);
        assert!(r1.is_ok() && r2.is_ok());
        let (c1, c2) = (r1.unwrap(), r2.unwrap());
        assert!(c1.contains('2') && c2.contains('2'),
            "both replies should contain '2', got r1={:?} r2={:?}", c1, c2);
        println!("[llama-server] temp=0 replies: r1={:?} r2={:?}", c1, c2);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_model_name() {
        // Verify model_name() returns the configured model.
        let provider = OpenAICompatibleProvider::new(LLAMA_SERVER_URL, LLAMA_MODEL, None)
            .expect("provider should be constructible");
        assert_eq!(provider.model_name(), LLAMA_MODEL);
    }
}
