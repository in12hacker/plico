//! Embedding Service — Text → Vector
//!
//! Converts text into dense vector embeddings for semantic similarity search.
//!
//! # Architecture
//!
//! ```text
//! EmbeddingProvider (trait)
//! ├── OllamaBackend           — calls local Ollama daemon via HTTP
//! ├── LocalEmbeddingBackend  — Python subprocess (ONNX Runtime), stdio JSON-RPC
//! └── StubEmbeddingProvider   — error stub when no backend available
//! ```
//!
//! All backends are thread-safe (`Send + Sync`).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

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

// ─── Ollama Backend ───────────────────────────────────────────────────────────

/// Ollama daemon backend for text embeddings.
///
/// Spawns a dedicated tokio runtime in a background thread for async HTTP calls.
/// Thread-safe: the runtime handle is shared via Arc.
pub struct OllamaBackend {
    /// Tokio runtime for making async HTTP calls.
    rt: Arc<tokio::runtime::Runtime>,
    client: reqwest::Client,
    url: String,
    model: String,
    dimension: usize,
}

impl OllamaBackend {
    /// Create a new Ollama backend.
    ///
    /// `url` — Ollama server URL (e.g. `"http://localhost:11434"`).
    /// `model` — Model name (e.g. `"all-minilm-l6-v2"` or `"nomic-embed-text"`).
    pub fn new(url: &str, model: &str) -> Result<Self, EmbedError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(EmbedError::Http)?;

        let dimension = rt.block_on(Self::probe(&client, url, model)).unwrap_or_else(|e| {
            tracing::warn!("Ollama probe failed: {e}. Using default dimension 384.");
            384
        });

        Ok(Self {
            rt: Arc::new(rt),
            client,
            url: url.to_string(),
            model: model.to_string(),
            dimension,
        })
    }

    async fn probe(client: &reqwest::Client, url: &str, model: &str) -> Result<usize, EmbedError> {
        let tags_url = format!("{}/api/tags", url.trim_end_matches('/'));
        let resp: serde_json::Value = client
            .get(&tags_url)
            .send()
            .await
            .map_err(|_| EmbedError::ServerUnavailable(url.to_string()))?
            .json()
            .await
            .map_err(EmbedError::Http)?;

        let models = resp
            .get("models")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("name").and_then(|n| n.as_str()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if !models.iter().any(|m| m.starts_with(model)) {
            return Err(EmbedError::ModelNotFound(format!(
                "model '{}' not found. Available: {:?}",
                model, models
            )));
        }

        let dim = match model {
            m if m.contains("all-minilm") => 384,
            m if m.contains("nomic-embed") => 768,
            m if m.contains("e5") => 1024,
            m if m.contains("bge-large") => 1024,
            m if m.contains("bge-") => 768,
            _ => 384,
        };
        Ok(dim)
    }

    async fn embed_async(&self, text: &str) -> Result<Embedding, EmbedError> {
        #[derive(serde::Serialize)]
        struct Request<'a> {
            model: &'a str,
            prompt: &'a str,
        }
        #[derive(serde::Deserialize)]
        struct Response {
            embedding: Vec<f32>,
        }

        let resp = self
            .client
            .post(format!("{}/api/embeddings", self.url.trim_end_matches('/')))
            .json(&Request {
                model: &self.model,
                prompt: text,
            })
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() {
                    EmbedError::ServerUnavailable(self.url.clone())
                } else {
                    EmbedError::Http(e)
                }
            })?;

        let status = resp.status();
        let body_bytes = resp.bytes().await.map_err(EmbedError::Http)?;

        if !status.is_success() {
            let body_str = String::from_utf8_lossy(&body_bytes);
            return Err(EmbedError::ollama(format!("status={} body={}", status, body_str)));
        }

        let Response { embedding } = serde_json::from_slice(&body_bytes)
            .map_err(|e| EmbedError::ollama(format!("parse error: {e}")))?;
        Ok(embedding)
    }
}

impl EmbeddingProvider for OllamaBackend {
    fn embed(&self, text: &str) -> Result<Embedding, EmbedError> {
        // If called from within a tokio runtime (e.g. plicod's request handler),
        // block_on() would panic. block_in_place() moves this call off the async
        // scheduler temporarily, which is safe on a multi-thread runtime.
        // If no tokio runtime is present (e.g. aicli), fall back to a plain block_on.
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(self.embed_async(text)))
            }
            Err(_) => self.rt.block_on(self.embed_async(text)),
        }
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        let this = self.clone();
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let fut = async move {
            let mut results = Vec::with_capacity(texts.len());
            for text in &texts {
                results.push(this.embed_async(text).await?);
            }
            Ok(results)
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(fut)),
            Err(_) => self.rt.block_on(fut),
        }
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Clone for OllamaBackend {
    fn clone(&self) -> Self {
        Self {
            rt: Arc::clone(&self.rt),
            client: self.client.clone(),
            url: self.url.clone(),
            model: self.model.clone(),
            dimension: self.dimension,
        }
    }
}

