//! SemanticFS integration tests.

use std::sync::Arc;
use tempfile::TempDir;

use crate::fs::embedding::StubEmbeddingProvider;
use crate::fs::graph::PetgraphBackend;
use crate::fs::search::InMemoryBackend;
use crate::fs::semantic_fs::{SemanticFS, Query, EventType, EventRelation};
use crate::fs::context_loader::{ContextLayer, ContextLoader};

fn make_fs() -> (SemanticFS, tempfile::TempDir) {
    let dir = TempDir::new().unwrap();
    let fs = SemanticFS::new(
        dir.path().to_path_buf(),
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(InMemoryBackend::new()),
        None,
        None,
    ).unwrap();
    (fs, dir)
}

fn make_fs_with_kg(dir: &TempDir) -> SemanticFS {
    SemanticFS::new(
        dir.path().to_path_buf(),
        Arc::new(StubEmbeddingProvider::new()),
        Arc::new(InMemoryBackend::new()),
        None,
        Some(Arc::new(PetgraphBackend::new())),
    ).unwrap()
}

#[test]
fn test_create_and_get() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"Agent task output".to_vec(), vec!["result".to_string()], "a".to_string(), None).unwrap();
    let results = fs.read(&Query::ByCid(cid.clone())).unwrap();
    assert_eq!(results.len(), 1);
}

#[test]
fn test_list_tags() {
    let (fs, _dir) = make_fs();
    fs.create(b"data".to_vec(), vec!["tag-a".to_string()], "a".to_string(), None).unwrap();
    let tags = fs.list_tags();
    assert!(tags.contains(&"tag-a".to_string()));
}

#[test]
fn test_deduplication_by_content() {
    let (fs, _dir) = make_fs();
    let cid1 = fs.create(b"identical".to_vec(), vec!["test".to_string()], "a".to_string(), None).unwrap();
    let cid2 = fs.create(b"identical".to_vec(), vec!["test".to_string()], "a".to_string(), None).unwrap();
    assert_eq!(cid1, cid2);
}

#[test]
fn test_delete_is_logical() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"to delete".to_vec(), vec!["tmp".to_string()], "a".to_string(), None).unwrap();
    fs.delete(&cid, "a".to_string()).unwrap();
    assert!(fs.read(&Query::ByTags(vec!["tmp".to_string()])).unwrap().is_empty());
    assert_eq!(fs.list_deleted().len(), 1);
}

#[test]
fn test_restore_puts_back_in_tag_index() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"restored".to_vec(), vec!["todo".to_string()], "a".to_string(), None).unwrap();
    fs.delete(&cid, "a".to_string()).unwrap();
    fs.restore(&cid, "a".to_string()).unwrap();
    assert_eq!(fs.read(&Query::ByTags(vec!["todo".to_string()])).unwrap().len(), 1);
}

#[test]
fn test_audit_log_records_create() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"test".to_vec(), vec![], "audit-agent".to_string(), None).unwrap();
    let log = fs.audit_log();
    assert!(log.iter().any(|e| e.cid == cid && matches!(e.action, crate::fs::semantic_fs::AuditAction::Create)));
}

#[test]
fn test_update_generates_new_cid() {
    let (fs, _dir) = make_fs();
    let cid1 = fs.create(b"v1".to_vec(), vec!["ver".to_string()], "a".to_string(), None).unwrap();
    let cid2 = fs.update(&cid1, b"v2".to_vec(), None, "a".to_string()).unwrap();
    assert_ne!(cid1, cid2);
}

#[test]
fn test_search_query() {
    let (fs, _dir) = make_fs();
    fs.create(b"apple banana".to_vec(), vec!["fruit".to_string()], "a".to_string(), None).unwrap();
    let results = fs.search("fruit", 10);
    assert!(!results.is_empty());
}

#[test]
fn test_search_by_tags() {
    let (fs, _dir) = make_fs();
    fs.create(b"data".to_vec(), vec!["rust".to_string()], "a".to_string(), None).unwrap();
    fs.create(b"data".to_vec(), vec!["python".to_string()], "a".to_string(), None).unwrap();
    assert_eq!(fs.read(&Query::ByTags(vec!["rust".to_string()])).unwrap().len(), 1);
}

#[test]
fn test_update_with_new_tags() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"old".to_vec(), vec!["old-tag".to_string()], "a".to_string(), None).unwrap();
    fs.update(&cid, b"new".to_vec(), Some(vec!["new-tag".to_string()]), "a".to_string()).unwrap();
    assert!(fs.read(&Query::ByTags(vec!["old-tag".to_string()])).unwrap().is_empty());
    assert_eq!(fs.read(&Query::ByTags(vec!["new-tag".to_string()])).unwrap().len(), 1);
}

