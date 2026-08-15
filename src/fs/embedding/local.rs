//! Local embedding backend via Python subprocess.

use std::io::{BufReader, Write};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

use crate::fs::embedding::json_rpc::{JsonRpcRequest, JsonRpcResponse};
use crate::fs::embedding::types::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
};

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
        let script = include_str!("local_worker.py");

        let mut child = std::process::Command::new(python_path)
            .arg("-c")
            .arg(script)
            .env("EMBEDDING_MODEL_ID", model_id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    EmbedError::SubprocessUnavailable
                } else {
                    EmbedError::Subprocess("local worker spawn failed".into())
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
        this.embed_single("plico local document operational probe v1")?;
        tracing::info!(
            provider_family = "local_hf",
            dimension = this.dimension,
            identity_available = false,
            "local embedding backend ready"
        );

        Ok(this)
    }

    fn probe(&self) -> Result<usize, EmbedError> {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct WorkerInfo {
            schema: String,
            model_id: String,
            raw_dimension: u32,
        }

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 0,
            method: "info".to_string(),
            params: serde_json::Value::Null,
        };
        let resp = self.send_rpc(req)?;
        let value = resp
            .result
            .ok_or_else(|| EmbedError::Subprocess("local worker info unavailable".into()))?;
        let info: WorkerInfo =
            serde_json::from_value(value).map_err(|_| EmbedError::Subprocess("local worker info invalid".into()))?;
        if info.schema != "plico.embedding.local-worker-info/v1"
            || info.model_id != self.model
            || info.raw_dimension == 0
            || info.raw_dimension > 65_536
        {
            return Err(EmbedError::Subprocess("local worker info invalid".into()));
        }
        Ok(info.raw_dimension as usize)
    }

    fn send_rpc(&self, req: JsonRpcRequest) -> Result<JsonRpcResponse, EmbedError> {
        let expected_id = req.id;
        let line = serde_json::to_string(&req)
            .map_err(|_| EmbedError::Subprocess("local worker request encoding failed".into()))?;
        let line = format!("{}\n", line);

        let handle = self.child.lock().unwrap();
        handle
            .to_stdin
            .as_ref()
            .expect("backend not dropped")
            .send(line)
            .map_err(|_| EmbedError::Subprocess("local worker request send failed".into()))?;

        let line = handle
            .from_stdout
            .recv_timeout(std::time::Duration::from_secs(40))
            .map_err(|_| EmbedError::Subprocess("local worker response unavailable".into()))?
            .map_err(|_| EmbedError::Subprocess("local worker response read failed".into()))?;

        let response: JsonRpcResponse =
            serde_json::from_str(&line).map_err(|_| EmbedError::Subprocess("local worker response invalid".into()))?;
        if response.jsonrpc != "2.0" || response.id != expected_id {
            return Err(EmbedError::Subprocess("local worker response envelope invalid".into()));
        }
        Ok(response)
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

        if resp.error.is_some() {
            return Err(EmbedError::Subprocess("local worker embed failed".into()));
        }
        let components = resp
            .result
            .as_ref()
            .and_then(|result| result.get("embedding"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| EmbedError::Subprocess("local worker embedding invalid".into()))?;
        let mut embedding = Vec::with_capacity(components.len());
        for component in components {
            let value = component
                .as_f64()
                .filter(|value| value.is_finite() && value.abs() <= f32::MAX as f64)
                .ok_or_else(|| EmbedError::Subprocess("local worker embedding invalid".into()))?
                as f32;
            embedding.push(value);
        }
        if embedding.len() != self.dimension
            || embedding.iter().any(|component| !component.is_finite())
            || embedding.iter().all(|component| *component == 0.0)
        {
            return Err(EmbedError::Subprocess("local worker embedding invalid".into()));
        }

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

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Err(EmbeddingIdentityError::LocalEvidenceIncomplete)
    }

    fn model_name(&self) -> String {
        self.model.clone()
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
