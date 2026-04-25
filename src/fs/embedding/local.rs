//! Local embedding backend via Python subprocess.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::io::{BufReader, Write};
use std::process::{Child, Stdio};

use crate::fs::embedding::types::{EmbedError, Embedding, EmbeddingProvider, EmbedResult};
use crate::fs::embedding::json_rpc::{JsonRpcRequest, JsonRpcResponse};

/// Local embedding backend via Python subprocess.
///
/// Uses a Python interpreter with ONNX Runtime + HuggingFace transformers
/// to run an embedding model entirely locally — no Ollama required.
pub struct LocalEmbeddingBackend {
    child: std::sync::Mutex<ChildHandle>,
    model: String,
    dimension: usize,
    counter: AtomicUsize,
}

/// Wrapper that holds the Python subprocess handles.
struct ChildHandle {
    process: Child,
    /// Write JSON-RPC requests here. `Option` so we can take it in Drop.
    to_stdin: Option<mpsc::Sender<String>>,
    /// Receive JSON-RPC responses here.
    from_stdout: mpsc::Receiver<Result<String, std::io::Error>>,
}

impl LocalEmbeddingBackend {
    /// Create a new local embedding backend.
    ///
    /// `model_id` — HuggingFace model ID (default: `BAAI/bge-small-en-v1.5`).
    /// `python_path` — Path to python interpreter.
    pub fn new(model_id: &str, python_path: &str) -> Result<Self, EmbedError> {
        let manifest_dir = std::env!("CARGO_MANIFEST_DIR");
        let script_path = format!("{}/tests/e2e/embed_server.py", manifest_dir);
        let script = std::fs::read_to_string(&script_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                EmbedError::SubprocessUnavailable
            } else {
                EmbedError::Subprocess(format!(
                    "e2e embed script not found at {}: {}",
                    script_path, e
                ))
            }
        })?;

        let mut child = std::process::Command::new(python_path)
            .arg("-c")
            .arg(script)
            .env("EMBEDDING_MODEL_ID", model_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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

        let (to_stdin, from_main) = mpsc::channel::<String>();
        let (to_main, from_stdout) = mpsc::channel();

        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match std::io::BufRead::read_line(&mut reader, &mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if to_main.send(Ok(line.clone())).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = to_main.send(Err(e));
                        break;
                    }
                }
            }
        });

        thread::spawn(move || {
            let mut stdin = stdin;
            for line in from_main.iter() {
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

        this.dimension = this.probe()?;
        tracing::info!(
            "LocalEmbeddingBackend ready: model={} dim={}",
            model_id,
            this.dimension
        );

        Ok(this)
    }

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
        let line = serde_json::to_string(&req).map_err(|e| EmbedError::Subprocess(e.to_string()))?;
        let line = format!("{}\n", line);

        self.child
            .lock()
            .unwrap()
            .to_stdin
            .as_ref()
            .expect("backend not dropped")
            .send(line)
            .map_err(|e| EmbedError::Subprocess(format!("stdin send error: {e}")))?;

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

    fn embed_single(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let id = self.counter.fetch_add(1, Ordering::SeqCst) as i64;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: "embed".to_string(),
            params: serde_json::json!({ "text": text }),
        };

        let resp = self.send_rpc(req)?;

        let embedding = resp
            .result
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
            })?;

        // Local subprocess doesn't return token counts — estimate
        let estimated_tokens = (text.len() / 4).max(1) as u32;
        Ok(EmbedResult::new(embedding, estimated_tokens))
    }
}

impl EmbeddingProvider for LocalEmbeddingBackend {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_single(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
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
        let mut handle = self.child.lock().unwrap();
        let _ = handle.to_stdin.take();
        let _ = handle.process.kill();
        let _ = handle.process.wait();
    }
}
