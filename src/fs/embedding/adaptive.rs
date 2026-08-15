//! Adaptive embedding wrapper — applies task prefixes and Matryoshka truncation.
//!
//! Wraps any `EmbeddingProvider` to add:
//! - Configurable query/document prefixes for asymmetric retrieval models
//! - Optional Matryoshka dimension truncation with L2 normalization
//!
//! Configuration (env vars):
//! - `EMBEDDING_QUERY_PREFIX`    — prepended to search queries (e.g. `"Query: "`)
//! - `EMBEDDING_DOCUMENT_PREFIX` — prepended to stored documents (e.g. `"Document: "`)
//! - `EMBEDDING_DIM`             — target dimension for Matryoshka truncation (omit to use native)

use crate::fs::embedding::types::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
    EmbeddingTransformContract,
};
use std::sync::Arc;

const QWEN3_QUERY_PREFIX: &str =
    "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ";

fn environment_target_dimension() -> Result<Option<usize>, EmbeddingIdentityError> {
    parse_target_dimension(std::env::var_os("EMBEDDING_DIM").as_deref())
}

fn parse_target_dimension(value: Option<&std::ffi::OsStr>) -> Result<Option<usize>, EmbeddingIdentityError> {
    match value {
        None => Ok(None),
        Some(value) => value
            .to_str()
            .and_then(|value| value.parse::<usize>().ok())
            .map(Some)
            .ok_or(EmbeddingIdentityError::InvalidIdentityEvidence),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrefixContract {
    Identity,
    Qwen3WebSearchV1,
    CustomUnsupported,
}

impl PrefixContract {
    const fn registered(self) -> Result<EmbeddingTransformContract, EmbeddingIdentityError> {
        match self {
            Self::Identity => Ok(EmbeddingTransformContract::ProviderNativeInputV1),
            Self::Qwen3WebSearchV1 => Ok(EmbeddingTransformContract::Qwen3WebSearchV1),
            Self::CustomUnsupported => Err(EmbeddingIdentityError::UnregisteredInputContract),
        }
    }
}

pub struct AdaptiveEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
    query_prefix: String,
    document_prefix: String,
    /// If set, truncate embeddings to this dimension and L2-normalize.
    target_dim: Option<usize>,
    prefix_contract: PrefixContract,
}

impl AdaptiveEmbeddingProvider {
    pub fn new(
        inner: Arc<dyn EmbeddingProvider>,
        query_prefix: String,
        document_prefix: String,
        target_dim: Option<usize>,
    ) -> Result<Self, EmbeddingIdentityError> {
        if inner.has_plico_adaptive_transform() {
            return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
        }
        let effective = inner.dimension();
        if target_dim.is_some_and(|dimension| dimension == 0 || effective == 0 || dimension > effective) {
            return Err(EmbeddingIdentityError::InvalidIdentityEvidence);
        }
        let prefix_contract = if query_prefix.is_empty() && document_prefix.is_empty() {
            PrefixContract::Identity
        } else if query_prefix == QWEN3_QUERY_PREFIX && document_prefix.is_empty() {
            PrefixContract::Qwen3WebSearchV1
        } else {
            PrefixContract::CustomUnsupported
        };
        Ok(Self {
            inner,
            query_prefix,
            document_prefix,
            target_dim,
            prefix_contract,
        })
    }

    /// Build from environment variables, wrapping a base provider.
    ///
    /// Auto-detects known model families and sets optimal prefixes:
    /// - Qwen3-Embedding: `"Instruct: ...\nQuery: "` for queries, no document prefix
    pub fn from_env(inner: Arc<dyn EmbeddingProvider>) -> Result<Self, EmbeddingIdentityError> {
        let query_prefix = std::env::var("EMBEDDING_QUERY_PREFIX").unwrap_or_default();
        let document_prefix = std::env::var("EMBEDDING_DOCUMENT_PREFIX").unwrap_or_default();
        let target_dim = environment_target_dimension()?;

        Self::new(inner, query_prefix, document_prefix, target_dim)
    }

    /// Build from config, wrapping a base provider.
    pub fn from_config(
        inner: Arc<dyn EmbeddingProvider>,
        config: &crate::config::InferenceConfig,
    ) -> Result<Self, EmbeddingIdentityError> {
        let mut query_prefix = config.query_prefix.clone().unwrap_or_default();
        let mut document_prefix = config.document_prefix.clone().unwrap_or_default();
        let target_dim = environment_target_dimension()?.or(config.target_dim);

        // Auto-detect model-specific prefixes when not explicitly set
        if query_prefix.is_empty() {
            let model = inner.model_name().to_lowercase();
            if model.contains("qwen3") && model.contains("embed") {
                query_prefix = QWEN3_QUERY_PREFIX.to_string();
                tracing::info!("Auto-detected Qwen3-Embedding, setting instruction-aware query prefix");
            }
        }

        // Support escaped newlines
        if query_prefix.contains("\\n") {
            query_prefix = query_prefix.replace("\\n", "\n");
        }
        if document_prefix.contains("\\n") {
            document_prefix = document_prefix.replace("\\n", "\n");
        }

        if !query_prefix.is_empty() || !document_prefix.is_empty() || target_dim.is_some() {
            tracing::info!(
                query_transform = !query_prefix.is_empty(),
                document_transform = !document_prefix.is_empty(),
                target_dimension = target_dim,
                "adaptive embedding configured"
            );
        }

        Self::new(inner, query_prefix, document_prefix, target_dim)
    }

    /// Whether this wrapper is a no-op passthrough (no prefix, no truncation).
    pub fn is_passthrough(&self) -> bool {
        self.query_prefix.is_empty() && self.document_prefix.is_empty() && self.target_dim.is_none()
    }

    fn effective_dim(&self) -> usize {
        self.target_dim.unwrap_or_else(|| self.inner.dimension())
    }

    fn postprocess(&self, mut result: EmbedResult) -> Result<EmbedResult, EmbedError> {
        if result.embedding.len() != self.inner.dimension() {
            tracing::warn!(
                provider_protocol = "adaptive_wrapper",
                failure_stage = "dimension_mismatch",
                "embedding provider protocol failure"
            );
            return Err(EmbedError::Api(
                "provider returned unexpected embedding dimension".into(),
            ));
        }
        if result.embedding.iter().any(|component| !component.is_finite())
            || result.embedding.iter().all(|component| *component == 0.0)
        {
            tracing::warn!(
                provider_protocol = "adaptive_wrapper",
                failure_stage = "invalid_vector",
                "embedding provider protocol failure"
            );
            return Err(EmbedError::Api("provider returned invalid embedding values".into()));
        }
        if let Some(td) = self.target_dim {
            if td < result.embedding.len() {
                result.embedding.truncate(td);
                if !l2_normalize(&mut result.embedding) {
                    tracing::warn!(
                        provider_protocol = "adaptive_wrapper",
                        failure_stage = "normalization",
                        "embedding provider protocol failure"
                    );
                    return Err(EmbedError::Api("embedding normalization failed".into()));
                }
            }
        }
        Ok(result)
    }

    fn postprocess_batch(&self, results: Vec<EmbedResult>) -> Result<Vec<EmbedResult>, EmbedError> {
        results.into_iter().map(|result| self.postprocess(result)).collect()
    }

    fn prefixed(&self, prefix: &str, text: &str) -> String {
        if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}")
        }
    }
}

