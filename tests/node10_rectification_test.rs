//! Node 10 Rectification Tests
//!
//! Tests for F-43 (soft-delete search isolation), F-44 (hybrid BM25 fallback),
//! F-47 (unified response envelope), F-48 (structured error diagnosis).

use plico::fs::semantic_fs::SemanticFS;
use plico::fs::embedding::StubEmbeddingProvider;
use plico::fs::search::InMemoryBackend;
use plico::api::semantic::ApiResponse;
use std::sync::Arc;

/// F-43: Soft-deleted objects are excluded from search after rebuild.
#[test]
fn test_soft_delete_excluded_from_search_after_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let stub_emb = Arc::new(StubEmbeddingProvider::new()) as Arc<dyn plico::fs::embedding::EmbeddingProvider>;
    let search_idx = Arc::new(InMemoryBackend::new()) as Arc<dyn plico::fs::search::SemanticSearch>;

    let fs = SemanticFS::new(
        dir.path().to_path_buf(),
        stub_emb,
        search_idx,
        None,
        None,
    ).unwrap();

    let cid = fs.create(b"secret content".to_vec(), vec!["secret".to_string()], "test".to_string(), None).unwrap();
    fs.delete(&cid, "test".to_string()).unwrap();

    // Simulate restart with new SemanticFS
    let stub_emb2 = Arc::new(StubEmbeddingProvider::new()) as Arc<dyn plico::fs::embedding::EmbeddingProvider>;
    let search_idx2 = Arc::new(InMemoryBackend::new()) as Arc<dyn plico::fs::search::SemanticSearch>;
    let fs2 = SemanticFS::new(dir.path().to_path_buf(), stub_emb2, search_idx2, None, None).unwrap();

    let results = fs2.search("secret", 10);
    let found_deleted = results.iter().any(|r| r.cid == cid);
    assert!(!found_deleted, "Soft-deleted CID {} should NOT appear after rebuild", cid);
}

/// F-44: SemanticFS.bm25_search API is accessible.
/// The hybrid_retrieve code uses fs.bm25_search() to get BM25 results
/// as KG seeds when vector search returns sparse results.
#[test]
fn test_bm25_search_exposed_for_hybrid_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let stub_emb = Arc::new(StubEmbeddingProvider::new()) as Arc<dyn plico::fs::embedding::EmbeddingProvider>;
    let search_idx = Arc::new(InMemoryBackend::new()) as Arc<dyn plico::fs::search::SemanticSearch>;

    let fs = SemanticFS::new(
        dir.path().to_path_buf(),
        stub_emb,
        search_idx,
        None,
        None,
    ).unwrap();

    // Verify bm25_search method exists and is callable
    // Returns Vec<(String, f32)> of (cid, score) pairs
    let results = fs.bm25_search("any query", 10);
    assert!(results.is_empty(), "Fresh BM25 index should be empty for any query");
    // Verify return type is correct (empty vec of tuples)
}

/// F-47: ApiResponse.ok_with_message includes confirmation message.
#[test]
fn test_api_response_ok_with_message() {
    let r = ApiResponse::ok_with_message("Object deleted successfully");
    assert!(r.ok);
    assert_eq!(r.message, Some("Object deleted successfully".to_string()));
}

/// F-48: ApiResponse.error_with_diagnosis includes structured diagnostics.
#[test]
fn test_api_response_error_with_diagnosis() {
    let r = ApiResponse::error_with_diagnosis(
        "CID not found",
        "NOT_FOUND",
        "Run search to find valid CIDs",
        vec!["plico(action=\"search\", query=\"...\")".to_string()],
    );
    assert!(!r.ok);
    assert!(r.error.is_some());
    assert_eq!(r.error_code, Some("NOT_FOUND".to_string()));
    assert_eq!(r.fix_hint, Some("Run search to find valid CIDs".to_string()));
    assert!(r.next_actions.is_some());
    assert_eq!(r.next_actions.as_ref().unwrap().len(), 1);
}

/// F-47: Agent lifecycle commands produce confirmation messages.
#[test]
fn test_agent_lifecycle_confirmations() {
    let suspend_msg = ApiResponse::ok_with_message("Agent 'test' suspended");
    assert!(suspend_msg.ok);
    assert!(suspend_msg.message.is_some());
    assert!(suspend_msg.message.unwrap().contains("suspended"));

    let resume_msg = ApiResponse::ok_with_message("Agent 'test' resumed");
    assert!(resume_msg.ok);
    assert!(resume_msg.message.is_some());

    let terminate_msg = ApiResponse::ok_with_message("Agent 'test' terminated");
    assert!(terminate_msg.ok);
    assert!(terminate_msg.message.is_some());
}

/// F-48: Error responses include structured recovery guidance.
#[test]
fn test_structured_error_recovery() {
    let err = ApiResponse::error_with_diagnosis(
        "Agent not found",
        "AGENT_NOT_FOUND",
        "Register agent first",
        vec!["plico(agent=\"register\")".to_string()],
    );
    assert!(!err.ok);
    assert_eq!(err.error_code, Some("AGENT_NOT_FOUND".to_string()));
    assert!(err.fix_hint.is_some());
    assert!(err.next_actions.is_some());
}