// ─── Local Embedding Backend (Python subprocess) ───────────────────────────────

/// Local embedding backend via Python subprocess.
///
/// Uses a Python interpreter with ONNX Runtime + HuggingFace transformers
/// to run an embedding model entirely locally — no Ollama required.
///
/// **Subprocess pooling**: A single Python subprocess is spawned per process
/// and shared across all `LocalEmbeddingBackend` instances via module-level
/// pooling. This eliminates the ~4s model-loading overhead on repeated calls
/// within the same daemon process (e.g., `plicod`). For `aicli` (single
/// invocations), each call still pays the cold-start cost.
///
/// **Protocol**: JSON-RPC over stdio.
/// Each request is a JSON line written to stdin; each response is a JSON line
/// read from stdout. Fully decoupled: swap the Python script path or model
/// by changing environment variables.
///
/// ## Model
///
/// Default: `bge-small-en-v1.5` (384d, ~24MB, MTEB 62.17)
/// Fast (<100ms/sentence on 4-core CPU), high quality, Apache 2.0.
///
/// Alternative models can be configured via `EMBEDDING_MODEL_ID`:
/// - `BAAI/bge-small-en-v1.5` (default, English, 24MB)
/// - `TaylorAI/bge-small-gоторage-en-v1.5` (English, 24MB)
/// - `intfloat/e5-small-v2` (multilingual, 22MB)
///
/// ## Setup
///
/// ```bash
/// pip install transformers huggingface_hub onnxruntime
/// # Model downloads automatically on first run (~24MB)
/// ```
pub struct LocalEmbeddingBackend {
    child: std::sync::Mutex<ChildHandle>,
    model: String,
    dimension: usize,
    counter: AtomicUsize,
}

/// Wrapper that holds the Python subprocess handles.
/// Communicates via channel: sender writes JSON lines to stdin,
/// receiver reads JSON lines from stdout. A dedicated thread drains stdout.
struct ChildHandle {
    /// Process handle for wait().
    process: std::process::Child,
    /// Write JSON-RPC requests here. `Option` so we can take it in Drop.
    to_stdin: Option<std::sync::mpsc::Sender<String>>,
    /// Receive JSON-RPC responses here.
    from_stdout: std::sync::mpsc::Receiver<Result<String, std::io::Error>>,
}

