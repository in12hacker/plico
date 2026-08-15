//! Embedding types and trait definitions.

use std::num::NonZeroU32;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PROVIDER_COMPATIBILITY_DOMAIN: &[u8] = b"plico.embedding.provider-compatibility.v1\0";
const MAX_EMBEDDING_DIMENSION: u32 = 65_536;

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

    #[error("Input too large: {0}")]
    InputTooLarge(String),
}

/// A provider family whose immutable output contract can be verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderFamily {
    Ollama,
    #[cfg(test)]
    TestDeterministicV1,
}

impl EmbeddingProviderFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            #[cfg(test)]
            Self::TestDeterministicV1 => "test_deterministic_v1",
        }
    }
}

/// The embedding operation is part of every runtime cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingInputOperation {
    Generic,
    Query,
    Document,
}

/// The canonical source handed to a verified document provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingInputContract {
    MemoryTextUtf8V1,
}

/// Output transformation applied after the verified provider response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingNormalization {
    ProviderNative,
    L2AfterMatryoshkaTruncationV1,
}

/// Plico-owned, closed set of document/query input transformations that may
/// participate in a publishable builder identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EmbeddingTransformContract {
    ProviderNativeInputV1,
    Qwen3WebSearchV1,
}

impl EmbeddingTransformContract {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderNativeInputV1 => "provider-native-input-v1",
            Self::Qwen3WebSearchV1 => "qwen3-web-search-query-document-native-v1",
        }
    }
}

impl EmbeddingNormalization {
    pub const fn transform_contract_id(self) -> &'static str {
        match self {
            Self::ProviderNative => "provider-native-document-v1",
            Self::L2AfterMatryoshkaTruncationV1 => "plico-matryoshka-truncate-l2-v1",
        }
    }
}

/// Stable reason that a provider cannot prove an immutable document builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EmbeddingIdentityError {
    #[error("embedding builder identity unavailable: unpinned_remote_model")]
    UnpinnedRemoteModel,
    #[error("embedding builder identity unavailable: local_evidence_incomplete")]
    LocalEvidenceIncomplete,
    #[error("embedding builder identity unavailable: stub_provider")]
    StubProvider,
    #[error("embedding builder identity unavailable: provider_probe_failed")]
    ProviderProbeFailed,
    #[error("embedding builder identity unavailable: provider_changed")]
    ProviderChanged,
    #[error("embedding builder identity unavailable: invalid_identity_evidence")]
    InvalidIdentityEvidence,
    #[error("embedding builder identity unavailable: unregistered_input_contract")]
    UnregisteredInputContract,
}

impl EmbeddingIdentityError {
    pub const fn category(self) -> &'static str {
        match self {
            Self::UnpinnedRemoteModel => "unpinned_remote_model",
            Self::LocalEvidenceIncomplete => "local_evidence_incomplete",
            Self::StubProvider => "stub_provider",
            Self::ProviderProbeFailed => "provider_probe_failed",
            Self::ProviderChanged => "provider_changed",
            Self::InvalidIdentityEvidence => "invalid_identity_evidence",
            Self::UnregisteredInputContract => "unregistered_input_contract",
        }
    }
}

/// Verified, non-secret identity for one immutable `embed_document` contract.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct EmbeddingBuilderIdentity {
    provider_family: EmbeddingProviderFamily,
    provider_compatibility_id: String,
    model_id: String,
    raw_dimension: NonZeroU32,
    effective_dimension: NonZeroU32,
    input_contract: EmbeddingInputContract,
    normalization: EmbeddingNormalization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OllamaIdentityEvidence {
    pub schema: String,
    pub model_tag: String,
    pub model_digest: String,
    pub server_version: String,
    pub api_contract: String,
    pub raw_dimension: u32,
}

