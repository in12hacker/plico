//! OpenAI-compatible embedding backend — calls any `/v1/embeddings` endpoint.
//!
//! Works with: llama.cpp, vLLM, SGLang, TensorRT-LLM, text-embeddings-inference,
//! OpenAI, Ollama (/v1 endpoint), and any server exposing the OpenAI embeddings API.

use std::sync::{Arc, OnceLock};

use crate::fs::embedding::types::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
};

const TRANSPORT_MAX_ATTEMPTS: u8 = 2;

pub struct OpenAIEmbeddingBackend {
    /// Only created when no Tokio runtime is active (standalone/CLI mode).
    rt: Option<Arc<tokio::runtime::Runtime>>,
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    dimension: OnceLock<usize>,
    /// Optional instruction prefix for asymmetric retrieval models (e.g. Qwen3-Embedding).
    /// When set, `embed_query` prepends this to the text.
    query_prefix: Option<String>,
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
            Err(_) => Some(Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .enable_all()
                    .build()?,
            )),
        };

        let base = base_url.trim_end_matches('/').to_string();
        let retry_host = reqwest::Url::parse(&base)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .ok_or_else(|| EmbedError::Api("invalid embedding provider base URL".into()))?;
        let retry_policy = reqwest::retry::for_host(retry_host)
            .classify_fn(|request| {
                if request.error().is_some() {
                    tracing::warn!(
                        retry_scope = "transport_send",
                        max_attempts = TRANSPORT_MAX_ATTEMPTS,
                        "embedding provider transport failure classified for bounded retry"
                    );
                    request.retryable()
                } else {
                    request.success()
                }
            })
            .max_retries_per_request(u32::from(TRANSPORT_MAX_ATTEMPTS - 1))
            .no_budget();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(4)
            .retry(retry_policy)
            .build()
            .map_err(EmbedError::Http)?;

        Ok(Self {
            rt,
            client,
            base_url: base,
            model: model.to_string(),
            api_key,
            dimension: OnceLock::new(),
            query_prefix: None,
        })
    }

    /// Set an instruction prefix for asymmetric retrieval models.
    /// `embed_query` will prepend `"{prefix}{text}"` to queries.
    pub fn with_query_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.query_prefix = Some(prefix.into());
        self
    }

    fn get_dimension(&self) -> Result<usize, EmbedError> {
        if let Some(d) = self.dimension.get() {
            return Ok(*d);
        }
        let probe = Self::probe_dimension(&self.client, &self.base_url, &self.model, self.api_key.as_deref());
        let dim = match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(probe))?,
            Err(_) => self
                .rt
                .as_ref()
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
        let embedding =
            Self::embed_request(client, base_url, model, api_key, "dimension probe", "dimension_probe").await?;
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
        request_kind: &'static str,
    ) -> Result<EmbedResult, EmbedError> {
        let body = serde_json::json!({
            "model": model,
            "input": input,
        });

        let mut request = client.post(format!("{}/embeddings", base_url)).json(&body);
        if let Some(key) = api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let resp = request.send().await.map_err(|_| provider_request_error())?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(|_| {
            log_provider_failure(request_kind, "response_body_read", None);
            EmbedError::Api("provider response read failed".into())
        })?;

        if !status.is_success() {
            return Err(provider_status_error(status, &body_bytes, request_kind));
        }

        parse_embedding_response(&body_bytes, request_kind)
    }

    async fn embed_async(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        Self::embed_request(
            &self.client,
            &self.base_url,
            &self.model,
            self.api_key.as_deref(),
            text,
            "single",
        )
        .await
    }

    async fn embed_batch_async(&self, texts: &[String]) -> Result<Vec<EmbedResult>, EmbedError> {
        let body = serde_json::json!({
            "model": self.model,
            "input": texts,
        });

        let mut request = self.client.post(format!("{}/embeddings", self.base_url)).json(&body);
        if let Some(ref key) = self.api_key {
            request = request.header("Authorization", format!("Bearer {key}"));
        }
        let resp = request.send().await.map_err(|_| provider_request_error())?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(|_| {
            log_provider_failure("batch", "response_body_read", None);
            EmbedError::Api("provider response read failed".into())
        })?;

        if !status.is_success() {
            return Err(provider_status_error(status, &body_bytes, "batch"));
        }

        parse_embedding_batch_response(&body_bytes, "batch")
    }
}

