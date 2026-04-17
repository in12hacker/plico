//! Embedding Service
//!
//! Re-exports all embedding backends.

pub mod json_rpc;
pub mod local;
pub mod ollama;
pub mod stub;
pub mod types;

pub use ollama::OllamaBackend;
pub use local::LocalEmbeddingBackend;
pub use stub::StubEmbeddingProvider;
pub use types::{EmbeddingProvider, EmbedError, Embedding, EmbeddingMeta};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_backend_creation_without_server() {
        let backend = OllamaBackend::new("http://localhost:9999", "all-minilm-l6-v2");
        match backend {
            Ok(b) => assert_eq!(b.dimension(), 384),
            Err(e) => {
                assert!(format!("{e}").contains("connection")
                    || format!("{e}").contains("9999")
                    || format!("{e}").contains("probe"));
            }
        }
    }
}
