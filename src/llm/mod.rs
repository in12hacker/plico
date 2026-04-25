//! LLM Provider Abstraction — model-agnostic chat/completion interface.
//!
//! Follows the same pattern as `fs::embedding::EmbeddingProvider`:
//! trait + multiple backends + env-driven selection.
//!
//! # Backends
//!
//! - `OllamaProvider` — calls local Ollama daemon (`/api/chat`)
//! - `StubProvider` — returns fixed responses for testing

pub mod circuit_breaker;
pub mod ollama;
pub mod openai;
pub mod stub;

pub use circuit_breaker::CircuitBreakerLlmProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAICompatibleProvider;
pub use stub::StubProvider;

use serde::{Deserialize, Serialize};

/// A chat message in the system/user/assistant role format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: "user".into(), content: content.into() }
    }
}

/// Options for a chat completion request.
#[derive(Debug, Clone)]
pub struct ChatOptions {
    pub temperature: f32,
    pub max_tokens: Option<usize>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self { temperature: 0.7, max_tokens: None }
    }
}

/// Errors from LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("LLM API error: {0}")]
    Api(String),

    #[error("LLM unavailable: {0}")]
    Unavailable(String),

    #[error("Response parse error: {0}")]
    Parse(String),

    #[error("Runtime error: {0}")]
    Runtime(#[from] std::io::Error),
}

/// Thread-safe provider for LLM chat/completion.
///
/// All model access in the kernel goes through this trait,
/// ensuring the system does not depend on any specific AI provider.
pub trait LlmProvider: Send + Sync {
    /// Send a chat completion request and return the assistant's response.
    ///
    /// Returns `(response, input_tokens, output_tokens)` on success.
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<(String, u32, u32), LlmError>;

    /// Name of the model used.
    fn model_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chat_message_constructors() {
        let sys = ChatMessage::system("You are helpful.");
        assert_eq!(sys.role, "system");
        let usr = ChatMessage::user("Hello");
        assert_eq!(usr.role, "user");
    }

    #[test]
    fn test_stub_provider_chat() {
        let provider = StubProvider::new("test response");
        let result = provider.chat(
            &[ChatMessage::user("anything")],
            &ChatOptions::default(),
        );
        assert!(result.is_ok());
        let (response, input_tokens, output_tokens) = result.unwrap();
        assert_eq!(response, "test response");
        assert_eq!(input_tokens, 0);
        assert_eq!(output_tokens, 0);
    }

    #[test]
    fn test_stub_provider_model_name() {
        let provider = StubProvider::new("x");
        assert_eq!(provider.model_name(), "stub");
    }
}