#[test]
fn test_context_layer_tokens() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"short content".to_vec(), vec![], "a".to_string(), None).unwrap();
    let ctx = fs.ctx_loader.load(&cid, ContextLayer::L0).unwrap();
    assert!(ctx.tokens_estimate > 0);
}

#[test]
fn test_recycle_bin_persists_across_restart() {
    let dir = TempDir::new().unwrap();
    let cid = {
        let fs = SemanticFS::new(dir.path().to_path_buf(), Arc::new(StubEmbeddingProvider::new()), Arc::new(InMemoryBackend::new()), None, None).unwrap();
        fs.create(b"persistent-delete".to_vec(), vec!["persist".to_string()], "a".to_string(), None).unwrap()
    };
    {
        let fs = SemanticFS::new(dir.path().to_path_buf(), Arc::new(StubEmbeddingProvider::new()), Arc::new(InMemoryBackend::new()), None, None).unwrap();
        fs.delete(&cid, "a".to_string()).unwrap();
    }
    let fs = SemanticFS::new(dir.path().to_path_buf(), Arc::new(StubEmbeddingProvider::new()), Arc::new(InMemoryBackend::new()), None, None).unwrap();
    assert_eq!(fs.list_deleted().len(), 1);
}

#[test]
fn test_restore_nonexistent_cid_returns_error() {
    let (fs, _dir) = make_fs();
    assert!(fs.restore("nonexistent-cid", "a".to_string()).is_err());
}

#[test]
fn test_list_deleted_after_delete() {
    let (fs, _dir) = make_fs();
    let cid = fs.create(b"delete me".to_vec(), vec![], "a".to_string(), None).unwrap();
    fs.delete(&cid, "a".to_string()).unwrap();
    assert_eq!(fs.list_deleted().len(), 1);
}

// ── Event tests ────────────────────────────────────────────────────────────

#[test]
fn event_meta_in_range_filters_correctly() {
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    let now = chrono::Utc::now().timestamp_millis() as u64;
    let id = fs.create_event("recent-meeting", EventType::Meeting, Some(now - 3_600_000), None, None, vec![], "a").unwrap();
    let events = fs.list_events(Some(now - 86_400_000), Some(now), &[], None).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].label, "recent-meeting");
    let events = fs.list_events(Some(now - 3_600_000_000), Some(now - 86_400_000), &[], None).unwrap();
    assert_eq!(events.len(), 0);
}

#[test]
fn event_type_serialize_roundtrip() {
    let meta = crate::fs::semantic_fs::EventMeta {
        label: "Standup".to_string(),
        event_type: EventType::Meeting,
        start_time: None,
        end_time: None,
        location: None,
        attendee_ids: vec![],
        related_cids: vec![],
    };
    let json = serde_json::to_value(&meta).unwrap();
    let back: crate::fs::semantic_fs::EventMeta = serde_json::from_value(json).unwrap();
    assert_eq!(back.label, "Standup");
    assert_eq!(back.event_type, EventType::Meeting);
}

#[test]
fn create_event_without_kg_returns_id() {
    let (fs, _dir) = make_fs();
    let id = fs.create_event("orphan-event", EventType::Meeting, None, None, None, vec![], "a").unwrap();
    assert!(id.starts_with("evt:"));
}

#[test]
fn create_and_list_event_with_kg() {
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    let id = fs.create_event("team-sync", EventType::Meeting, None, None, None, vec!["sync".to_string()], "a").unwrap();
    let events = fs.list_events(None, None, &[], None).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].label, "team-sync");
    assert_eq!(fs.list_events(None, None, &["sync".to_string()], None).unwrap().len(), 1);
    assert_eq!(fs.list_events(None, None, &["missing".to_string()], None).unwrap().len(), 0);
}

#[test]
fn list_events_by_time_range() {
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    let now = chrono::Utc::now().timestamp_millis() as u64;
    fs.create_event("today", EventType::Meeting, Some(now - 3_600_000), None, None, vec![], "a").unwrap();
    let events = fs.list_events_by_time("几天前", &[], None, &crate::temporal::RULE_BASED_RESOLVER).unwrap();
    assert_eq!(events.len(), 1);
}

#[test]
fn list_events_by_tag_intersection() {
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    fs.create_event("multi-tag", EventType::Meeting, None, None, None, vec!["a".to_string(), "b".to_string()], "a").unwrap();
    assert_eq!(fs.list_events(None, None, &["a".to_string()], None).unwrap().len(), 1);
    assert_eq!(fs.list_events(None, None, &["a".to_string(), "b".to_string()], None).unwrap().len(), 1);
    assert_eq!(fs.list_events(None, None, &["a".to_string(), "c".to_string()], None).unwrap().len(), 0);
}