impl LocalEmbeddingBackend {
    /// Create a new local embedding backend.
    ///
    /// `model_id` — HuggingFace model ID (default: `BAAI/bge-small-en-v1.5`).
    /// `python_path` — Path to python interpreter.
    ///
    /// Script location: `CARGO_MANIFEST_DIR/tests/e2e/embed_server.py`.
    /// This is an E2E test utility only. Production: use `EMBEDDING_BACKEND=ollama`.
    ///
    /// Returns an error if the Python script cannot be started.
    pub fn new(model_id: &str, python_path: &str) -> Result<Self, EmbedError> {
        let manifest_dir = std::env!("CARGO_MANIFEST_DIR");
        let script_path = format!("{}/tests/e2e/embed_server.py", manifest_dir);
        let script = std::fs::read_to_string(&script_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                EmbedError::SubprocessUnavailable
            } else {
                EmbedError::Subprocess(format!(
                    "e2e embed script not found at {}: {}. \
                    This script is E2E-only. Set EMBEDDING_BACKEND=ollama for production.",
                    script_path, e
                ))
            }
        })?;

        let mut child = std::process::Command::new(python_path)
            .arg("-c")
            .arg(script)
            .env("EMBEDDING_MODEL_ID", model_id)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    EmbedError::SubprocessUnavailable
                } else {
                    EmbedError::Subprocess(format!("failed to spawn python: {e}"))
                }
            })?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        // Channel: main thread sends requests, stdout-reader thread receives responses
        let (to_stdin, from_main) = std::sync::mpsc::channel::<String>();
        let (to_main, from_stdout) = std::sync::mpsc::channel();

        // Dedicated thread that reads stdout lines and forwards them to main thread
        std::thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        if to_main.send(Ok(line.clone())).is_err() {
                            break; // receiver dropped
                        }
                    }
                    Err(e) => {
                        let _ = to_main.send(Err(e));
                        break;
                    }
                }
            }
        });

        // Spawn stdin writer thread that reads from main and writes to python
        std::thread::spawn(move || {
            let mut stdin = stdin;
            for line in from_main.iter() {
                use std::io::Write;
                if stdin.write_all(line.as_bytes()).is_err() {
                    break;
                }
                if stdin.flush().is_err() {
                    break;
                }
            }
        });

        let handle = ChildHandle {
            process: child,
            to_stdin: Some(to_stdin),
            from_stdout,
        };

        let mut this = Self {
            child: std::sync::Mutex::new(handle),
            model: model_id.to_string(),
            dimension: 0,
            counter: AtomicUsize::new(0),
        };

        // Probe for dimension
        this.dimension = this.probe()?;

        tracing::info!(
            "LocalEmbeddingBackend ready: model={} dim={}",
            model_id,
            this.dimension
        );

        Ok(this)
    }

    /// Probe the Python server for model info (dimension).
    fn probe(&self) -> Result<usize, EmbedError> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 0,
            method: "info".to_string(),
            params: serde_json::Value::Null,
        };
        let resp = self.send_rpc(req)?;
        resp.result
            .and_then(|r| r.get("dimension").and_then(|d| d.as_u64()))
            .map(|d| d as usize)
            .ok_or_else(|| {
                EmbedError::Subprocess(
                    resp.error
                        .map(|e| e.message)
                        .unwrap_or_else(|| "info probe failed".into()),
                )
            })
    }

    fn send_rpc(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse, EmbedError> {
        // Serialize request
        let line = serde_json::to_string(&req).map_err(|e| EmbedError::Subprocess(e.to_string()))?;
        let line = format!("{}\n", line);

        // Send to stdin writer thread
        self.child
            .lock()
            .unwrap()
            .to_stdin
            .as_ref()
            .expect("backend not dropped")
            .send(line)
            .map_err(|e| EmbedError::Subprocess(format!("stdin send error: {e}")))?;

        // Wait for response from stdout reader thread
        let line = self
            .child
            .lock()
            .unwrap()
            .from_stdout
            .recv()
            .map_err(|e| EmbedError::Subprocess(format!("recv error: {e}")))?
            .map_err(|e| EmbedError::Subprocess(format!("stdout read error: {e}")))?;

        serde_json::from_str(&line)
            .map_err(|e| EmbedError::Subprocess(format!("parse error: {e}: {line}")))
    }

    fn embed_single(&self, text: &str) -> Result<Embedding, EmbedError> {
        let id = self.counter.fetch_add(1, Ordering::SeqCst) as i64;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "embed".to_string(),
            params: serde_json::json!({ "text": text }),
        };

        let resp = self.send_rpc(req)?;

        resp.result
            .and_then(|r| r.get("embedding").and_then(|e| {
                e.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect()
                })
            }))
            .ok_or_else(|| {
                EmbedError::Subprocess(
                    resp.error.map(|e| e.message).unwrap_or_else(|| "embed failed".into()),
                )
            })
    }
}

impl EmbeddingProvider for LocalEmbeddingBackend {
    fn embed(&self, text: &str) -> Result<Embedding, EmbedError> {
        self.embed_single(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed_single(text)?);
        }
        Ok(results)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

impl Drop for LocalEmbeddingBackend {
    fn drop(&mut self) {
        // Force-terminate the Python subprocess rather than waiting for the stdin
        // writer thread to gracefully drain and exit. Taking `to_stdin` out of
        // the ChildHandle drops it, closing the channel and signaling the
        // stdin-writer thread to exit. We then kill and wait for the process.
        let mut handle = self.child.lock().unwrap();
        // Take the sender out of Option — this drops it and closes the channel.
        let _ = handle.to_stdin.take();
        // Kill and reap the subprocess.
        let _ = handle.process.kill();
        let _ = handle.process.wait();
    }
}

// ─── JSON-RPC types ───────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: i64,
    method: String,
    params: serde_json::Value,
}

/// JSON-RPC response envelope. `jsonrpc` and `id` are required by the spec but
/// only `result` and `error.message` are consumed by this implementation.
#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct JsonRpcResponse {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[allow(dead_code)]
#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ─── Stub Backend ─────────────────────────────────────────────────────────────

/// A stub embedding provider used when no backend is available.
/// Always returns an error, triggering tag-based fallback in search.
#[derive(Default)]
pub struct StubEmbeddingProvider;

impl StubEmbeddingProvider {
    pub fn new() -> Self {
        Self
    }
}

impl EmbeddingProvider for StubEmbeddingProvider {
    fn embed(&self, _text: &str) -> Result<Embedding, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        384
    }

    fn model_name(&self) -> &str {
        "stub"
    }
}

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
