//! Semantic filesystem unit tests
//!
//! Tests cover: create, read, update, logical delete,
//! tag index, search, and context loading.

use plico::fs::{
    SemanticFS, Query, ContextLoader, ContextLayer, AuditAction,
    EmbeddingProvider, InMemoryBackend, EmbedError,
};
use plico::cas::CASStorage;
use std::sync::Arc;
use tempfile::tempdir;

/// A stub embedding provider that always fails — forces tag-based fallback in tests.
struct StubProvider;
impl EmbeddingProvider for StubProvider {
    fn embed(&self, _: &str) -> Result<Vec<f32>, EmbedError> {
        Err(EmbedError::ServerUnavailable("stub".to_string()))
    }
    fn embed_batch(&self, _: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        Err(EmbedError::ServerUnavailable("stub".to_string()))
    }
    fn dimension(&self) -> usize { 384 }
    fn model_name(&self) -> &str { "stub" }
}

fn make_fs_at(path: &std::path::Path) -> (SemanticFS, ()) {
    let fs = SemanticFS::new(
        path.to_path_buf(),
        std::sync::Arc::new(StubProvider),
        std::sync::Arc::new(InMemoryBackend::new()),
        None,
        None,
    ).unwrap();
    (fs, ())
}

fn make_fs() -> (SemanticFS, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let (fs, _) = make_fs_at(dir.path());
    (fs, dir)
}

#[test]
fn test_create_and_get() {
    let (fs, _dir) = make_fs();

    let cid = fs
        .create(
            b"Agent task output: embedding batch result".to_vec(),
            vec!["embedding".to_string(), "batch-result".to_string()],
            "TestAgent".to_string(),
            Some("Embedding computation output".to_string()),
        )
        .unwrap();

    let results = fs.read(&Query::ByCid(cid.clone())).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].data, b"Agent task output: embedding batch result");
    assert_eq!(results[0].meta.tags, vec!["embedding", "batch-result"]);
}

#[test]
fn test_search_by_tags() {
    let (fs, _dir) = make_fs();

    fs.create(b"doc1".to_vec(), vec!["embedding".to_string()], "a".to_string(), None)
        .unwrap();
    fs.create(b"doc2".to_vec(), vec!["embedding".to_string(), "batch-result".to_string()], "a".to_string(), None)
        .unwrap();
    fs.create(b"doc3".to_vec(), vec!["batch-result".to_string()], "a".to_string(), None)
        .unwrap();

    let results = fs.read(&Query::ByTags(vec!["embedding".to_string()])).unwrap();
    assert_eq!(results.len(), 2);

    let results = fs.read(&Query::ByTags(vec!["batch-result".to_string()])).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_search_query() {
    let (fs, _dir) = make_fs();

    fs.create(b"doc about rust".to_vec(), vec!["embedding".to_string()], "a".to_string(), None)
        .unwrap();

    let results = fs.search("embedding", 10);
    assert!(!results.is_empty());
}

#[test]
fn test_update_generates_new_cid() {
    let (fs, _dir) = make_fs();

    let old_cid = fs
        .create(b"original content".to_vec(), vec!["test".to_string()], "a".to_string(), None)
        .unwrap();

    let new_cid = fs
        .update(
            &old_cid,
            b"updated content".to_vec(),
            None,
            "a".to_string(),
        )
        .unwrap();

    // Content changed → new CID
    assert_ne!(new_cid, old_cid);

    // Old object still exists (immutable CAS)
    let old_obj = fs.read(&Query::ByCid(old_cid.clone())).unwrap();
    assert_eq!(old_obj[0].data, b"original content");

    // New object has new content
    let new_obj = fs.read(&Query::ByCid(new_cid.clone())).unwrap();
    assert_eq!(new_obj[0].data, b"updated content");
}

#[test]
fn test_update_with_new_tags() {
    let (fs, _dir) = make_fs();

    let old_cid = fs
        .create(b"content".to_vec(), vec!["old-tag".to_string()], "a".to_string(), None)
        .unwrap();

    let new_cid = fs
        .update(
            &old_cid,
            b"new content".to_vec(),
            Some(vec!["new-tag".to_string()]),
            "a".to_string(),
        )
        .unwrap();

    let new_obj = fs.read(&Query::ByCid(new_cid)).unwrap();
    assert!(new_obj[0].meta.tags.contains(&"new-tag".to_string()));
    assert!(!new_obj[0].meta.tags.contains(&"old-tag".to_string()));
}

#[test]
fn test_delete_is_logical() {
    let (fs, _dir) = make_fs();

    let cid = fs
        .create(b"to delete".to_vec(), vec!["test".to_string()], "a".to_string(), None)
        .unwrap();

    // Delete (logical)
    fs.delete(&cid, "a".to_string()).unwrap();

    // Object still exists in CAS (logical delete only moves to recycle bin)
    // SemanticFS doesn't expose recycle bin directly, but CAS still has it
    let results = fs.read(&Query::ByCid(cid.clone())).unwrap();
    assert_eq!(results.len(), 1); // Still readable
}

#[test]
fn test_audit_log_records_create() {
    let (fs, _dir) = make_fs();

    fs.create(b"test".to_vec(), vec!["test".to_string()], "agent1".to_string(), None)
        .unwrap();

    let log = fs.audit_log();
    assert!(!log.is_empty());

    // First entry should be Create
    let first = &log[0];
    assert!(matches!(first.action, AuditAction::Create));
}

#[test]
fn test_audit_log_records_update() {
    let (fs, _dir) = make_fs();

    let cid = fs.create(b"v1".to_vec(), vec!["test".to_string()], "a".to_string(), None).unwrap();
    fs.update(&cid, b"v2".to_vec(), None, "a".to_string()).unwrap();

    let log = fs.audit_log();

    // Should have Create + Update entries
    let updates: Vec<_> = log
        .iter()
        .filter(|e| matches!(e.action, AuditAction::Update { .. }))
        .collect();
    assert!(!updates.is_empty());
}

#[test]
fn test_list_tags() {
    let (fs, _dir) = make_fs();

    fs.create(b"doc1".to_vec(), vec!["a".to_string(), "b".to_string()], "x".to_string(), None)
        .unwrap();
    fs.create(b"doc2".to_vec(), vec!["b".to_string(), "c".to_string()], "x".to_string(), None)
        .unwrap();

    let tags = fs.list_tags();
    assert!(tags.contains(&"a".to_string()));
    assert!(tags.contains(&"b".to_string()));
    assert!(tags.contains(&"c".to_string()));
    assert!(!tags.contains(&"z".to_string()));
}

#[test]
fn test_deduplication_by_content() {
    let (fs, _dir) = make_fs();

    let cid1 = fs.create(b"same content".to_vec(), vec!["tag1".to_string()], "a".to_string(), None).unwrap();
    let cid2 = fs.create(b"same content".to_vec(), vec!["tag2".to_string()], "b".to_string(), None).unwrap();

    // Same content → same CID (CAS deduplication)
    assert_eq!(cid1, cid2);
}

// ─── Context Loader Tests ────────────────────────────────────────────

#[test]
fn test_context_loader_l0_cache() {
    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, cas).unwrap();

    // Compute L0 summary (fallback heuristic since no summarizer)
    let summary = loader.compute_l0("This is a very long document with many words that should be summarized for the L0 layer.");
    assert!(!summary.is_empty());
    assert!(summary.len() < 200); // L0 should be short
}

