//! OpenAI-compatible embedding backend — calls any `/v1/embeddings` endpoint.
//!
//! Works with: llama.cpp, vLLM, SGLang, TensorRT-LLM, text-embeddings-inference,
//! OpenAI, Ollama (/v1 endpoint), and any server exposing the OpenAI embeddings API.

use std::sync::{Arc, OnceLock};

use crate::fs::embedding::types::{EmbedError, EmbeddingProvider, EmbedResult};

pub struct OpenAIEmbeddingBackend {
    /// Only created when no Tokio runtime is active (standalone/CLI mode).
    rt: Option<Arc<tokio::runtime::Runtime>>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    dimension: OnceLock<usize>,
}

impl OpenAIEmbeddingBackend {
    /// Create a new OpenAI-compatible embedding backend.
    ///
    /// `base_url` — Server base URL (e.g. `"http://127.0.0.1:8080/v1"`).
    /// `model` — Model name sent in the request body.
    /// `api_key` — Optional Bearer token for authenticated endpoints.
    pub fn new(base_url: &str, model: &str, api_key: Option<String>) -> Result<Self, EmbedError> {
        let rt = match tokio::runtime::Handle::try_current() {
            Ok(_) => None,
            Err(_) => {
                Some(Arc::new(tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?))
            }
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(EmbedError::Http)?;

        let base = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            rt,
            client,
            base_url: base,
            model: model.to_string(),
            api_key,
            dimension: OnceLock::new(),
        })
    }

    fn get_dimension(&self) -> Result<usize, EmbedError> {
        if let Some(d) = self.dimension.get() {
            return Ok(*d);
        }
        let probe = Self::probe_dimension(&self.client, &self.base_url, &self.model, self.api_key.as_deref());
        let dim = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(probe))?,
            Err(_) => self.rt.as_ref()
                .expect("rt must exist when no Tokio runtime is active")
                .block_on(probe)?,
        };
        self.dimension.set(dim).ok();
        Ok(dim)
    }

    async fn probe_dimension(
        client: &reqwest::Client,
        base_url: &str,
        model: &str,
        api_key: Option<&str>,
    ) -> Result<usize, EmbedError> {
        let embedding = Self::embed_request(client, base_url, model, api_key, "dimension probe").await?;
        if embedding.embedding.is_empty() {
            return Err(EmbedError::ServerUnavailable(
                "probe returned empty embedding".to_string(),
            ));
        }
        Ok(embedding.embedding.len())
    }

    async fn embed_request(
        client: &reqwest::Client,
        base_url: &str,
        model: &str,
        api_key: Option<&str>,
        input: &str,
    ) -> Result<EmbedResult, EmbedError> {
        let body = serde_json::json!({
            "model": model,
            "input": input,
        });

        let mut req = client
            .post(format!("{}/embeddings", base_url))
            .json(&body);

        if let Some(key) = api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_connect() {
                EmbedError::ServerUnavailable(base_url.to_string())
            } else {
                EmbedError::Http(e)
            }
        })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(EmbedError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(EmbedError::Api(format!("status={status} body={body_str}")));
        }

        parse_embedding_response(&body_bytes)
    }

    async fn embed_async(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        Self::embed_request(&self.client, &self.base_url, &self.model, self.api_key.as_deref(), text).await
    }

    async fn embed_batch_async(&self, texts: &[String]) -> Result<Vec<EmbedResult>, EmbedError> {
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let mut req = self.client.post(format!("{}/embeddings", self.base_url)).json(&body);

        if let Some(ref key) = self.api_key {
            req = req.header("Authorization", format!("Bearer {key}"));
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_connect() {
                EmbedError::ServerUnavailable(self.base_url.clone())
            } else {
                EmbedError::Http(e)
            }
        })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(EmbedError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(EmbedError::Api(format!("status={status} body={body_str}")));
        }

        parse_embedding_batch_response(&body_bytes)
    }
}

fn parse_embedding_response(body: &[u8]) -> Result<EmbedResult, EmbedError> {
    #[derive(serde::Deserialize)]
    struct Response {
        data: Vec<EmbeddingData>,
        usage: Option<Usage>,
    }
    #[derive(serde::Deserialize)]
    struct EmbeddingData {
        embedding: Vec<f32>,
    }
    #[derive(serde::Deserialize)]
    struct Usage {
        prompt_tokens: u32,
    }

    let parsed: Response = serde_json::from_slice(body)
        .map_err(|e| EmbedError::Api(format!("response parse error: {e}")))?;

    let embedding = parsed
        .data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| EmbedError::Api("empty data array in response".into()))?;

    let input_tokens = parsed.usage.map(|u| u.prompt_tokens).unwrap_or(0);
    Ok(EmbedResult::new(embedding, input_tokens))
}