impl EmbeddingBuilderIdentity {
    fn from_verified_evidence<T: Serialize>(
        provider_family: EmbeddingProviderFamily,
        model_id: &str,
        raw_dimension: u32,
        effective_dimension: u32,
        input_contract: EmbeddingInputContract,
        normalization: EmbeddingNormalization,
        evidence: &T,
    ) -> Result<Self, EmbeddingIdentityError> {
        if !safe_identity_label(model_id)
            || raw_dimension > MAX_EMBEDDING_DIMENSION
            || effective_dimension > raw_dimension
        {
            return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
        }
        let raw_dimension = NonZeroU32::new(raw_dimension).ok_or(EmbeddingIdentityError::InvalidIdentityEvidence)?;
        let effective_dimension =
            NonZeroU32::new(effective_dimension).ok_or(EmbeddingIdentityError::InvalidIdentityEvidence)?;
        match normalization {
            EmbeddingNormalization::ProviderNative if raw_dimension != effective_dimension => {
                return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
            }
            EmbeddingNormalization::L2AfterMatryoshkaTruncationV1 if effective_dimension >= raw_dimension => {
                return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
            }
            _ => {}
        }

        let canonical =
            serde_json_canonicalizer::to_vec(evidence).map_err(|_| EmbeddingIdentityError::InvalidIdentityEvidence)?;
        let mut hasher = Sha256::new();
        hasher.update(PROVIDER_COMPATIBILITY_DOMAIN);
        hasher.update(canonical);
        let provider_compatibility_id = format!("{:x}", hasher.finalize());
        Ok(Self {
            provider_family,
            provider_compatibility_id,
            model_id: model_id.to_string(),
            raw_dimension,
            effective_dimension,
            input_contract,
            normalization,
        })
    }

    pub(crate) fn from_ollama_evidence(evidence: &OllamaIdentityEvidence) -> Result<Self, EmbeddingIdentityError> {
        if evidence.schema != "plico.embedding.ollama-evidence/v1"
            || !canonical_lower_hash(&evidence.model_digest, 64)
            || evidence.server_version.is_empty()
            || evidence.server_version.len() > 128
            || evidence.api_contract != "ollama-api-embed-truncate-false/v1"
        {
            return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
        }
        Self::from_verified_evidence(
            EmbeddingProviderFamily::Ollama,
            &evidence.model_tag,
            evidence.raw_dimension,
            evidence.raw_dimension,
            EmbeddingInputContract::MemoryTextUtf8V1,
            EmbeddingNormalization::ProviderNative,
            evidence,
        )
    }

    pub(crate) fn with_adaptive_contract(
        &self,
        transform_contract: EmbeddingTransformContract,
        target_dimension: Option<u32>,
    ) -> Result<Self, EmbeddingIdentityError> {
        #[derive(Serialize)]
        struct AdaptiveEvidence<'a> {
            schema: &'static str,
            inner_provider_compatibility_id: &'a str,
            prefix_contract_id: &'static str,
            requested_target_dimension: Option<u32>,
            effective_dimension: u32,
            normalization: EmbeddingNormalization,
        }

        if target_dimension.is_some_and(|dimension| dimension == 0 || dimension > self.effective_dimension()) {
            return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
        }
        let effective_dimension = target_dimension.unwrap_or_else(|| self.effective_dimension());
        let normalization = if effective_dimension < self.raw_dimension() {
            EmbeddingNormalization::L2AfterMatryoshkaTruncationV1
        } else {
            EmbeddingNormalization::ProviderNative
        };
        let prefix_contract_id = transform_contract.as_str();
        let evidence = AdaptiveEvidence {
            schema: "plico.embedding.adaptive-contract/v1",
            inner_provider_compatibility_id: self.provider_compatibility_id(),
            prefix_contract_id,
            requested_target_dimension: target_dimension,
            effective_dimension,
            normalization,
        };
        Self::from_verified_evidence(
            self.provider_family,
            &self.model_id,
            self.raw_dimension(),
            effective_dimension,
            self.input_contract,
            normalization,
            &evidence,
        )
    }

    #[cfg(test)]
    pub(crate) fn test_deterministic(model_id: &str, dimension: u32, contract: &str) -> Self {
        #[derive(Serialize)]
        struct Evidence<'a> {
            schema: &'static str,
            contract: &'a str,
            dimension: u32,
        }
        Self::from_verified_evidence(
            EmbeddingProviderFamily::TestDeterministicV1,
            model_id,
            dimension,
            dimension,
            EmbeddingInputContract::MemoryTextUtf8V1,
            EmbeddingNormalization::ProviderNative,
            &Evidence {
                schema: "plico.embedding.test-evidence/v1",
                contract,
                dimension,
            },
        )
        .expect("deterministic test identity must be valid")
    }

    pub const fn provider_family(&self) -> EmbeddingProviderFamily {
        self.provider_family
    }

    pub fn provider_compatibility_id(&self) -> &str {
        &self.provider_compatibility_id
    }

    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    pub const fn raw_dimension(&self) -> u32 {
        self.raw_dimension.get()
    }

    pub const fn effective_dimension(&self) -> u32 {
        self.effective_dimension.get()
    }

    pub const fn input_contract(&self) -> EmbeddingInputContract {
        self.input_contract
    }

    pub const fn normalization(&self) -> EmbeddingNormalization {
        self.normalization
    }

    pub const fn transform_contract_id(&self) -> &'static str {
        self.normalization.transform_contract_id()
    }
}

