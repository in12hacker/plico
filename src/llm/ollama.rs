//! Ollama LLM provider — calls local Ollama daemon via `/api/chat`.

use std::sync::Arc;
use tokio::runtime::Runtime;

use super::{LlmProvider, ChatMessage, ChatOptions, LlmError};

pub struct OllamaProvider {
    /// Only created when no Tokio runtime is active (standalone/CLI mode).
    rt: Option<Arc<Runtime>>,
    client: reqwest::Client,
    url: String,
    model: String,
}

impl OllamaProvider {
    pub fn new(url: &str, model: &str) -> Result<Self, LlmError> {
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
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(LlmError::Http)?;
        Ok(Self {
            rt,
            client,
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
        })
    }

    async fn chat_async(
        &self,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<(String, u32, u32), LlmError> {
        let api_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "messages": api_messages,
            "stream": false,
            "options": {
                "temperature": options.temperature
            }
        });

        if let Some(max_tokens) = options.max_tokens {
            body["options"]["num_predict"] = serde_json::json!(max_tokens);
        }

        let resp = self
            .client
            .post(format!("{}/api/chat", self.url))
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    LlmError::Unavailable(format!("cannot connect to Ollama at {}", self.url))
                } else {
                    LlmError::Http(e)
                }
            })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(LlmError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(LlmError::Api(format!("status={} body={}", status, body_str)));
        }

        #[derive(serde::Deserialize)]
        struct ChatResponse {
            message: MessageContent,
        }
        #[derive(serde::Deserialize)]
        struct MessageContent {
            content: String,
        }

        let parsed: ChatResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| LlmError::Parse(format!("response parse error: {e}")))?;

        let content = parsed.message.content.trim().to_string();
        let input_tokens = messages.iter().map(|m| m.content.len() as u32 / 4).sum::<u32>().max(1);
        let output_tokens = (content.len() as u32 / 4).max(1);

        Ok((content, input_tokens, output_tokens))
    }
}

impl LlmProvider for OllamaProvider {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.chat_async(messages, options)))
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

impl Clone for OllamaProvider {
    fn clone(&self) -> Self {
        Self {
            rt: self.rt.as_ref().map(Arc::clone),
            client: self.client.clone(),
            url: self.url.clone(),
            model: self.model.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_provider_new() {
        let provider = OllamaProvider::new("http://localhost:11434", "test-model");
        assert!(provider.is_ok());
        let p = provider.unwrap();
        assert_eq!(p.model_name(), "test-model");
        assert_eq!(p.url, "http://localhost:11434");
    }

    #[test]
    fn test_ollama_provider_trim_trailing_slash() {
        let provider = OllamaProvider::new("http://localhost:11434/", "model").unwrap();
        assert_eq!(provider.url, "http://localhost:11434");
    }

    #[test]
    fn test_ollama_provider_clone() {
        let provider = OllamaProvider::new("http://localhost:11434", "model").unwrap();
        let cloned = provider.clone();
        assert_eq!(cloned.model_name(), provider.model_name());
        assert_eq!(cloned.url, provider.url);
    }

    #[test]
    fn test_ollama_chat_unreachable() {
        let provider = OllamaProvider::new("http://127.0.0.1:1", "model").unwrap();
        let msgs = vec![ChatMessage::user("test")];
        let opts = ChatOptions { temperature: 0.0, max_tokens: None };
        let result = provider.chat(&msgs, &opts);
        assert!(result.is_err());
    }
}