#[test]
fn event_attach_updates_meta_and_edge() {
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    let event_id = fs.create_event("batch-indexing", EventType::Meeting, None, None, None, vec![], "a").unwrap();
    let person_id = "agent:worker-01";
    fs.event_attach(&event_id, person_id, EventRelation::Attendee, "a").unwrap();
    let events = fs.list_events(None, None, &[], None).unwrap();
    assert_eq!(events[0].attendee_count, 1);
    fs.event_attach(&event_id, "QmAABBCC", EventRelation::Document, "a").unwrap();
    let events = fs.list_events(None, None, &[], None).unwrap();
    assert_eq!(events[0].related_count, 1);
}

#[test]
fn list_events_returns_empty_without_kg() {
    let (fs, _dir) = make_fs();
    fs.create_event("test", EventType::Meeting, None, None, None, vec![], "a").unwrap();
    assert!(fs.list_events(None, None, &[], None).unwrap().is_empty());
}

#[test]
fn event_attach_fails_without_kg() {
    let (fs, _dir) = make_fs();
    let id = fs.create_event("test", EventType::Meeting, None, None, None, vec![], "a").unwrap();
    assert!(fs.event_attach(&id, "target", EventRelation::Attendee, "a").is_err());
}

#[test]
fn list_events_by_time_resolves_expression() {
    use crate::temporal::RULE_BASED_RESOLVER;
    let dir = TempDir::new().unwrap();
    let fs = make_fs_with_kg(&dir);
    let three_days_ago = (chrono::Local::now() - chrono::Duration::days(3))
        .date_naive()
        .and_hms_opt(0, 0, 0).unwrap()
        .and_utc()
        .timestamp_millis() as u64;
    fs.create_event("past-indexing-run", EventType::Meeting, Some(three_days_ago), None, None, vec!["indexing".to_string()], "a").unwrap();
    let events = fs.list_events_by_time("几天前", &["indexing".to_string()], None, &RULE_BASED_RESOLVER).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].label, "past-indexing-run");
}

#[test]
fn list_events_by_time_unknown_expression_returns_error() {
    let (fs, _dir) = make_fs();
    let resolver = crate::temporal::StubTemporalResolver;
    assert!(fs.list_events_by_time("当我还是个孩子的时候", &[], None, &resolver).is_err());
}

#[test]
fn context_loader_l2_returns_actual_content() {
    let (fs, _dir) = make_fs();
    let expected = b"The quick brown fox";
    let cid = fs.create(expected.to_vec(), vec!["test".to_string()], "agent".to_string(), None).unwrap();
    let ctx = fs.ctx_loader.load(&cid, ContextLayer::L2).unwrap();
    assert_eq!(ctx.layer, ContextLayer::L2);
    assert_eq!(ctx.content.as_bytes(), expected);
    assert!(ctx.tokens_estimate > 0);
}

#[test]
fn by_type_returns_matching_objects() {
    let (fs, _dir) = make_fs();
    let cid_text = fs.create(b"hello text".to_vec(), vec!["doc".to_string()], "a".to_string(), None).unwrap();
    let cid_bin = fs.create(vec![0x89, 0x50, 0x4E, 0x47], vec!["img".to_string()], "a".to_string(), None).unwrap();
    let results = fs.read(&Query::ByType("text".to_string())).unwrap();
    let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
    assert!(cids.contains(&cid_text.as_str()));
    assert!(!cids.contains(&cid_bin.as_str()));
}

#[test]
fn hybrid_query_with_tags_filters_correctly() {
    let (fs, _dir) = make_fs();
    let cid_a = fs.create(b"Rust programming notes".to_vec(), vec!["rust".to_string(), "notes".to_string()], "a".to_string(), None).unwrap();
    let _cid_b = fs.create(b"Python tutorial".to_vec(), vec!["python".to_string(), "notes".to_string()], "a".to_string(), None).unwrap();
    let results = fs.read(&Query::Hybrid { tags: vec!["rust".to_string()], semantic: None, content_type: None }).unwrap();
    let cids: Vec<_> = results.iter().map(|o| o.cid.as_str()).collect();
    assert!(cids.contains(&cid_a.as_str()));
    assert_eq!(cids.len(), 1);
}

#[test]
fn update_tag_index_reflects_new_cid() {
    let (fs, _dir) = make_fs();
    let cid1 = fs.create(b"version one".to_vec(), vec!["rust".to_string(), "plico".to_string()], "agent-test".to_string(), None).unwrap();
    let cid2 = fs.update(&cid1, b"version two".to_vec(), None, "agent-test".to_string()).unwrap();
    assert_ne!(cid1, cid2);
    let results = fs.read(&Query::ByTags(vec!["rust".to_string()])).unwrap();
    let cids: Vec<_> = results.iter().map(|r| r.cid.as_str()).collect();
    assert!(cids.contains(&cid2.as_str()), "new CID must be in tag index after update; got {:?}", cids);
    assert!(!cids.contains(&cid1.as_str()), "old CID must be removed from tag index after update; got {:?}", cids);
}
