//! Ollama daemon backend for text embeddings.

use std::sync::{Arc, OnceLock};

use crate::fs::embedding::types::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
    OllamaIdentityEvidence,
};

/// Ollama daemon backend for text embeddings.
///
/// In daemon mode (Tokio runtime active), HTTP calls use `block_in_place`.
/// In standalone mode, a dedicated runtime is created.
pub struct OllamaBackend {
    /// Only created when no Tokio runtime is active (standalone/CLI mode).
    rt: Option<Arc<tokio::runtime::Runtime>>,
    client: reqwest::Client,
    url: String,
    model: String,
    dimension: OnceLock<usize>,
    verified_document: OnceLock<VerifiedOllamaDocumentProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerifiedOllamaDocumentProvider {
    evidence: OllamaIdentityEvidence,
    identity: EmbeddingBuilderIdentity,
}

#[derive(serde::Deserialize)]
struct OllamaTagModel {
    name: String,
    digest: String,
    #[serde(flatten)]
    _ignored: std::collections::BTreeMap<String, serde_json::Value>,
}

impl OllamaBackend {
    /// Create a new Ollama backend.
    ///
    /// `url` — Ollama server URL (e.g. `"http://localhost:11434"`).
    /// `model` — Model name (e.g. `"all-minilm-l6-v2"` or `"nomic-embed-text"`).
    pub fn new(url: &str, model: &str) -> Result<Self, EmbedError> {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(_) => None,
            Err(_) => Some(Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?,
            )),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(EmbedError::Http)?;

        Ok(Self {
            rt,
            client,
            url: url.to_string(),
            model: model.to_string(),
            dimension: OnceLock::new(),
            verified_document: OnceLock::new(),
        })
    }