fn parse_embedding_batch_response(body: &[u8]) -> Result<Vec<EmbedResult>, EmbedError> {
    #[derive(serde::Deserialize)]
    struct Response {
        data: Vec<EmbeddingData>,
        usage: Option<Usage>,
    }
    #[derive(serde::Deserialize)]
    struct EmbeddingData {
        embedding: Vec<f32>,
    }
    #[derive(serde::Deserialize)]
    struct Usage {
        prompt_tokens: u32,
    }

    let parsed: Response = serde_json::from_slice(body)
        .map_err(|e| EmbedError::Api(format!("batch response parse error: {e}")))?;

    let total_tokens = parsed.usage.map(|u| u.prompt_tokens).unwrap_or(0);
    let count = parsed.data.len();
    let tokens_per = if count == 0 { 0 } else { total_tokens / count as u32 };

    Ok(parsed.data.into_iter().map(|d| EmbedResult::new(d.embedding, tokens_per)).collect())
}

impl EmbeddingProvider for OpenAIEmbeddingBackend {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.embed_async(text)))
            }
            Err(_) => self.rt.as_ref()
                .expect("rt must exist when no Tokio runtime is active")
                .block_on(self.embed_async(text)),
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.embed_batch_async(&owned)))
            }
            Err(_) => self.rt.as_ref()
                .expect("rt must exist when no Tokio runtime is active")
                .block_on(self.embed_batch_async(&owned)),
        }
    }

    fn dimension(&self) -> usize {
        self.get_dimension().unwrap_or(384)
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OpenAIEmbeddingBackend {
    fn clone(&self) -> Self {
        Self {
            rt: self.rt.as_ref().map(Arc::clone),
            client: self.client.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            dimension: OnceLock::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_embedding_response_valid() {
        let json = br#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2,0.3]}],"model":"test","usage":{"prompt_tokens":3,"total_tokens":3}}"#;
        let result = parse_embedding_response(json);
        assert!(result.is_ok());
        let emb = result.unwrap();
        assert_eq!(emb.embedding.len(), 3);
        assert!((emb.embedding[0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_parse_embedding_response_empty_data() {
        let json = br#"{"object":"list","data":[],"model":"test"}"#;
        let result = parse_embedding_response(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty data"));
    }

    #[test]
    fn test_parse_embedding_response_malformed() {
        let json = br#"{"error":"bad request"}"#;
        let result = parse_embedding_response(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_embedding_batch_response() {
        let json = br#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2]},{"object":"embedding","index":1,"embedding":[0.3,0.4]}],"model":"test"}"#;
        let result = parse_embedding_batch_response(json);
        assert!(result.is_ok());
        let embs = result.unwrap();
        assert_eq!(embs.len(), 2);
        assert_eq!(embs[0].embedding.len(), 2);
        assert_eq!(embs[1].embedding.len(), 2);
    }

    fn llama_embedding_url() -> String {
        std::env::var("LLAMA_TEST_URL").unwrap_or_else(|_| "http://127.0.0.1:18920/v1".to_string())
    }
    fn llama_embedding_model() -> String {
        std::env::var("LLAMA_TEST_MODEL").unwrap_or_else(|_| "qwen2.5-0.5b-instruct-q4_k_m.gguf".to_string())
    }

    #[test]
    fn test_openai_embedding_llama_server() {
        let backend = match OpenAIEmbeddingBackend::new(&llama_embedding_url(), &llama_embedding_model(), None) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("llama-server not available, skipping: {e}");
                return;
            }
        };
        let result = backend.embed("Hello world");
        match result {
            Err(ref e) if e.to_string().to_lowercase().contains("unavailable") || e.to_string().contains("connect") => {
                eprintln!("llama-server not reachable, skipping: {e}");
                return;
            }
            _ => {}
        }
        assert!(result.is_ok(), "embed should succeed: {:?}", result);
        let emb = result.unwrap();
        assert!(!emb.embedding.is_empty(), "embedding should not be empty");
        assert!(emb.embedding.len() > 10, "embedding dimension should be reasonable, got {}", emb.embedding.len());
        println!("[llama-embedding] dim={}", emb.embedding.len());
    }

    #[test]
    fn test_openai_embedding_llama_server_batch() {
        let backend = match OpenAIEmbeddingBackend::new(&llama_embedding_url(), &llama_embedding_model(), None) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("llama-server not available, skipping: {e}");
                return;
            }
        };
        let result = backend.embed_batch(&["Hello", "World"]);
        match result {
            Err(ref e) if e.to_string().to_lowercase().contains("unavailable") || e.to_string().contains("connect") => {
                eprintln!("llama-server not reachable, skipping: {e}");
                return;
            }
            _ => {}
        }
        assert!(result.is_ok(), "batch embed should succeed: {:?}", result);
        let embs = result.unwrap();
        assert_eq!(embs.len(), 2);
    }
}