fn canonical_lower_hash(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn safe_identity_label(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.contains("://")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'"' | b'\'' | b'\\'))
}

impl EmbedError {
    pub fn ollama(msg: impl Into<String>) -> Self {
        EmbedError::Ollama(msg.into())
    }

    /// Stable, non-sensitive category for logs and transport diagnostics.
    ///
    /// Provider messages can contain response bodies, URLs, paths, or input
    /// fragments and must not be copied into structured runtime logs.
    pub const fn category(&self) -> &'static str {
        match self {
            Self::Http(_) => "http",
            Self::Ollama(_) => "ollama_api",
            Self::Api(_) => "provider_api",
            Self::ModelNotFound(_) => "model_not_found",
            Self::ServerUnavailable(_) => "server_unavailable",
            Self::Runtime(_) => "runtime_io",
            Self::Subprocess(_) => "subprocess",
            Self::SubprocessUnavailable => "subprocess_unavailable",
            Self::InputTooLarge(_) => "input_too_large",
        }
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

    /// Embed a document batch without erasing document-specific preprocessing.
    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    /// Return a verified immutable identity for this exact document builder.
    ///
    /// Implementations must fail closed when model revision, preprocessing,
    /// dimension, or output transformation cannot be proven.
    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError>;

    /// Whether Plico's adaptive transform has already been applied.
    fn has_plico_adaptive_transform(&self) -> bool {
        false
    }

    /// Output embedding dimension after any post-processing (e.g. Matryoshka truncation).
    fn dimension(&self) -> usize;

    /// Raw embedding dimension from the underlying model (before truncation).
    /// Default: same as [`dimension`].
    fn raw_dimension(&self) -> usize {
        self.dimension()
    }

    /// Name of the model used.
    fn model_name(&self) -> String;
}

/// A provider and its verified identity captured from the same hot-swap slot.
#[derive(Clone)]
pub struct VerifiedDocumentProviderSnapshot {
    provider: Arc<dyn EmbeddingProvider>,
    identity: EmbeddingBuilderIdentity,
}

impl VerifiedDocumentProviderSnapshot {
    pub(crate) fn verify(provider: Arc<dyn EmbeddingProvider>) -> Result<Self, EmbeddingIdentityError> {
        let identity = provider.builder_identity()?;
        if provider.raw_dimension() != identity.raw_dimension() as usize
            || provider.dimension() != identity.effective_dimension() as usize
        {
            return Err(EmbeddingIdentityError::ProviderChanged);
        }
        Ok(Self { provider, identity })
    }

    pub(crate) fn verify_exact(
        provider: Arc<dyn EmbeddingProvider>,
        expected: &EmbeddingBuilderIdentity,
    ) -> Result<Self, EmbeddingIdentityError> {
        let snapshot = Self::verify(provider)?;
        if snapshot.identity != *expected {
            return Err(EmbeddingIdentityError::ProviderChanged);
        }
        Ok(snapshot)
    }

    pub fn identity(&self) -> &EmbeddingBuilderIdentity {
        &self.identity
    }

    pub(crate) fn revalidate(&self) -> Result<(), EmbeddingIdentityError> {
        Self::verify_exact(Arc::clone(&self.provider), &self.identity).map(|_| ())
    }

    pub fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.provider.embed_document(text)
    }

    pub fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        self.provider.embed_document_batch(texts)
    }
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
        Self {
            embedding,
            input_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DimensionMismatchProvider;

    impl EmbeddingProvider for DimensionMismatchProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(vec![1.0, 1.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "mismatch",
                3,
                "mismatch-v1",
            ))
        }

        fn model_name(&self) -> String {
            "mismatch".into()
        }
    }

    #[test]
    fn verified_snapshot_rejects_provider_identity_dimension_mismatch() {
        assert!(matches!(
            VerifiedDocumentProviderSnapshot::verify(Arc::new(DimensionMismatchProvider)),
            Err(EmbeddingIdentityError::ProviderChanged)
        ));
    }
}
