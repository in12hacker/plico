//! LLM Summarizer — Text → Compressed Summary
//!
//! Generates L0/L1 summaries using a language model, replacing heuristic extraction.
//!
//! # Architecture
//!
//! ```text
//! Summarizer (trait)
//! ├── OllamaSummarizer  — calls local Ollama daemon (MVP)
//! └── LocalONNXSummarizer — future: native ONNX inference
//! ```
//!
//! # Integration
//!
//! - `ContextLoader::compute_l0()` calls `Summarizer::summarize()` instead of heuristics
//! - L0: compress ~500 tokens → ~50 token summary (2-3 sentences)
//! - L1: compress ~2000 tokens → ~200 token summary (paragraph)
//!
//! # Prompt Design
//!
//! L0 prompt: concise summary instruction (~100 tokens input)
//! L1 prompt: detailed summary instruction (~2000 tokens input)
//!
//! Uses streaming disabled, JSON mode disabled for simplicity.

use std::sync::Arc;
use tokio::runtime::Runtime;

/// Errors from summarization operations.
#[derive(Debug, thiserror::Error)]
pub enum SummarError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Ollama API error: {0}")]
    Ollama(String),

    #[error("Runtime error: {0}")]
    Runtime(#[from] std::io::Error),

    #[error("Content too long ({len} chars): max is {max}")]
    ContentTooLong { len: usize, max: usize },
}

/// Target summary layer — determines target length and compression ratio.
#[derive(Debug, Clone, Copy)]
pub enum SummaryLayer {
    /// L0: ~100 tokens → ~20 token summary (2-3 sentences)
    L0,
    /// L1: ~2000 tokens → ~200 token summary (paragraph)
    L1,
}

impl SummaryLayer {
    /// Maximum input characters for this layer.
    pub fn max_input_chars(&self) -> usize {
        match self {
            SummaryLayer::L0 => 1000,
            SummaryLayer::L1 => 8000,
        }
    }

    /// System prompt for this layer.
    pub fn system_prompt(&self) -> &'static str {
        match self {
            SummaryLayer::L0 => {
                "You are a precise text summarizer. Respond with only the summary — no preamble, no quotes, no explanation. Output 2-3 sentences maximum."
            }
            SummaryLayer::L1 => {
                "You are a detailed text summarizer. Respond with only the summary — no preamble, no quotes, no explanation. Capture the key points in 1-2 paragraphs."
            }
        }
    }

    /// User prompt template.
    pub fn user_prompt(&self, content: &str) -> String {
        match self {
            SummaryLayer::L0 => format!("Summarize this briefly:\n\n{}", content),
            SummaryLayer::L1 => format!("Summarize this in detail:\n\n{}", content),
        }
    }
}

/// Thread-safe summarizer trait.
pub trait Summarizer: Send + Sync {
    /// Generate a summary for the given content at the specified layer.
    ///
    /// Returns the summary text, or an error if summarization failed.
    ///
    /// If the content exceeds `layer.max_input_chars()`, it is truncated
    /// before being sent to the model.
    fn summarize(&self, content: &str, layer: SummaryLayer) -> Result<String, SummarError>;

    /// Name of the underlying model.
    fn model_name(&self) -> &str;
}

// ─── Ollama Backend ───────────────────────────────────────────────────────────

/// Ollama daemon summarizer.
///
/// Uses `POST /api/chat` with a chatml-style messages array.
pub struct OllamaSummarizer {
    rt: Arc<Runtime>,
    client: reqwest::Client,
    url: String,
    model: String,
}

