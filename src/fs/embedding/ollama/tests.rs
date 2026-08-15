use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use tiny_http::{Header, Response, Server, StatusCode};

use super::*;
use crate::fs::EmbeddingProviderFamily;

const MODEL: &str = "nomic-embed-text:latest";
const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

const MODE_VALID: u8 = 0;
const MODE_WRONG_MODEL: u8 = 1;
const MODE_WRONG_COUNT: u8 = 2;
const MODE_ZERO: u8 = 3;
const MODE_WRONG_DIMENSION: u8 = 4;
const MODE_INVALID_NUMBER: u8 = 5;
const MODE_ERROR_BODY: u8 = 6;

struct MockState {
    stop: AtomicBool,
    tag_reads: AtomicUsize,
    switch_digest_at: AtomicUsize,
    invalid_digest: AtomicBool,
    duplicate_tag: AtomicBool,
    evidence_status: AtomicUsize,
    mode: AtomicU8,
    paths: Mutex<Vec<String>>,
    embed_bodies: Mutex<Vec<String>>,
}

struct MockOllama {
    url: String,
    state: Arc<MockState>,
    thread: Option<JoinHandle<()>>,
}

impl MockOllama {
    fn start() -> Self {
        let server = Server::http("127.0.0.1:0").expect("mock server bind");
        let url = format!("http://{}", server.server_addr());
        let state = Arc::new(MockState {
            stop: AtomicBool::new(false),
            tag_reads: AtomicUsize::new(0),
            switch_digest_at: AtomicUsize::new(usize::MAX),
            invalid_digest: AtomicBool::new(false),
            duplicate_tag: AtomicBool::new(false),
            evidence_status: AtomicUsize::new(200),
            mode: AtomicU8::new(MODE_VALID),
            paths: Mutex::new(Vec::new()),
            embed_bodies: Mutex::new(Vec::new()),
        });
        let thread_state = Arc::clone(&state);
        let thread = std::thread::spawn(move || {
            while !thread_state.stop.load(Ordering::Acquire) {
                let Some(mut request) = server.recv_timeout(Duration::from_millis(20)).expect("mock receive") else {
                    continue;
                };
                let path = request.url().to_string();
                thread_state.paths.lock().unwrap().push(path.clone());
                let (status, body) = match path.as_str() {
                    "/api/tags" => {
                        let read = thread_state.tag_reads.fetch_add(1, Ordering::SeqCst) + 1;
                        let digest = if thread_state.invalid_digest.load(Ordering::Acquire) {
                            "not-a-canonical-digest"
                        } else if read >= thread_state.switch_digest_at.load(Ordering::SeqCst) {
                            DIGEST_B
                        } else {
                            DIGEST_A
                        };
                        let mut models = vec![serde_json::json!({"name": MODEL, "digest": digest, "size": 3})];
                        if thread_state.duplicate_tag.load(Ordering::Acquire) {
                            models.push(serde_json::json!({"name": MODEL, "digest": digest, "size": 3}));
                        }
                        (
                            thread_state.evidence_status.load(Ordering::Acquire) as u16,
                            serde_json::json!({"models": models}).to_string(),
                        )
                    }
                    "/api/version" => (
                        thread_state.evidence_status.load(Ordering::Acquire) as u16,
                        serde_json::json!({"version": "0.11.5"}).to_string(),
                    ),
                    "/api/embed" => {
                        let mut input = String::new();
                        request
                            .as_reader()
                            .read_to_string(&mut input)
                            .expect("mock request body");
                        thread_state.embed_bodies.lock().unwrap().push(input);
                        match thread_state.mode.load(Ordering::Acquire) {
                            MODE_VALID => (200, embed_response(MODEL, vec![vec![1.0, 2.0, 3.0]])),
                            MODE_WRONG_MODEL => (200, embed_response("other:latest", vec![vec![1.0, 2.0, 3.0]])),
                            MODE_WRONG_COUNT => (200, embed_response(MODEL, vec![vec![1.0; 3], vec![2.0; 3]])),
                            MODE_ZERO => (200, embed_response(MODEL, vec![vec![0.0; 3]])),
                            MODE_WRONG_DIMENSION => (200, embed_response(MODEL, vec![vec![1.0, 2.0]])),
                            MODE_INVALID_NUMBER => {
                                (200, format!("{{\"model\":\"{MODEL}\",\"embeddings\":[[1e999,2,3]]}}"))
                            }
                            MODE_ERROR_BODY => (500, "PRIVATE_PROVIDER_BODY_CANARY".into()),
                            _ => unreachable!("known mock mode"),
                        }
                    }
                    _ => (404, "not found".into()),
                };
                let response = Response::from_string(body)
                    .with_status_code(StatusCode(status))
                    .with_header(Header::from_bytes("content-type", "application/json").unwrap());
                request.respond(response).expect("mock response");
            }
        });
        Self {
            url,
            state,
            thread: Some(thread),
        }
    }