fn log_provider_failure(request_kind: &'static str, failure_stage: &'static str, http_status_code: Option<u16>) {
    if let Some(http_status_code) = http_status_code {
        tracing::warn!(
            provider_protocol = "openai_compatible",
            request_kind,
            failure_stage,
            http_status_code,
            "embedding provider protocol failure"
        );
    } else {
        tracing::warn!(
            provider_protocol = "openai_compatible",
            request_kind,
            failure_stage,
            "embedding provider protocol failure"
        );
    }
}

fn provider_status_error(status: reqwest::StatusCode, body: &[u8], request_kind: &'static str) -> EmbedError {
    let body = String::from_utf8_lossy(body).to_ascii_lowercase();
    if body.contains("too large")
        || body.contains("batch size")
        || body.contains("too many tokens")
        || body.contains("context_length_exceeded")
        || body.contains("exceed_context_size")
        || body.contains("exceeds the available context size")
    {
        log_provider_failure(request_kind, "input_rejected", Some(status.as_u16()));
        EmbedError::InputTooLarge("provider rejected input size".into())
    } else {
        log_provider_failure(request_kind, "http_status", Some(status.as_u16()));
        EmbedError::Api(format!("provider returned HTTP status {}", status.as_u16()))
    }
}

fn provider_request_error() -> EmbedError {
    EmbedError::ServerUnavailable("embedding provider unavailable".into())
}

fn parse_embedding_response(body: &[u8], request_kind: &'static str) -> Result<EmbedResult, EmbedError> {
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

    let parsed: Response = serde_json::from_slice(body).map_err(|e| {
        log_provider_failure(request_kind, "response_json", None);
        EmbedError::Api(format!("response parse error: {e}"))
    })?;

    let embedding = parsed.data.into_iter().next().map(|d| d.embedding).ok_or_else(|| {
        log_provider_failure(request_kind, "empty_data", None);
        EmbedError::Api("empty data array in response".into())
    })?;

    let input_tokens = parsed.usage.map(|u| u.prompt_tokens).unwrap_or(0);
    Ok(EmbedResult::new(embedding, input_tokens))
}

fn parse_embedding_batch_response(body: &[u8], request_kind: &'static str) -> Result<Vec<EmbedResult>, EmbedError> {
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

    let parsed: Response = serde_json::from_slice(body).map_err(|e| {
        log_provider_failure(request_kind, "response_json", None);
        EmbedError::Api(format!("batch response parse error: {e}"))
    })?;

    let total_tokens = parsed.usage.map(|u| u.prompt_tokens).unwrap_or(0);
    let count = parsed.data.len();
    let tokens_per = if count == 0 { 0 } else { total_tokens / count as u32 };

    Ok(parsed
        .data
        .into_iter()
        .map(|d| EmbedResult::new(d.embedding, tokens_per))
        .collect())
}

