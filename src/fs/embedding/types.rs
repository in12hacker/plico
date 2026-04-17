//! Embedding types and trait definitions.

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

    #[error("Python subprocess error: {0}")]
    Subprocess(String),

    #[error("Python subprocess not available. Install dependencies:\n  pip install transformers huggingface_hub onnxruntime")]
    SubprocessUnavailable,
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
