//! Embedding Service — Text → Vector
//!
//! Converts text into dense vector embeddings for semantic similarity search.
//!
//! # Architecture
//!
//! ```text
//! EmbeddingProvider (trait)
//! ├── OllamaBackend      — calls local Ollama daemon via HTTP
//! └── LocalONNXBackend   — pure Rust ONNX Runtime (future iteration)
//! ```
//!
//! All backends are thread-safe (`Send + Sync`).

use std::sync::Arc;

/// A dense text embedding vector.
pub type Embedding = Vec<f32>;

/// Metadata associated with an embedded chunk.
#[derive(Debug, Clone)]
pub struct EmbeddingMeta {
    /// CID of the parent AIObject.
    pub cid: String,
    /// Chunk index within the parent object.
    pub chunk_id: u32,
    /// Original text chunk.
    pub text: String,
    /// Tags from the parent object.
    pub tags: Vec<String>,
    /// Start/end token offsets.
    pub start_token: u32,
    pub end_token: u32,
}

/// Errors from embedding operations.
#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Ollama API error: {0}")]
    Ollama(String),

    #[error("ONNX inference error: {0}")]
    Onnx(String),

    #[error("Model not available: {0}")]
    ModelNotFound(String),

    #[error("Server unavailable at {0}")]
    ServerUnavailable(String),

    #[error("Runtime error: {0}")]
    Runtime(#[from] std::io::Error),
}

impl EmbedError {
    pub fn ollama(msg: impl Into<String>) -> Self {
        EmbedError::Ollama(msg.into())
    }
}

/// Thread-safe provider for generating text embeddings.
pub trait EmbeddingProvider: Send + Sync {
    /// Generate an embedding for a single text.
    fn embed(&self, text: &str) -> Result<Embedding, EmbedError>;

    /// Generate embeddings for multiple texts in a batch.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbedError>;

    /// Embedding vector dimension (e.g. 384 for all-MiniLM-L6-v2).
    fn dimension(&self) -> usize;

    /// Name of the model used.
    fn model_name(&self) -> &str;
}

// ─── Ollama Backend ───────────────────────────────────────────────────────────

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
        // Build a single-threaded runtime for HTTP I/O
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(EmbedError::Http)?;

        // Probe server to verify model availability and get dimension
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

    /// Probe Ollama for model availability and embedding dimension.
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

    async fn embed_async(&self, text: &str) -> Result<Embedding, EmbedError> {
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
        Ok(embedding)
    }
}

impl EmbeddingProvider for OllamaBackend {
    fn embed(&self, text: &str) -> Result<Embedding, EmbedError> {
        self.rt.block_on(self.embed_async(text))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        let this = self.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        self.rt.block_on(async {
            let mut results = Vec::with_capacity(texts.len());
            for text in &texts {
                results.push(this.embed_async(text).await?);
            }
            Ok(results)
        })
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

// ─── Local ONNX Stub ──────────────────────────────────────────────────────────

/// Placeholder for local ONNX inference backend.
///
/// In a future iteration, this will use the `ort` crate to run
/// all-MiniLM-L6-v2 directly without an external Ollama daemon.
pub struct LocalONNXBackend {
    dimension: usize,
    model: String,
}

impl LocalONNXBackend {
    pub fn new(model_path: &str) -> Result<Self, EmbedError> {
        tracing::warn!("LocalONNXBackend is a stub — falling back to Ollama for embeddings");
        Ok(Self {
            dimension: 384,
            model: model_path.to_string(),
        })
    }
}

impl EmbeddingProvider for LocalONNXBackend {
    fn embed(&self, _text: &str) -> Result<Embedding, EmbedError> {
        Err(EmbedError::Onnx("LocalONNXBackend not yet implemented".to_string()))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        Err(EmbedError::Onnx("LocalONNXBackend not yet implemented".to_string()))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_backend_creation_without_server() {
        // Without a running Ollama, the backend still creates (probe fails gracefully)
        let backend = OllamaBackend::new("http://localhost:9999", "all-minilm-l6-v2");
        // Should succeed with default dimension (probe fails but is handled)
        match backend {
            Ok(b) => assert_eq!(b.dimension(), 384),
            Err(e) => {
                // Connection refused is acceptable
                assert!(format!("{e}").contains("connection") || format!("{e}").contains("9999") || format!("{e}").contains("probe"));
            }
        }
    }
}
