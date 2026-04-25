//! LLM Summarizer — Text → Compressed Summary
//!
//! Generates L0/L1 summaries using a language model, replacing heuristic extraction.
//!
//! # Architecture
//!
//! ```text
//! Summarizer (trait)
//! └── LlmSummarizer — delegates to any LlmProvider (model-agnostic)
//! ```
//!
//! # Integration
//!
//! - `ContextLoader::compute_l0()` calls `Summarizer::summarize()` instead of heuristics
//! - L0: compress ~500 tokens → ~50 token summary (2-3 sentences)
//! - L1: compress ~2000 tokens → ~200 token summary (paragraph)

use std::sync::Arc;
use crate::llm::{LlmProvider, ChatMessage, ChatOptions};

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

    #[error("LLM error: {0}")]
    Llm(String),
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
    fn summarize(&self, content: &str, layer: SummaryLayer) -> Result<String, SummarError>;
    fn model_name(&self) -> &str;
}

// ─── LLM-backed Summarizer (model-agnostic) ─────────────────────────────────

/// Summarizer that delegates to any `LlmProvider`.
pub struct LlmSummarizer {
    provider: Arc<dyn LlmProvider>,
}

impl LlmSummarizer {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }
}

impl Summarizer for LlmSummarizer {
    fn summarize(&self, content: &str, layer: SummaryLayer) -> Result<String, SummarError> {
        let max_chars = layer.max_input_chars();
        let truncated = if content.len() > max_chars {
            tracing::warn!(
                "Content {} chars exceeds L{:?} max {} — truncating",
                content.len(),
                layer,
                max_chars
            );
            let start = content.len() - max_chars;
            &content[start..]
        } else {
            content
        };

        let messages = vec![
            ChatMessage::system(layer.system_prompt()),
            ChatMessage::user(layer.user_prompt(truncated)),
        ];
        let options = ChatOptions { temperature: 0.3, max_tokens: None };

        let (summary, _input_tokens, _output_tokens) = self.provider
            .chat(&messages, &options)
            .map_err(|e| SummarError::Llm(e.to_string()))?;
        Ok(summary)
    }

    fn model_name(&self) -> &str {
        self.provider.model_name()
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StubProvider;

    #[test]
    fn test_llm_summarizer_delegates_to_provider() {
        let provider = Arc::new(StubProvider::new("This is a summary."));
        let summarizer = LlmSummarizer::new(provider);
        let result = summarizer.summarize("Some long content here", SummaryLayer::L0);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "This is a summary.");
    }

    #[test]
    fn test_llm_summarizer_model_name() {
        let provider = Arc::new(StubProvider::new("x"));
        let summarizer = LlmSummarizer::new(provider);
        assert_eq!(summarizer.model_name(), "stub");
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
}