impl EmbeddingProvider for OpenAIEmbeddingBackend {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(self.embed_async(text))),
            Err(_) => {
                if let Some(ref rt) = self.rt {
                    rt.block_on(self.embed_async(text))
                } else {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(EmbedError::Runtime)?;
                    rt.block_on(self.embed_async(text))
                }
            }
        }
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        match &self.query_prefix {
            Some(prefix) => {
                let prefixed = format!("{prefix}{text}");
                self.embed(&prefixed)
            }
            None => self.embed(text),
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(self.embed_batch_async(&owned))),
            Err(_) => {
                if let Some(ref rt) = self.rt {
                    rt.block_on(self.embed_batch_async(&owned))
                } else {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(EmbedError::Runtime)?;
                    rt.block_on(self.embed_batch_async(&owned))
                }
            }
        }
    }

    fn dimension(&self) -> usize {
        self.get_dimension().unwrap_or_default()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Err(EmbeddingIdentityError::UnpinnedRemoteModel)
    }

    fn model_name(&self) -> String {
        self.model.clone()
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
            query_prefix: self.query_prefix.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy)]
    enum RawServerAction {
        DropConnection,
        SingleSuccess,
        BatchSuccess,
        HttpError,
    }

    fn raw_retry_server(actions: Vec<RawServerAction>) -> Option<(String, Arc<AtomicUsize>, JoinHandle<()>)> {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("AF_INET unavailable; skipping raw retry protocol test");
                return None;
            }
            Err(error) => panic!("raw retry server bind failed: {error}"),
        };
        listener.set_nonblocking(true).expect("raw retry server nonblocking");
        let address = listener.local_addr().expect("raw retry server address");
        let calls = Arc::new(AtomicUsize::new(0));
        let thread_calls = Arc::clone(&calls);
        let thread = std::thread::spawn(move || {
            for action in actions {
                let mut stream = accept_before(&listener, Instant::now() + Duration::from_secs(2));
                thread_calls.fetch_add(1, Ordering::SeqCst);
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .expect("raw retry server read timeout");
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request);
                match action {
                    RawServerAction::DropConnection => {}
                    RawServerAction::SingleSuccess => respond_json(
                        &mut stream,
                        200,
                        r#"{"data":[{"embedding":[0.1,0.2,0.3]}],"usage":{"prompt_tokens":3}}"#,
                    ),
                    RawServerAction::BatchSuccess => respond_json(
                        &mut stream,
                        200,
                        r#"{"data":[{"embedding":[0.1,0.2]},{"embedding":[0.3,0.4]}],"usage":{"prompt_tokens":4}}"#,
                    ),
                    RawServerAction::HttpError => respond_json(&mut stream, 503, r#"{"error":{"type":"temporary"}}"#),
                }
            }
        });
        Some((format!("http://{address}/v1"), calls, thread))
    }

    fn accept_before(listener: &TcpListener, deadline: Instant) -> TcpStream {
        loop {
            match listener.accept() {
                Ok((stream, _)) => return stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("raw retry server accept failed: {error}"),
            }
        }
    }

    fn respond_json(stream: &mut TcpStream, status: u16, body: &str) {
        let reason = if status == 200 { "OK" } else { "Service Unavailable" };
        let response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("raw retry server response");
        stream.flush().expect("raw retry server flush");
    }

    #[test]
    fn test_parse_embedding_response_valid() {
        let json = br#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2,0.3]}],"model":"test","usage":{"prompt_tokens":3,"total_tokens":3}}"#;
        let result = parse_embedding_response(json, "test");
        assert!(result.is_ok());
        let emb = result.unwrap();
        assert_eq!(emb.embedding.len(), 3);
        assert!((emb.embedding[0] - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_parse_embedding_response_empty_data() {
        let json = br#"{"object":"list","data":[],"model":"test"}"#;
        let result = parse_embedding_response(json, "test");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty data"));
    }

    #[test]
    fn test_parse_embedding_response_malformed() {
        let json = br#"{"error":"bad request"}"#;
        let result = parse_embedding_response(json, "test");
        assert!(result.is_err());
    }

    #[test]
    fn provider_status_errors_do_not_expose_response_body() {
        const BODY_CANARY: &str = "EMBEDDING_RESPONSE_PERSONAL_CANARY_7c31";
        const ENDPOINT_CANARY: &str = "http://local-private-path/ENDPOINT_CANARY_4e29";
        let error = provider_status_error(
            reqwest::StatusCode::BAD_REQUEST,
            format!("too large {BODY_CANARY}").as_bytes(),
            "test",
        )
        .to_string();
        assert!(matches!(
            provider_status_error(reqwest::StatusCode::BAD_REQUEST, b"too large", "test"),
            EmbedError::InputTooLarge(_)
        ));
        assert!(matches!(
            provider_status_error(
                reqwest::StatusCode::BAD_REQUEST,
                br#"{"error":{"type":"exceed_context_size_error","message":"input exceeds the available context size"}}"#,
                "test",
            ),
            EmbedError::InputTooLarge(_)
        ));
        assert!(!error.contains(BODY_CANARY));

        let error = provider_status_error(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            BODY_CANARY.as_bytes(),
            "test",
        )
        .to_string();
        assert!(error.contains("status 500"));
        assert!(!error.contains(BODY_CANARY));

        let request_error = provider_request_error().to_string();
        assert!(!request_error.contains(ENDPOINT_CANARY));
        assert_eq!(request_error, "Server unavailable at embedding provider unavailable");
    }

    #[test]
    fn transport_failure_retries_once_and_recovers_single_request() {
        let Some((url, calls, thread)) =
            raw_retry_server(vec![RawServerAction::DropConnection, RawServerAction::SingleSuccess])
        else {
            return;
        };
        let backend = OpenAIEmbeddingBackend::new(&url, "test-model", None).unwrap();

        let result = backend.embed("safe synthetic input").unwrap();

        thread.join().unwrap();
        assert_eq!(result.embedding.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn transport_failure_exhaustion_stops_after_two_requests() {
        let Some((url, calls, thread)) =
            raw_retry_server(vec![RawServerAction::DropConnection, RawServerAction::DropConnection])
        else {
            return;
        };
        let backend = OpenAIEmbeddingBackend::new(&url, "test-model", None).unwrap();

        let result = backend.embed("safe synthetic input");

        thread.join().unwrap();
        assert!(matches!(result, Err(EmbedError::ServerUnavailable(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn http_status_is_not_retried() {
        let Some((url, calls, thread)) = raw_retry_server(vec![RawServerAction::HttpError]) else {
            return;
        };
        let backend = OpenAIEmbeddingBackend::new(&url, "test-model", None).unwrap();

        let result = backend.embed("safe synthetic input");

        thread.join().unwrap();
        assert!(matches!(result, Err(EmbedError::Api(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn transport_failure_retries_once_and_recovers_batch_request() {
        let Some((url, calls, thread)) =
            raw_retry_server(vec![RawServerAction::DropConnection, RawServerAction::BatchSuccess])
        else {
            return;
        };
        let backend = OpenAIEmbeddingBackend::new(&url, "test-model", None).unwrap();

        let result = backend.embed_batch(&["first", "second"]).unwrap();

        thread.join().unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_parse_embedding_batch_response() {
        let json = br#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2]},{"object":"embedding","index":1,"embedding":[0.3,0.4]}],"model":"test"}"#;
        let result = parse_embedding_batch_response(json, "test");
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
            Err(_) => {
                eprintln!("llama-server not available; skipping live-provider test");
                return;
            }
        };
        let result = backend.embed("Hello world");
        match result {
            Err(ref e)
                if e.to_string().to_lowercase().contains("unavailable")
                    || e.to_string().contains("connect")
                    || e.to_string().contains("501")
                    || e.to_string().contains("not_supported") =>
            {
                eprintln!("llama-server embedding not available; skipping live-provider test");
                return;
            }
            _ => {}
        }
        assert!(result.is_ok(), "embed should succeed: {:?}", result);
        let emb = result.unwrap();
        assert!(!emb.embedding.is_empty(), "embedding should not be empty");
        assert!(
            emb.embedding.len() > 10,
            "embedding dimension should be reasonable, got {}",
            emb.embedding.len()
        );
        println!("[llama-embedding] dim={}", emb.embedding.len());
    }

    #[test]
    fn test_openai_embedding_llama_server_batch() {
        let backend = match OpenAIEmbeddingBackend::new(&llama_embedding_url(), &llama_embedding_model(), None) {
            Ok(b) => b,
            Err(_) => {
                eprintln!("llama-server not available; skipping live-provider test");
                return;
            }
        };
        let result = backend.embed_batch(&["Hello", "World"]);
        match result {
            Err(ref e)
                if e.to_string().to_lowercase().contains("unavailable")
                    || e.to_string().contains("connect")
                    || e.to_string().contains("501")
                    || e.to_string().contains("not_supported") =>
            {
                eprintln!("llama-server embedding not available; skipping live-provider test");
                return;
            }
            _ => {}
        }
        assert!(result.is_ok(), "batch embed should succeed: {:?}", result);
        let embs = result.unwrap();
        assert_eq!(embs.len(), 2);
    }
}
