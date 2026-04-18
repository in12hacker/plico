//! Stub LLM provider — returns fixed responses for testing.

use super::{LlmProvider, ChatMessage, ChatOptions, LlmError};

pub struct StubProvider {
    response: String,
}

impl StubProvider {
    pub fn new(response: impl Into<String>) -> Self {
        Self { response: response.into() }
    }

    pub fn empty() -> Self {
        Self { response: String::new() }
    }
}

impl LlmProvider for StubProvider {
    fn chat(&self, _messages: &[ChatMessage], _options: &ChatOptions) -> Result<String, LlmError> {
        Ok(self.response.clone())
    }

    fn model_name(&self) -> &str {
        "stub"
    }
}