fn l2_normalize(v: &mut [f32]) -> bool {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if !norm.is_finite() || norm <= 1e-10 {
        return false;
    }
    for x in v.iter_mut() {
        *x /= norm;
    }
    let normalized = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    normalized.is_finite() && (normalized - 1.0).abs() <= 1e-4
}

impl EmbeddingProvider for AdaptiveEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let result = self.inner.embed(text)?;
        self.postprocess(result)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let results = self.inner.embed_batch(texts)?;
        self.postprocess_batch(results)
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let prefixed = self.prefixed(&self.query_prefix, text);
        let result = self.inner.embed_query(&prefixed)?;
        self.postprocess(result)
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let prefixed = self.prefixed(&self.document_prefix, text);
        let result = self.inner.embed_document(&prefixed)?;
        self.postprocess(result)
    }

    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn dimension(&self) -> usize {
        self.effective_dim()
    }

    fn raw_dimension(&self) -> usize {
        self.inner.raw_dimension()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        let inner = self.inner.builder_identity()?;
        let transform_contract = self.prefix_contract.registered()?;
        let target_dimension = self
            .target_dim
            .map(u32::try_from)
            .transpose()
            .map_err(|_| EmbeddingIdentityError::InvalidIdentityEvidence)?;
        inner.with_adaptive_contract(transform_contract, target_dimension)
    }

    fn has_plico_adaptive_transform(&self) -> bool {
        true
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::embedding::EmbeddingNormalization;

    struct FakeProvider;

    impl EmbeddingProvider for FakeProvider {
        fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            let dim = 8;
            let embedding: Vec<f32> = (0..dim)
                .map(|i| (i as f32 + 1.0) * if text.contains("Query:") { 2.0 } else { 1.0 })
                .collect();
            Ok(EmbedResult::new(embedding, text.len() as u32 / 4))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimension(&self) -> usize {
            8
        }
        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic("fake", 8, "fake-v1"))
        }
        fn model_name(&self) -> String {
            "fake".into()
        }
    }

    #[test]
    fn test_passthrough_no_config() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(inner, String::new(), String::new(), None).unwrap();
        assert!(adaptive.is_passthrough());
        assert_eq!(adaptive.dimension(), 8);

        let result = adaptive.embed("hello").unwrap();
        assert_eq!(result.embedding.len(), 8);
    }

    #[test]
    fn test_matryoshka_truncation() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(inner, String::new(), String::new(), Some(4)).unwrap();
        assert!(!adaptive.is_passthrough());
        assert_eq!(adaptive.dimension(), 4);
        assert_eq!(adaptive.raw_dimension(), 8);

        let result = adaptive.embed("hello").unwrap();
        assert_eq!(result.embedding.len(), 4);

        let norm: f32 = result.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "should be L2-normalized, got norm={norm}");
        let identity = adaptive.builder_identity().unwrap();
        assert_eq!(identity.raw_dimension(), 8);
        assert_eq!(identity.effective_dimension(), 4);
        assert_eq!(
            identity.normalization(),
            EmbeddingNormalization::L2AfterMatryoshkaTruncationV1
        );
        assert_eq!(identity.transform_contract_id(), "plico-matryoshka-truncate-l2-v1");
    }

    #[test]
    fn test_query_document_prefixes() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(
            inner,
            "Instruct: retrieve semantically related passages\nQuery: ".to_string(),
            String::new(),
            None,
        )
        .unwrap();

        let q_result = adaptive.embed_query("hello").unwrap();
        let d_result = adaptive.embed_document("hello").unwrap();

        // FakeProvider multiplies by 2.0 when text starts with "Query:"
        assert!(
            q_result.embedding[0] > d_result.embedding[0],
            "query embedding should differ from document embedding"
        );
    }

    #[test]
    fn test_target_dim_exceeds_native_is_rejected() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(inner, String::new(), String::new(), Some(100));
        assert!(matches!(adaptive, Err(EmbeddingIdentityError::InvalidIdentityEvidence)));
    }

    #[test]
    fn target_equal_to_raw_keeps_provider_native_contract() {
        let adaptive =
            AdaptiveEmbeddingProvider::new(Arc::new(FakeProvider), String::new(), String::new(), Some(8)).unwrap();
        let identity = adaptive.builder_identity().unwrap();
        assert_eq!(identity.raw_dimension(), 8);
        assert_eq!(identity.effective_dimension(), 8);
        assert_eq!(identity.normalization(), EmbeddingNormalization::ProviderNative);
        assert_eq!(identity.transform_contract_id(), "provider-native-document-v1");
    }

    #[test]
    fn double_adaptive_and_custom_prefix_are_not_publishable() {
        let first =
            AdaptiveEmbeddingProvider::new(Arc::new(FakeProvider), String::new(), String::new(), Some(4)).unwrap();
        assert!(AdaptiveEmbeddingProvider::new(Arc::new(first), String::new(), String::new(), None).is_err());

        let custom = AdaptiveEmbeddingProvider::new(
            Arc::new(FakeProvider),
            "PRIVATE_PREFIX_CANARY".into(),
            String::new(),
            None,
        )
        .unwrap();
        assert!(matches!(
            custom.builder_identity(),
            Err(EmbeddingIdentityError::UnregisteredInputContract)
        ));
    }

    #[test]
    fn invalid_dimension_configuration_is_rejected() {
        assert!(parse_target_dimension(Some(std::ffi::OsStr::new("not-a-dimension"))).is_err());
        assert!(parse_target_dimension(Some(std::ffi::OsStr::new("0"))).is_ok());
        assert!(AdaptiveEmbeddingProvider::new(
            Arc::new(FakeProvider),
            String::new(),
            String::new(),
            parse_target_dimension(Some(std::ffi::OsStr::new("0"))).unwrap(),
        )
        .is_err());
    }

    struct VectorProvider(Vec<f32>);

    impl EmbeddingProvider for VectorProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(self.0.clone(), 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn dimension(&self) -> usize {
            self.0.len()
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                "vector-test",
                self.0.len() as u32,
                "vector-test-v1",
            ))
        }

        fn model_name(&self) -> String {
            "vector-test".into()
        }
    }

    #[test]
    fn invalid_provider_values_and_zero_truncated_prefix_are_rejected() {
        for vector in [vec![f32::NAN, 1.0], vec![f32::INFINITY, 1.0], vec![0.0, 0.0]] {
            let adaptive =
                AdaptiveEmbeddingProvider::new(Arc::new(VectorProvider(vector)), String::new(), String::new(), None)
                    .unwrap();
            assert!(adaptive.embed_document("test").is_err());
        }
        let adaptive = AdaptiveEmbeddingProvider::new(
            Arc::new(VectorProvider(vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0])),
            String::new(),
            String::new(),
            Some(4),
        )
        .unwrap();
        assert!(adaptive.embed_document("test").is_err());
    }
}