    fn block_on_async<F: std::future::Future>(&self, fut: F) -> F::Output {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => self
                .rt
                .as_ref()
                .expect("rt must exist when no Tokio runtime is active")
                .block_on(fut),
        }
    }

    fn get_dimension(&self) -> Result<usize, EmbedError> {
        if let Some(d) = self.dimension.get() {
            return Ok(*d);
        }
        let dim = self
            .verified_document()
            .map_err(|_| EmbedError::ServerUnavailable("embedding builder identity unavailable".into()))?
            .evidence
            .raw_dimension as usize;
        self.dimension.set(dim).ok();
        Ok(dim)
    }

    fn verified_document(&self) -> Result<&VerifiedOllamaDocumentProvider, EmbeddingIdentityError> {
        if let Some(verified) = self.verified_document.get() {
            return Ok(verified);
        }
        let verified = self.resolve_verified_document()?;
        self.verified_document.set(verified).ok();
        self.verified_document
            .get()
            .ok_or(EmbeddingIdentityError::ProviderProbeFailed)
    }

    fn resolve_verified_document(&self) -> Result<VerifiedOllamaDocumentProvider, EmbeddingIdentityError> {
        let before = self
            .block_on_async(self.read_identity_evidence())
            .map_err(|_| EmbeddingIdentityError::ProviderProbeFailed)?;
        let probe = self
            .block_on_async(self.embed_document_request(&before.model_tag, "plico document identity probe v1"))
            .map_err(|_| EmbeddingIdentityError::ProviderProbeFailed)?;
        let after = self
            .block_on_async(self.read_identity_evidence())
            .map_err(|_| EmbeddingIdentityError::ProviderProbeFailed)?;
        if before != after {
            return Err(EmbeddingIdentityError::ProviderChanged);
        }
        let raw_dimension = u32::try_from(probe.embedding.len())
            .ok()
            .filter(|dimension| *dimension > 0 && *dimension <= 65_536)
            .ok_or(EmbeddingIdentityError::InvalidIdentityEvidence)?;
        let mut evidence = before;
        evidence.raw_dimension = raw_dimension;
        let identity = EmbeddingBuilderIdentity::from_ollama_evidence(&evidence)?;
        Ok(VerifiedOllamaDocumentProvider { evidence, identity })
    }

    async fn read_identity_evidence(&self) -> Result<OllamaIdentityEvidence, EmbedError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct TagsResponse {
            models: Vec<OllamaTagModel>,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct VersionResponse {
            version: String,
        }

        let base = self.url.trim_end_matches('/');
        let tags_response = self
            .client
            .get(format!("{base}/api/tags"))
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable("embedding provider unavailable".into()))?;
        if !tags_response.status().is_success() {
            return Err(EmbedError::Api(format!(
                "provider identity endpoint returned HTTP status {}",
                tags_response.status().as_u16()
            )));
        }
        let tags = tags_response
            .json::<TagsResponse>()
            .await
            .map_err(|_| EmbedError::Api("provider response parse failed".into()))?;
        let version_response = self
            .client
            .get(format!("{base}/api/version"))
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable("embedding provider unavailable".into()))?;
        if !version_response.status().is_success() {
            return Err(EmbedError::Api(format!(
                "provider identity endpoint returned HTTP status {}",
                version_response.status().as_u16()
            )));
        }
        let version = version_response
            .json::<VersionResponse>()
            .await
            .map_err(|_| EmbedError::Api("provider response parse failed".into()))?;
        let model = exact_ollama_tag(&self.model, &tags.models)
            .ok_or_else(|| EmbedError::ModelNotFound("configured embedding model".into()))?;
        Ok(OllamaIdentityEvidence {
            schema: "plico.embedding.ollama-evidence/v1".to_string(),
            model_tag: model.name.clone(),
            model_digest: model.digest.clone(),
            server_version: version.version,
            api_contract: "ollama-api-embed-truncate-false/v1".to_string(),
            raw_dimension: 0,
        })
    }

    async fn embed_document_request(&self, model: &str, text: &str) -> Result<EmbedResult, EmbedError> {
        #[derive(serde::Serialize)]
        struct Request<'a> {
            model: &'a str,
            input: &'a str,
            truncate: bool,
        }
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Response {
            model: String,
            embeddings: Vec<Vec<f32>>,
            #[serde(default)]
            prompt_eval_count: u32,
            #[serde(default)]
            total_duration: u64,
            #[serde(default)]
            load_duration: u64,
        }

        let response = self
            .client
            .post(format!("{}/api/embed", self.url.trim_end_matches('/')))
            .json(&Request {
                model,
                input: text,
                truncate: false,
            })
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable("embedding provider unavailable".into()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(EmbedError::Ollama(format!(
                "provider returned HTTP status {}",
                status.as_u16()
            )));
        }
        let mut parsed = response
            .json::<Response>()
            .await
            .map_err(|_| EmbedError::Ollama("provider response parse failed".into()))?;
        if parsed.model != model
            || parsed.embeddings.len() != 1
            || parsed.embeddings[0].is_empty()
            || parsed.embeddings[0].iter().any(|component| !component.is_finite())
            || parsed.embeddings[0].iter().all(|component| *component == 0.0)
        {
            return Err(EmbedError::Ollama("provider returned invalid embedding count".into()));
        }
        let _provider_timings = (parsed.total_duration, parsed.load_duration);
        Ok(EmbedResult::new(parsed.embeddings.remove(0), parsed.prompt_eval_count))
    }

    fn guarded_embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let verified = self
            .verified_document()
            .map_err(|_| EmbedError::ServerUnavailable("embedding builder identity unavailable".into()))?
            .clone();
        let before = self.block_on_async(self.read_identity_evidence())?;
        if !same_ollama_provider(&verified.evidence, &before) {
            return Err(EmbedError::ServerUnavailable(
                "embedding provider identity changed".into(),
            ));
        }
        let result = self.block_on_async(self.embed_document_request(&verified.evidence.model_tag, text))?;
        let after = self.block_on_async(self.read_identity_evidence())?;
        if !same_ollama_provider(&verified.evidence, &after)
            || result.embedding.len() != verified.evidence.raw_dimension as usize
        {
            return Err(EmbedError::ServerUnavailable(
                "embedding provider identity changed".into(),
            ));
        }
        Ok(result)
    }

    /// Send a chat request to Ollama with JSON structured output mode.
    pub async fn chat_async(
        &self,
        prompt: &str,
        system: Option<&str>,
        model_override: Option<&str>,
    ) -> Result<String, EmbedError> {
        #[derive(serde::Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            messages: Vec<ChatMessage<'a>>,
            format: &'a str,
            stream: bool,
            options: serde_json::Value,
        }

        #[derive(serde::Serialize)]
        struct ChatMessage<'a> {
            role: &'a str,
            content: &'a str,
        }

        #[derive(serde::Deserialize)]
        struct ChatResponse {
            message: ChatMessageOut,
        }

        #[derive(serde::Deserialize)]
        struct ChatMessageOut {
            content: String,
        }

        let model = model_override.unwrap_or(&self.model);

        let mut messages = Vec::new();
        if let Some(sys) = system {
            messages.push(ChatMessage {
                role: "system",
                content: sys,
            });
        }
        messages.push(ChatMessage {
            role: "user",
            content: prompt,
        });

        let req = ChatRequest {
            model,
            messages,
            format: "json",
            stream: false,
            options: serde_json::json!({
                "temperature": 0.1,
                "num_predict": 512
            }),
        };

        let resp = self
            .client
            .post(format!("{}/api/chat", self.url.trim_end_matches('/')))
            .json(&req)
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable("embedding provider unavailable".into()))?;

        let status = resp.status();
        let body_bytes = resp
            .bytes()
            .await
            .map_err(|_| EmbedError::Ollama("provider response read failed".into()))?;

        if !status.is_success() {
            return Err(EmbedError::Ollama(format!(
                "provider returned HTTP status {}",
                status.as_u16()
            )));
        }

        let parsed: ChatResponse = serde_json::from_slice(&body_bytes)
            .map_err(|_| EmbedError::Ollama("provider response parse failed".into()))?;

        Ok(parsed.message.content)
    }

    /// Synchronous wrapper for `chat_async`.
    pub fn chat(&self, prompt: &str, system: Option<&str>, model_override: Option<&str>) -> Result<String, EmbedError> {
        self.block_on_async(self.chat_async(prompt, system, model_override))
    }
}

