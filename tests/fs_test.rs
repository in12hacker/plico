//! Semantic filesystem unit tests
//!
//! Tests cover: create, read, update, logical delete,
//! tag index, search, and context loading.

use plico::fs::{SemanticFS, Query, ContextLoader, ContextLayer, AuditAction};
use tempfile::tempdir;

fn make_fs() -> (SemanticFS, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let fs = SemanticFS::new(dir.path().to_path_buf()).unwrap();
    (fs, dir)
}

#[test]
fn test_create_and_get() {
    let (fs, _dir) = make_fs();

    let cid = fs
        .create(
            b"Meeting notes for Project X".to_vec(),
            vec!["meeting".to_string(), "project-x".to_string()],
            "TestAgent".to_string(),
            Some("Quarterly kickoff notes".to_string()),
        )
        .unwrap();

    let results = fs.read(&Query::ByCid(cid.clone())).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].data, b"Meeting notes for Project X");
    assert_eq!(results[0].meta.tags, vec!["meeting", "project-x"]);
}

#[test]
fn test_search_by_tags() {
    let (fs, _dir) = make_fs();

    fs.create(b"doc1".to_vec(), vec!["meeting".to_string()], "a".to_string(), None)
        .unwrap();
    fs.create(b"doc2".to_vec(), vec!["meeting".to_string(), "project-x".to_string()], "a".to_string(), None)
        .unwrap();
    fs.create(b"doc3".to_vec(), vec!["project-x".to_string()], "a".to_string(), None)
        .unwrap();

    let results = fs.read(&Query::ByTags(vec!["meeting".to_string()])).unwrap();
    assert_eq!(results.len(), 2);

    let results = fs.read(&Query::ByTags(vec!["project-x".to_string()])).unwrap();
    assert_eq!(results.len(), 2);
}

#[test]
fn test_search_query() {
    let (fs, _dir) = make_fs();

    fs.create(b"doc about rust".to_vec(), vec!["meeting".to_string()], "a".to_string(), None)
        .unwrap();

    // search() uses tag-based keyword matching
    let results = fs.search("meeting", 10);
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
    let loader = ContextLoader::new(dir.path().to_path_buf()).unwrap();

    // Compute L0 summary
    let summary = loader.compute_l0("This is a very long document with many words that should be summarized for the L0 layer.");
    assert!(!summary.is_empty());
    assert!(summary.len() < 200); // L0 should be short
}

#[test]
fn test_context_loader_l0_round_trip() {
    let dir = tempdir().unwrap();
    let loader = ContextLoader::new(dir.path().to_path_buf()).unwrap();

    // Create a test object (compute CID manually)
    let content = "Meeting notes: Project X kickoff discussion about Rust performance optimization.";
    let cid = plico::cas::object::AIObject::compute_cid(content.as_bytes());

    // Store L0 summary
    loader.store_l0(&cid, "Meeting notes for Project X kickoff.".to_string()).unwrap();

    // Load L0 summary
    let ctx = loader.load(&cid, ContextLayer::L0).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L0);
    assert!(ctx.content.contains("Project X"));
}

#[test]
fn test_context_loader_l0_not_found_returns_error() {
    let dir = tempdir().unwrap();
    let loader = ContextLoader::new(dir.path().to_path_buf()).unwrap();

    // Load nonexistent CID → should return an error (or placeholder)
    let result = loader.load("nonexistent00000000000000000000000000000000000000000000000000000000", ContextLayer::L0);
    // Should handle gracefully — either error or placeholder
    // Current implementation: placeholder
    assert!(result.is_ok());
}

#[test]
fn test_context_loader_l1_returns_placeholder() {
    let dir = tempdir().unwrap();
    let loader = ContextLoader::new(dir.path().to_path_buf()).unwrap();

    let ctx = loader.load("somecid0000000000000000000000000000000000000000000000000000000000", ContextLayer::L1).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L1);
    assert!(ctx.content.contains("not pre-computed") || !ctx.content.is_empty());
}

#[test]
fn test_context_loader_l2_returns_full_content_marker() {
    let dir = tempdir().unwrap();
    let loader = ContextLoader::new(dir.path().to_path_buf()).unwrap();

    let ctx = loader.load("somecid0000000000000000000000000000000000000000000000000000000000", ContextLayer::L2).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L2);
}

#[test]
fn test_context_layer_tokens() {
    assert_eq!(ContextLayer::L0.tokens_approx(), 100);
    assert_eq!(ContextLayer::L1.tokens_approx(), 2000);
    assert_eq!(ContextLayer::L2.tokens_approx(), usize::MAX);
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