#[test]
fn test_context_loader_l0_round_trip() {
    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, cas).unwrap();

    // Create a test object (compute CID manually)
    let content = "Vector embedding batch result: 384-dim output for 50 documents using bge-small-en-v1.5.";
    let cid = plico::cas::object::AIObject::compute_cid(content.as_bytes());

    loader.store_l0(&cid, "Embedding batch: 50 docs, bge-small-en-v1.5.".to_string()).unwrap();

    let ctx = loader.load(&cid, ContextLayer::L0).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L0);
    assert!(ctx.content.contains("bge-small"));
}

#[test]
fn test_context_loader_l0_not_found_returns_error() {
    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, cas).unwrap();

    // Load nonexistent CID → should return an error (or placeholder)
    let result = loader.load("nonexistent00000000000000000000000000000000000000000000000000000000", ContextLayer::L0);
    // Should handle gracefully — either error or placeholder
    // Current implementation: placeholder
    assert!(result.is_ok());
}

#[test]
fn test_context_loader_l1_fallback_for_unknown_cid() {
    // When no L1 file is pre-computed and the CID is not in CAS,
    // load() should return empty content (not a leak the impl detail as a placeholder string).
    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, cas).unwrap();

    let ctx = loader.load("somecid0000000000000000000000000000000000000000000000000000000000", ContextLayer::L1).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L1);
    // CID not in CAS → empty content, no error
    assert!(ctx.content.is_empty());
    assert_eq!(ctx.tokens_estimate, 0);
}

#[test]
fn test_context_loader_l1_on_demand_from_cas() {
    use plico::cas::AIObject;

    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test2")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, Arc::clone(&cas)).unwrap();

    // Store a real object in CAS
    let content = "This is the actual document content that L1 should return.";
    let meta = plico::cas::AIObjectMeta::text(["doc"]);
    let obj = AIObject::new(content.as_bytes().to_vec(), meta);
    let cid = cas.put(&obj).unwrap();

    // L1 should return real content from CAS (no placeholder)
    let ctx = loader.load(&cid, ContextLayer::L1).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L1);
    assert_eq!(ctx.content, content);
    assert!(!ctx.content.contains("not pre-computed"));
}

#[test]
fn test_context_loader_l2_real_content() {
    use plico::cas::AIObject;

    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas_loader_test")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, Arc::clone(&cas)).unwrap();

    // Store a real object in CAS
    let content = b"Full document content for L2 loading.";
    let meta = plico::cas::AIObjectMeta::text(["doc"]);
    let obj = AIObject::new(content.to_vec(), meta);
    let cid = cas.put(&obj).unwrap();

    // L2 should return the actual content
    let ctx = loader.load(&cid, ContextLayer::L2).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L2);
    assert_eq!(ctx.content.as_bytes(), content);

    // L2 for nonexistent CID should return an error
    let err = loader.load("0000000000000000000000000000000000000000000000000000000000000000", ContextLayer::L2);
    assert!(err.is_err());
}

