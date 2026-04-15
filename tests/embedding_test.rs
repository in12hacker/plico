//! E2E test for LocalEmbeddingBackend — spawns Python subprocess and validates
//! JSON-RPC protocol + vector output.
//!
//! This is an E2E smoke test only. The Python script and model are NOT production
//! deliverables; they are test infrastructure for validating the decoupled embedding
//! pipeline.
//!
//! Run with:  cargo test -F e2e --test embedding_test
//!
//! Requires: pip install transformers huggingface_hub onnxruntime

use std::sync::mpsc;
use std::thread;
use std::io::Write;
use std::process::{Command, Stdio};

// ─── JSON-RPC types (mirrors embedding.rs) ────────────────────────────────

#[derive(Debug, serde::Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: i64,
    method: String,
    params: serde_json::Value,
}

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

#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

// ─── Protocol helpers ────────────────────────────────────────────────────────

fn spawn_embed_server() -> std::io::Result<(std::process::Child, mpsc::Sender<String>, mpsc::Receiver<Result<String, std::io::Error>>)> {
    let manifest_dir = std::env!("CARGO_MANIFEST_DIR");
    let script_path = format!("{}/tests/e2e/embed_server.py", manifest_dir);
    let script = std::fs::read_to_string(&script_path)?;

    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .env("EMBEDDING_MODEL_ID", "BAAI/bge-small-en-v1.5")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();

    let (to_stdin, from_main) = mpsc::channel::<String>();
    let (to_main, from_stdout) = mpsc::channel();

    // Reader thread — drains stdout line by line
    thread::spawn(move || {
        let mut reader = std::io::BufReader::new(stdout);
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

    // Writer thread — writes JSON-RPC lines to stdin
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

    Ok((child, to_stdin, from_stdout))
}

fn send_rpc(
    to_stdin: &mpsc::Sender<String>,
    from_stdout: &mpsc::Receiver<Result<String, std::io::Error>>,
    id: i64,
    method: &str,
    params: serde_json::Value,
) -> Result<JsonRpcResponse, String> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id,
        method: method.to_string(),
        params,
    };
    let line = serde_json::to_string(&req).map_err(|e| e.to_string())?;
    let line = format!("{}\n", line);
    to_stdin.send(line).map_err(|e| format!("send error: {}", e))?;

    let line = from_stdout
        .recv()
        .map_err(|e| format!("recv error: {}", e))?
        .map_err(|e| format!("stdout error: {}", e))?;

    serde_json::from_str(&line).map_err(|e| format!("parse error: {}: {}", e, line))
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn test_embed_info_probe() {
    // Skip if python3 not available
    let python_check = Command::new("python3").arg("--version").output();
    if python_check.is_err() {
        eprintln!("SKIP: python3 not available");
        return;
    }

    let (mut child, to_stdin, from_stdout) = spawn_embed_server().expect("spawn embed server");

    // Probe dimension
    let resp = send_rpc(&to_stdin, &from_stdout, 0, "info", serde_json::Value::Null)
        .expect("info rpc failed");

    assert!(resp.result.is_some(), "info returned error: {:?}", resp.error);
    let result = resp.result.unwrap();

    let dimension = result.get("dimension")
        .and_then(|d| d.as_u64())
        .expect("dimension field missing");
    assert_eq!(dimension, 384, "bge-small-en-v1.5 should be 384d");

    let model = result.get("model")
        .and_then(|m| m.as_str())
        .expect("model field missing");
    assert!(model.contains("bge-small"), "expected bge-small model, got: {}", model);

    drop(to_stdin);
    let _ = child.wait();
}

#[test]
fn test_embed_single_text() {
    let python_check = Command::new("python3").arg("--version").output();
    if python_check.is_err() {
        eprintln!("SKIP: python3 not available");
        return;
    }

    let (mut child, to_stdin, from_stdout) = spawn_embed_server().expect("spawn embed server");

    let resp = send_rpc(
        &to_stdin,
        &from_stdout,
        1,
        "embed",
        serde_json::json!({ "text": "hello world" }),
    ).expect("embed rpc failed");

    assert!(resp.result.is_some(), "embed returned error: {:?}", resp.error);
    let result = resp.result.unwrap();

    let embedding = result.get("embedding")
        .and_then(|e| e.as_array())
        .cloned()
        .expect("embedding field missing");

    assert_eq!(embedding.len(), 384, "bge-small-en-v1.5 produces 384-dim vectors");
    for val in &embedding {
        let f = val.as_f64().expect("embedding value must be f64");
        assert!(f.is_finite(), "embedding values must be finite");
    }

    drop(to_stdin);
    let _ = child.wait();
}

#[test]
fn test_embed_deterministic() {
    let python_check = Command::new("python3").arg("--version").output();
    if python_check.is_err() {
        eprintln!("SKIP: python3 not available");
        return;
    }

    let (mut child, to_stdin, from_stdout) = spawn_embed_server().expect("spawn embed server");

    let text = "the quick brown fox jumps over the lazy dog";

    let resp1 = send_rpc(&to_stdin, &from_stdout, 10, "embed", serde_json::json!({ "text": text }))
        .expect("embed rpc failed");
    let resp2 = send_rpc(&to_stdin, &from_stdout, 11, "embed", serde_json::json!({ "text": text }))
        .expect("embed rpc failed");

    let emb1_result = resp1.result.unwrap();
    let emb2_result = resp2.result.unwrap();
    let emb1 = emb1_result.get("embedding").unwrap().as_array().unwrap();
    let emb2 = emb2_result.get("embedding").unwrap().as_array().unwrap();

    // ONNX/PyTorch embeddings are deterministic for the same input
    assert_eq!(emb1.len(), emb2.len());
    for (a, b) in emb1.iter().zip(emb2.iter()) {
        assert_eq!(a.as_f64(), b.as_f64(), "embedding should be deterministic");
    }

    drop(to_stdin);
    let _ = child.wait();
}

#[test]
fn test_embed_different_texts_different_vectors() {
    let python_check = Command::new("python3").arg("--version").output();
    if python_check.is_err() {
        eprintln!("SKIP: python3 not available");
        return;
    }

    let (mut child, to_stdin, from_stdout) = spawn_embed_server().expect("spawn embed server");

    let resp1 = send_rpc(&to_stdin, &from_stdout, 20, "embed", serde_json::json!({ "text": "apple" }))
        .expect("embed rpc failed");
    let resp2 = send_rpc(&to_stdin, &from_stdout, 21, "embed", serde_json::json!({ "text": "banana" }))
        .expect("embed rpc failed");

    let emb1_result = resp1.result.unwrap();
    let emb2_result = resp2.result.unwrap();
    let emb1 = emb1_result.get("embedding").unwrap().as_array().unwrap();
    let emb2 = emb2_result.get("embedding").unwrap().as_array().unwrap();

    // Cosine similarity: normalize both vectors then compute dot product
    let dot: f64 = emb1.iter()
        .zip(emb2.iter())
        .map(|(a, b)| a.as_f64().unwrap() * b.as_f64().unwrap())
        .sum();
    let norm1: f64 = emb1.iter().map(|v| v.as_f64().unwrap().powi(2)).sum::<f64>().sqrt();
    let norm2: f64 = emb2.iter().map(|v| v.as_f64().unwrap().powi(2)).sum::<f64>().sqrt();
    let cos_sim = dot / (norm1 * norm2);

    // "apple" and "banana" share fruit semantics — expect moderate positive similarity
    assert!(cos_sim > 0.0, "expected positive cosine similarity, got {}", cos_sim);
    assert!(cos_sim < 0.99, "different texts should not have near-identical embeddings");

    drop(to_stdin);
    let _ = child.wait();
}

#[test]
fn test_embed_invalid_method() {
    let python_check = Command::new("python3").arg("--version").output();
    if python_check.is_err() {
        eprintln!("SKIP: python3 not available");
        return;
    }

    let (mut child, to_stdin, from_stdout) = spawn_embed_server().expect("spawn embed server");

    let resp = send_rpc(
        &to_stdin,
        &from_stdout,
        99,
        "unknown_method",
        serde_json::json!({}),
    ).expect("embed rpc failed");

    // Server should return an error response for unknown method
    assert!(resp.error.is_some(), "expected error response for unknown method");
    let err = resp.error.unwrap();
    assert!(err.code != 0 || !err.message.is_empty(), "error should have a message");

    drop(to_stdin);
    let _ = child.wait();
}