    fn backend(&self) -> OllamaBackend {
        OllamaBackend::new(&self.url, MODEL).unwrap()
    }
}

impl Drop for MockOllama {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn embed_response(model: &str, embeddings: Vec<Vec<f32>>) -> String {
    serde_json::json!({
        "model": model,
        "embeddings": embeddings,
        "prompt_eval_count": 4,
        "total_duration": 9,
        "load_duration": 2
    })
    .to_string()
}

#[test]
fn identity_requires_exact_full_tag_and_immutable_evidence() {
    let server = MockOllama::start();
    let backend = server.backend();
    let identity = backend.builder_identity().unwrap();
    assert_eq!(identity.provider_family(), EmbeddingProviderFamily::Ollama);
    assert_eq!(identity.model_id(), MODEL);
    assert_eq!(identity.raw_dimension(), 3);
    assert_eq!(identity.effective_dimension(), 3);
    assert!(server
        .state
        .embed_bodies
        .lock()
        .unwrap()
        .iter()
        .all(|body| body.contains("\"truncate\":false")));

    let alias = OllamaBackend::new(&server.url, "nomic-embed-text").unwrap();
    assert!(alias.builder_identity().is_err());
    server.state.duplicate_tag.store(true, Ordering::Release);
    let duplicate = OllamaBackend::new(&server.url, MODEL).unwrap();
    assert!(duplicate.builder_identity().is_err());
    server.state.duplicate_tag.store(false, Ordering::Release);
    server.state.invalid_digest.store(true, Ordering::Release);
    let invalid_digest = OllamaBackend::new(&server.url, MODEL).unwrap();
    assert!(invalid_digest.builder_identity().is_err());
}

#[test]
fn same_name_with_different_digest_has_different_identity() {
    let first = MockOllama::start();
    let second = MockOllama::start();
    second.state.switch_digest_at.store(1, Ordering::SeqCst);
    let first_identity = first.backend().builder_identity().unwrap();
    let second_identity = second.backend().builder_identity().unwrap();
    assert_ne!(first_identity, second_identity);
}

#[test]
fn every_operation_discards_result_if_digest_drifts_after_call() {
    for operation in 0..5 {
        let server = MockOllama::start();
        let backend = server.backend();
        backend.builder_identity().unwrap();
        let embed_count_before = server.state.embed_bodies.lock().unwrap().len();
        server.state.switch_digest_at.store(5, Ordering::SeqCst);
        let result = match operation {
            0 => backend.embed("generic"),
            1 => backend.embed_batch(&["generic"]).map(|mut values| values.remove(0)),
            2 => backend.embed_query("query"),
            3 => backend.embed_document("document"),
            4 => backend
                .embed_document_batch(&["document"])
                .map(|mut values| values.remove(0)),
            _ => unreachable!(),
        };
        assert!(result.is_err(), "operation {operation} accepted a drifted result");
        assert_eq!(
            server.state.embed_bodies.lock().unwrap().len(),
            embed_count_before + 1,
            "operation {operation} did not reach the provider before drift"
        );
    }
}

#[test]
fn response_contract_rejects_model_count_shape_and_values() {
    for mode in [
        MODE_WRONG_MODEL,
        MODE_WRONG_COUNT,
        MODE_ZERO,
        MODE_WRONG_DIMENSION,
        MODE_INVALID_NUMBER,
    ] {
        let server = MockOllama::start();
        let backend = server.backend();
        backend.builder_identity().unwrap();
        server.state.mode.store(mode, Ordering::Release);
        assert!(
            backend.embed_document("contract canary").is_err(),
            "mode {mode} was accepted"
        );
    }
}

#[test]
fn provider_errors_do_not_expose_endpoint_body_or_input() {
    let server = MockOllama::start();
    let backend = server.backend();
    backend.builder_identity().unwrap();
    server.state.mode.store(MODE_ERROR_BODY, Ordering::Release);
    let error = backend
        .embed_document("PRIVATE_EMBEDDING_INPUT_CANARY")
        .unwrap_err()
        .to_string();
    assert!(!error.contains("PRIVATE_PROVIDER_BODY_CANARY"));
    assert!(!error.contains("PRIVATE_EMBEDDING_INPUT_CANARY"));
    assert!(!error.contains(&server.url));
    assert!(!error.contains(DIGEST_A));
}

#[test]
fn non_success_identity_endpoints_are_never_accepted() {
    let server = MockOllama::start();
    server.state.evidence_status.store(503, Ordering::Release);
    let backend = server.backend();
    assert!(matches!(
        backend.builder_identity(),
        Err(EmbeddingIdentityError::ProviderProbeFailed)
    ));
    assert!(server.state.embed_bodies.lock().unwrap().is_empty());
}
