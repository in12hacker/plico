//! Ollama daemon backend for text embeddings.

use std::sync::Arc;

use crate::fs::embedding::types::{EmbedError, EmbeddingProvider, EmbedResult};

/// Ollama daemon backend for text embeddings.
///
/// Spawns a dedicated tokio runtime in a background thread for async HTTP calls.
/// Thread-safe: the runtime handle is shared via Arc.
pub struct OllamaBackend {
    /// Tokio runtime for making async HTTP calls.
    rt: Arc<tokio::runtime::Runtime>,
    client: reqwest::Client,
    url: String,
    model: String,
    dimension: usize,
}

impl OllamaBackend {
    /// Create a new Ollama backend.
    ///
    /// `url` — Ollama server URL (e.g. `"http://localhost:11434"`).
    /// `model` — Model name (e.g. `"all-minilm-l6-v2"` or `"nomic-embed-text"`).
    pub fn new(url: &str, model: &str) -> Result<Self, EmbedError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(EmbedError::Http)?;

        let dimension = rt.block_on(Self::probe(&client, url, model)).unwrap_or_else(|e| {
            tracing::warn!("Ollama probe failed: {e}. Using default dimension 384.");
            384
        });

        Ok(Self {
            rt: Arc::new(rt),
            client,
            url: url.to_string(),
            model: model.to_string(),
            dimension,
        })
    }

    async fn probe(client: &reqwest::Client, url: &str, model: &str) -> Result<usize, EmbedError> {
        let tags_url = format!("{}/api/tags", url.trim_end_matches('/'));
        let resp: serde_json::Value = client
            .get(&tags_url)
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable(url.to_string()))?
            .json()
            .await
            .map_err(EmbedError::Http)?;

        let models = resp
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if !models.iter().any(|m| m.starts_with(model)) {
            return Err(EmbedError::ModelNotFound(format!(
                "model '{}' not found. Available: {:?}",
                model, models
            )));
        }

        let dim = match model {
            m if m.contains("all-minilm") => 384,
            m if m.contains("nomic-embed") => 768,
            m if m.contains("e5") => 1024,
            m if m.contains("bge-large") => 1024,
            m if m.contains("bge-") => 768,
            _ => 384,
        };
        Ok(dim)
    }

    async fn embed_async(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        #[derive(serde::Serialize)]
        struct Request<'a> {
            model: &'a str,
            prompt: &'a str,
        }
        #[derive(serde::Deserialize)]
        struct Response {
            embedding: Vec<f32>,
        }

        let resp = self
            .client
            .post(format!("{}/api/embeddings", self.url.trim_end_matches('/')))
            .json(&Request {
                model: &self.model,
                prompt: text,
            })
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    EmbedError::ServerUnavailable(self.url.clone())
                } else {
                    EmbedError::Http(e)
                }
            })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(EmbedError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(EmbedError::ollama(format!("status={} body={}", status, body_str)));
        }

        let Response { embedding } = serde_json::from_slice(&body_bytes)
            .map_err(|e| EmbedError::ollama(format!("parse error: {e}")))?;
        // Ollama doesn't return token counts — estimate based on character count
        let estimated_tokens = (text.len() / 4).max(1) as u32;
        Ok(EmbedResult::new(embedding, estimated_tokens))
    }

    /// Send a chat request to Ollama with JSON structured output mode.
    ///
    /// `model_override` — use a different model for chat (e.g. "llama3.2" for reasoning).
    /// If None, uses the same model as embeddings.
    pub async fn chat_async(
        &self,
        prompt: &str,
        system: Option<&str>,
        model_override: Option<&str>,
    ) -> Result<String, EmbedError> {
        #[derive(serde::Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: Vec<ChatMessage<'a>>,
            format: &'a str,
            stream: bool,
            options: serde_json::Value,
        }

        #[derive(serde::Serialize)]
        struct ChatMessage<'a> {
            role: &'a str,
            content: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct ChatResponse {
            message: ChatMessageOut,
        }

        #[derive(serde::Deserialize)]
        struct ChatMessageOut {
            content: String,
        }

        let model = model_override.unwrap_or(&self.model);

        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(ChatMessage { role: "system", content: sys });
        }
        messages.push(ChatMessage { role: "user", content: prompt });

        let req = ChatRequest {
            model,
            messages,
            format: "json",
            stream: false,
            options: serde_json::json!({
                "temperature": 0.1,
                "num_predict": 512
            }),
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.url.trim_end_matches('/')))
            .json(&req)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    EmbedError::ServerUnavailable(self.url.clone())
                } else {
                    EmbedError::Http(e)
                }
            })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(EmbedError::Http)?;

        if !status.is_success() {
            return Err(EmbedError::Ollama(format!(
                "chat API returned {}: {}",
                status,
                String::from_utf8_lossy(&body_bytes)
            )));
        }

        let parsed: ChatResponse = serde_json::from_slice(&body_bytes)
            .map_err(|e| EmbedError::Ollama(format!("failed to parse chat response: {e}")))?;

        Ok(parsed.message.content)
    }

    /// Synchronous wrapper for `chat_async`.
    pub fn chat(
        &self,
        prompt: &str,
        system: Option<&str>,
        model_override: Option<&str>,
    ) -> Result<String, EmbedError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| {
                    handle.block_on(self.chat_async(prompt, system, model_override))
                })
            }
            Err(_) => self.rt.block_on(self.chat_async(prompt, system, model_override)),
        }
    }
}

impl EmbeddingProvider for OllamaBackend {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.embed_async(text)))
            }
            Err(_) => self.rt.block_on(self.embed_async(text)),
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let this = self.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let fut = async move {
            let mut results = Vec::with_capacity(texts.len());
            for text in &texts {
                results.push(this.embed_async(text).await?);
            }
            Ok(results)
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => self.rt.block_on(fut),
        }
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OllamaBackend {
    fn clone(&self) -> Self {
        Self {
            rt: Arc::clone(&self.rt),
            client: self.client.clone(),
            url: self.url.clone(),
            model: self.model.clone(),
            dimension: self.dimension,
        }
    }
}