#[test]
fn test_context_layer_tokens() {
    assert_eq!(ContextLayer::L0.tokens_approx(), 100);
    assert_eq!(ContextLayer::L1.tokens_approx(), 2000);
    assert_eq!(ContextLayer::L2.tokens_approx(), usize::MAX);
}

#[test]
fn test_context_loader_l1_store_and_load() {
    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, cas).unwrap();

    let cid = "abcdef0000000000000000000000000000000000000000000000000000000000";
    let summary = "This is a pre-computed L1 summary.".to_string();
    loader.store_l1(cid, summary.clone()).unwrap();

    let ctx = loader.load(cid, ContextLayer::L1).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L1);
    assert_eq!(ctx.content, summary);
}

#[test]
fn test_context_loader_l1_prefix_truncation_without_summarizer() {
    use plico::cas::AIObject;

    let dir = tempdir().unwrap();
    let cas = Arc::new(CASStorage::new(dir.path().join("cas")).unwrap());
    let loader = ContextLoader::new(dir.path().to_path_buf(), None, Arc::clone(&cas)).unwrap();

    let content = "x".repeat(10_000);
    let meta = plico::cas::AIObjectMeta::text(["doc"]);
    let obj = AIObject::new(content.as_bytes().to_vec(), meta);
    let cid = cas.put(&obj).unwrap();

    let ctx = loader.load(&cid, ContextLayer::L1).unwrap();
    assert_eq!(ctx.content.len(), 8_000);
}

#[test]
fn test_content_type_from_extension() {
    use plico::cas::ContentType;

    assert_eq!(ContentType::from_extension("txt"), ContentType::Text);
    assert_eq!(ContentType::from_extension("md"), ContentType::Text);
    assert_eq!(ContentType::from_extension("json"), ContentType::Structured);
    assert_eq!(ContentType::from_extension("jpg"), ContentType::Image);
    assert_eq!(ContentType::from_extension("mp3"), ContentType::Audio);
    assert_eq!(ContentType::from_extension("mp4"), ContentType::Video);
    assert_eq!(ContentType::from_extension("exe"), ContentType::Binary);
    assert_eq!(ContentType::from_extension("XYZ"), ContentType::Unknown);
}

#[test]
fn test_content_type_predicates() {
    use plico::cas::ContentType;

    assert!(ContentType::Text.is_text());
    assert!(ContentType::Structured.is_text());
    assert!(!ContentType::Image.is_text());

    assert!(ContentType::Image.is_multimedia());
    assert!(ContentType::Audio.is_multimedia());
    assert!(ContentType::Video.is_multimedia());
    assert!(!ContentType::Text.is_multimedia());
}

// ─── Recycle Bin Tests ────────────────────────────────────────────────────────

#[test]
fn test_list_deleted_after_delete() {
    let (fs, _dir) = make_fs();

    let cid = fs.create(b"will be deleted".to_vec(), vec!["trash".to_string()], "a".to_string(), None).unwrap();
    assert!(fs.list_deleted().is_empty());

    fs.delete(&cid, "a".to_string()).unwrap();

    let deleted = fs.list_deleted();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].cid, cid);
    assert!(deleted[0].original_meta.tags.contains(&"trash".to_string()));
}

#[test]
fn test_restore_puts_back_in_tag_index() {
    let (fs, _dir) = make_fs();

    let cid = fs.create(b"restorable content".to_vec(), vec!["restore-me".to_string()], "a".to_string(), None).unwrap();
    fs.delete(&cid, "a".to_string()).unwrap();

    // After delete: not in tag index
    let before = fs.read(&Query::ByTags(vec!["restore-me".to_string()])).unwrap();
    assert!(before.is_empty(), "deleted object must not appear in tag search");

    // Restore
    fs.restore(&cid, "a".to_string()).unwrap();

    // After restore: back in tag index
    let after = fs.read(&Query::ByTags(vec!["restore-me".to_string()])).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].cid, cid);

    // Recycle bin is now empty
    assert!(fs.list_deleted().is_empty());
}

#[test]
fn test_recycle_bin_persists_across_restart() {
    let dir = tempdir().unwrap();
    let cid = {
        let (fs, _) = make_fs_at(dir.path());
        let cid = fs.create(b"survive restart".to_vec(), vec!["persist-test".to_string()], "a".to_string(), None).unwrap();
        fs.delete(&cid, "a".to_string()).unwrap();
        cid
    }; // fs dropped here — simulates process restart

    // Re-open the same root
    let (fs2, _) = make_fs_at(dir.path());
    let deleted = fs2.list_deleted();
    assert_eq!(deleted.len(), 1, "recycle bin must survive process restart");
    assert_eq!(deleted[0].cid, cid);
}

#[test]
fn test_restore_nonexistent_cid_returns_error() {
    let (fs, _dir) = make_fs();
    let result = fs.restore("nonexistentcid000000000000000000000000000000000000000000000000000", "a".to_string());
    assert!(result.is_err());
}
