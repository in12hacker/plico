//! OpenAI-compatible LLM provider — calls any `/v1/chat/completions` endpoint.
//!
//! Covers: OpenAI, DeepSeek, Groq, Together, Mistral, vLLM, SGLang,
//! TensorRT-LLM, llama.cpp server, Ollama (/v1 endpoint), and any future
//! service exposing the OpenAI chat completions format.

use std::sync::Arc;
use tokio::runtime::Runtime;

use super::{LlmProvider, ChatMessage, ChatOptions, LlmError};

pub struct OpenAICompatibleProvider {
    /// Only created when no Tokio runtime is active (standalone/CLI mode).
    /// In daemon mode, `chat()` uses `block_in_place` on the existing runtime.
    rt: Option<Arc<Runtime>>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl OpenAICompatibleProvider {
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, LlmError> {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(_) => None,
            Err(_) => {
                Some(Arc::new(tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?))
            }
        };
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(LlmError::Http)?;
        Ok(Self {
            rt,
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
    ) -> Result<(String, u32, u32), LlmError> {
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

pub(crate) fn parse_response(body: &[u8]) -> Result<(String, u32, u32), LlmError> {
    #[derive(serde::Deserialize)]
    struct ChatCompletionResponse {
        choices: Vec<Choice>,
        usage: Option<Usage>,
    }
    #[derive(serde::Deserialize)]
    struct Choice {
        message: ChoiceMessage,
    }
    #[derive(serde::Deserialize)]
    struct ChoiceMessage {
        #[serde(default)]
        content: Option<String>,
        #[serde(default)]
        reasoning_content: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    }

    let parsed: ChatCompletionResponse = serde_json::from_slice(body)
        .map_err(|e| LlmError::Parse(format!("response parse error: {e}")))?;

    let msg = parsed
        .choices
        .into_iter()
        .next()
        .map(|c| c.message)
        .ok_or_else(|| LlmError::Parse("empty choices array".into()))?;

    let content = msg.content.as_deref().unwrap_or("").trim();
    let text = if content.is_empty() {
        msg.reasoning_content.as_deref().unwrap_or("").trim().to_string()
    } else {
        content.to_string()
    };

    let input_tokens = parsed.usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0);
    let output_tokens = parsed.usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0);

    Ok((text, input_tokens, output_tokens))
}

impl LlmProvider for OpenAICompatibleProvider {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| {
                    handle.block_on(self.chat_async(messages, options))
                })
            }
            Err(_) => self.rt.as_ref()
                .expect("rt must exist when no Tokio runtime is active")
                .block_on(self.chat_async(messages, options)),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OpenAICompatibleProvider {
    fn clone(&self) -> Self {
        Self {
            rt: self.rt.as_ref().map(Arc::clone),
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
        let json = br#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":" hello world "}}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let result = parse_response(json);
        assert!(result.is_ok());
        let (content, in_tok, out_tok) = result.unwrap();
        assert_eq!(content, "hello world");
        assert_eq!(in_tok, 10);
        assert_eq!(out_tok, 5);
    }

    #[test]
    fn test_parse_response_reasoning_content_fallback() {
        let json = br#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"","reasoning_content":"The answer is Four."}}],"usage":{"prompt_tokens":28,"completion_tokens":50}}"#;
        let result = parse_response(json);
        assert!(result.is_ok());
        let (content, _, _) = result.unwrap();
        assert_eq!(content, "The answer is Four.");
    }

    #[test]
    fn test_parse_response_null_content_with_reasoning() {
        let json = br#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","reasoning_content":"Thinking..."}}],"usage":{"prompt_tokens":5,"completion_tokens":3}}"#;
        let result = parse_response(json);
        assert!(result.is_ok());
        let (content, _, _) = result.unwrap();
        assert_eq!(content, "Thinking...");
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

    fn llama_server_url() -> String {
        std::env::var("LLAMA_TEST_URL").unwrap_or_else(|_| "http://127.0.0.1:18920/v1".to_string())
    }
    fn llama_model() -> String {
        std::env::var("LLAMA_TEST_MODEL").unwrap_or_else(|_| "qwen2.5-0.5b-instruct-q4_k_m.gguf".to_string())
    }

    macro_rules! skip_if_unavailable {
        ($result:expr) => {
            match $result {
                Err(ref e) if e.to_string().contains("Unavailable") || e.to_string().contains("connect") => {
                    eprintln!("llama-server not reachable, skipping: {e}");
                    return;
                }
                other => other,
            }
        };
    }

    #[test]
    fn test_openai兼容_provider_llama_server_simple() {
        let provider = OpenAICompatibleProvider::new(&llama_server_url(), &llama_model(), None)
            .expect("provider should be constructible");
        let msgs = vec![ChatMessage::user("Say hello in 3 words")];
        let opts = ChatOptions { temperature: 0.7, max_tokens: Some(20) };
        let result = skip_if_unavailable!(provider.chat(&msgs, &opts));
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let (reply, input_tokens, output_tokens) = result.unwrap();
        assert!(!reply.is_empty(), "reply should not be empty, got: {:?}", reply);
        println!("[llama-server] simple reply: {:?}, tokens: in={} out={}", reply, input_tokens, output_tokens);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_system_prompt() {
        let provider = OpenAICompatibleProvider::new(&llama_server_url(), &llama_model(), None)
            .expect("provider should be constructible");
        let msgs = vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("What is 1+1?"),
        ];
        let opts = ChatOptions { temperature: 0.5, max_tokens: Some(20) };
        let result = skip_if_unavailable!(provider.chat(&msgs, &opts));
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let (reply, input_tokens, output_tokens) = result.unwrap();
        assert!(!reply.is_empty(), "reply should not be empty, got: {:?}", reply);
        println!("[llama-server] system-prompt reply: {:?}, tokens: in={} out={}", reply, input_tokens, output_tokens);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_multi_turn() {
        let provider = OpenAICompatibleProvider::new(&llama_server_url(), &llama_model(), None)
            .expect("provider should be constructible");
        let msgs = vec![
            ChatMessage::user("My favorite color is blue."),
            ChatMessage::user("What is my favorite color?"),
        ];
        let opts = ChatOptions { temperature: 0.0, max_tokens: Some(30) };
        let result = skip_if_unavailable!(provider.chat(&msgs, &opts));
        assert!(result.is_ok(), "chat should succeed: {:?}", result);
        let (reply, _, _) = result.unwrap();
        let reply = reply.to_lowercase();
        assert!(reply.contains("blue"), "reply should mention 'blue', got: {:?}", reply);
        println!("[llama-server] multi-turn reply: {:?}", reply);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_temperature_zero() {
        let provider = OpenAICompatibleProvider::new(&llama_server_url(), &llama_model(), None)
            .expect("provider should be constructible");
        let msgs = vec![ChatMessage::user("What is 1+1?")];
        let opts = ChatOptions { temperature: 0.0, max_tokens: Some(200) };
        let r1 = skip_if_unavailable!(provider.chat(&msgs, &opts));
        let r2 = skip_if_unavailable!(provider.chat(&msgs, &opts));
        assert!(r1.is_ok() && r2.is_ok());
        let (c1, _, _) = r1.unwrap();
        let (c2, _, _) = r2.unwrap();
        assert!(c1.contains('2') && c2.contains('2'),
            "both replies should contain '2', got r1={:?} r2={:?}", c1, c2);
        println!("[llama-server] temp=0 replies: r1={:?} r2={:?}", c1, c2);
    }

    #[test]
    fn test_openai兼容_provider_llama_server_model_name() {
        let provider = OpenAICompatibleProvider::new(&llama_server_url(), &llama_model(), None)
            .expect("provider should be constructible");
        assert_eq!(provider.model_name(), llama_model());
    }
}
