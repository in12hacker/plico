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

    #[error("API error: {0}")]
    Api(String),

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
    /// Generate an embedding for a single text (generic / unspecified usage).
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError>;

    /// Generate embeddings for multiple texts in a batch.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError>;

    /// Embed text intended as a **search query** (asymmetric retrieval).
    ///
    /// Models trained with task-specific prefixes (e.g. jina-v5 `"Query: "`,
    /// E5 `"query: "`, BGE `"Represent this sentence..."`) should override this.
    /// Default: delegates to [`embed`].
    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed(text)
    }

    /// Embed text intended as a **stored document** (asymmetric retrieval).
    ///
    /// Default: delegates to [`embed`].
    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed(text)
    }

    /// Output embedding dimension after any post-processing (e.g. Matryoshka truncation).
    fn dimension(&self) -> usize;

    /// Raw embedding dimension from the underlying model (before truncation).
    /// Default: same as [`dimension`].
    fn raw_dimension(&self) -> usize {
        self.dimension()
    }

    /// Name of the model used.
    fn model_name(&self) -> &str;
}

/// Result of an embedding operation, including token usage.
#[derive(Debug, Clone)]
pub struct EmbedResult {
    pub embedding: Embedding,
    pub input_tokens: u32,
}

/// Result of a batch embedding operation.
impl EmbedResult {
    pub fn new(embedding: Embedding, input_tokens: u32) -> Self {
        Self { embedding, input_tokens }
    }
}