impl EmbeddingProvider for OllamaBackend {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.guarded_embed_document(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.guarded_embed_document(text)).collect()
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.guarded_embed_document(text)
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.guarded_embed_document(text)
    }

    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.guarded_embed_document(text)).collect()
    }

    fn dimension(&self) -> usize {
        self.get_dimension().unwrap_or_default()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        let verified = self.verified_document()?.clone();
        let current = self
            .block_on_async(self.read_identity_evidence())
            .map_err(|_| EmbeddingIdentityError::ProviderProbeFailed)?;
        if !same_ollama_provider(&verified.evidence, &current) {
            return Err(EmbeddingIdentityError::ProviderChanged);
        }
        Ok(verified.identity)
    }

    fn model_name(&self) -> String {
        self.model.clone()
    }
}

impl Clone for OllamaBackend {
    fn clone(&self) -> Self {
        Self {
            rt: self.rt.as_ref().map(Arc::clone),
            client: self.client.clone(),
            url: self.url.clone(),
            model: self.model.clone(),
            dimension: OnceLock::new(),
            verified_document: OnceLock::new(),
        }
    }
}

fn exact_ollama_tag<'a>(configured: &str, models: &'a [OllamaTagModel]) -> Option<&'a OllamaTagModel> {
    if !configured.contains(':') {
        return None;
    }
    let mut matches = models.iter().filter(|model| model.name == configured);
    let selected = matches.next()?;
    matches.next().is_none().then_some(selected)
}

fn same_ollama_provider(expected: &OllamaIdentityEvidence, actual: &OllamaIdentityEvidence) -> bool {
    expected.model_tag == actual.model_tag
        && expected.model_digest == actual.model_digest
        && expected.server_version == actual.server_version
}

#[cfg(test)]
#[path = "ollama/tests.rs"]
mod tests;