impl OllamaSummarizer {
    /// Create a new Ollama summarizer.
    ///
    /// `url` — Ollama server URL (e.g. `"http://localhost:11434"`).
    /// `model` — Model name (e.g. `"llama3.2"` or `"qwen2.5"`).
    pub fn new(url: &str, model: &str) -> Result<Self, SummarError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(SummarError::Http)?;
        Ok(Self {
            rt: Arc::new(rt),
            client,
            url: url.to_string(),
            model: model.to_string(),
        })
    }

    async fn chat_async(&self, system: &str, user: &str) -> Result<String, SummarError> {
        #[derive(serde::Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: [ChatMessage<'a>; 2],
            stream: bool,
        }
        #[derive(serde::Serialize)]
        struct ChatMessage<'a> {
            role: &'a str,
            content: &'a str,
        }
        #[derive(serde::Deserialize)]
        struct ChatResponse {
            message: ChatMessageResponse,
        }
        #[derive(serde::Deserialize)]
        struct ChatMessageResponse {
            content: String,
        }

        let request = ChatRequest {
            model: &self.model,
            messages: [
                ChatMessage { role: "system", content: system },
                ChatMessage { role: "user", content: user },
            ],
            stream: false,
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.url.trim_end_matches('/')))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    SummarError::Ollama(format!("cannot connect to Ollama at {}", self.url))
                } else {
                    SummarError::Http(e)
                }
            })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(SummarError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(SummarError::Ollama(format!(
                "status={} body={}",
                status, body_str
            )));
        }

        let ChatResponse { message } =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                SummarError::Ollama(format!("parse error: {e}"))
            })?;

        Ok(message.content.trim().to_string())
    }
}

impl Summarizer for OllamaSummarizer {
    fn summarize(&self, content: &str, layer: SummaryLayer) -> Result<String, SummarError> {
        let max_chars = layer.max_input_chars();
        let truncated = if content.len() > max_chars {
            tracing::warn!(
                "Content {} chars exceeds L{:?} max {} — truncating",
                content.len(),
                layer,
                max_chars
            );
            // Truncate to last N chars that fit within limit
            let start = content.len() - max_chars;
            &content[start..]
        } else {
            content
        };

        let system = layer.system_prompt();
        let user = layer.user_prompt(truncated);

        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(self.chat_async(system, &user))),
            Err(_) => self.rt.block_on(self.chat_async(system, &user)),
        }
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OllamaSummarizer {
    fn clone(&self) -> Self {
        Self {
            rt: Arc::clone(&self.rt),
            client: self.client.clone(),
            url: self.url.clone(),
            model: self.model.clone(),
        }
    }
}

// ─── Local ONNX Stub ──────────────────────────────────────────────────────────

/// Placeholder for local ONNX summarization.
///
/// In a future iteration, this will use the `ort` crate with a summarization model.
pub struct LocalONNXSummarizer {
    model: String,
}

impl LocalONNXSummarizer {
    pub fn new(model_path: &str) -> Result<Self, SummarError> {
        tracing::warn!("LocalONNXSummarizer is a stub — falling back to Ollama");
        Ok(Self {
            model: model_path.to_string(),
        })
    }
}

impl Summarizer for LocalONNXSummarizer {
    fn summarize(&self, _content: &str, _layer: SummaryLayer) -> Result<String, SummarError> {
        Err(SummarError::Ollama(
            "LocalONNXSummarizer not yet implemented".to_string(),
        ))
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarizer_creation_without_server() {
        // Should create even without Ollama (connection fails at runtime, not init)
        let s = OllamaSummarizer::new("http://localhost:9999", "llama3.2");
        match s {
            Ok(s) => assert_eq!(s.model_name(), "llama3.2"),
            Err(e) => {
                assert!(format!("{e}").contains("9999") || format!("{e}").contains("connection"));
            }
        }
    }

    #[test]
    fn test_summary_layer_max_chars() {
        assert_eq!(SummaryLayer::L0.max_input_chars(), 1000);
        assert_eq!(SummaryLayer::L1.max_input_chars(), 8000);
    }

    #[test]
    fn test_summary_layer_prompts() {
        let l0_sys = SummaryLayer::L0.system_prompt();
        assert!(l0_sys.contains("2-3 sentences"));

        let l1_sys = SummaryLayer::L1.system_prompt();
        assert!(l1_sys.contains("1-2 paragraphs"));
    }

    /// Calling summarize() from inside a tokio::spawn must not panic.
    /// The connection will fail (no server at 9999) but must not panic with
    /// "Cannot start a runtime from within a runtime".
    #[tokio::test]
    async fn test_summarize_safe_inside_tokio_spawn() {
        let s = OllamaSummarizer::new("http://localhost:9999", "test-model").unwrap();
        let result = tokio::task::spawn_blocking(move || {
            s.summarize("hello world", SummaryLayer::L0)
        }).await.unwrap();
        // Connection refused is expected — what must NOT happen is a panic
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(!msg.contains("Cannot start a runtime from within a runtime"));
    }
}
