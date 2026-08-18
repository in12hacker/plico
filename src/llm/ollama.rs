//! Ollama LLM provider — calls local Ollama daemon via `/api/chat`.

use std::sync::Arc;
use tokio::runtime::Runtime;

use super::{ChatMessage, ChatOptions, LlmError, LlmProvider};

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
            Err(_) => Some(Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?,
            )),
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
            .map_err(|_| LlmError::Unavailable("provider request failed".into()))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(LlmError::Api(format!(
                "provider returned HTTP status {}",
                status.as_u16()
            )));
        }
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|_| LlmError::Api("provider response read failed".into()))?;

        #[derive(serde::Deserialize)]
        struct ChatResponse {
            message: MessageContent,
            prompt_eval_count: Option<u32>,
            eval_count: Option<u32>,
        }
        #[derive(serde::Deserialize)]
        struct MessageContent {
            content: String,
        }

        let parsed: ChatResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| LlmError::Parse(format!("response parse error: {e}")))?;

        let content = parsed.message.content.trim().to_string();
        let (input_tokens, output_tokens) =
            usage_or_estimate(parsed.prompt_eval_count, parsed.eval_count, messages, &content);

        Ok((content, input_tokens, output_tokens))
    }
}

/// Real token usage from the provider response; older Ollama servers omit
/// the usage fields, so the ~4-chars-per-token estimate remains the
/// documented fallback (wheels audit W-05; no tokenizer dependency).
fn usage_or_estimate(
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
    messages: &[ChatMessage],
    content: &str,
) -> (u32, u32) {
    let input_tokens =
        prompt_eval_count.unwrap_or_else(|| messages.iter().map(|m| m.content.len() as u32 / 4).sum::<u32>().max(1));
    let output_tokens = eval_count.unwrap_or_else(|| (content.len() as u32 / 4).max(1));
    (input_tokens, output_tokens)
}

impl LlmProvider for OllamaProvider {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<(String, u32, u32), LlmError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(self.chat_async(messages, options))),
            Err(_) => self
                .rt
                .as_ref()
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
        let opts = ChatOptions {
            temperature: 0.0,
            max_tokens: None,
        };
        let result = provider.chat(&msgs, &opts);
        assert!(result.is_err());
    }

    #[test]
    fn test_ollama_usage_fields_win_over_estimate() {
        let messages = vec![ChatMessage::user("a long prompt"), ChatMessage::user("and a reply")];
        let (input_tokens, output_tokens) = usage_or_estimate(Some(4321), Some(876), &messages, "short");
        assert_eq!((input_tokens, output_tokens), (4321, 876));
    }

    #[test]
    fn test_ollama_usage_fallback_estimates_chars_quarters() {
        // Older servers omit prompt_eval_count/eval_count entirely: the
        // ~4-chars-per-token estimate is the documented fallback.
        let messages = vec![ChatMessage::user("twelve chars"), ChatMessage::user("four")];
        let (input_tokens, output_tokens) = usage_or_estimate(None, None, &messages, "sixteen characters"); // 18 bytes
        assert_eq!(input_tokens, (12 / 4) + (4 / 4));
        assert_eq!(output_tokens, 18 / 4);
    }

    #[test]
    fn test_ollama_usage_fallback_never_returns_zero() {
        let messages = vec![ChatMessage::user("x")];
        let (input_tokens, output_tokens) = usage_or_estimate(None, None, &messages, "y");
        assert_eq!((input_tokens, output_tokens), (1, 1));
    }
}
