//! Memory module unit tests
//!
//! Tests cover tier storage, runtime-only ephemeral eviction, and retrieval.

use plico::memory::{LayeredMemory, MemoryContent, MemoryEntry, MemoryTier};

fn make_entry(agent: &str, tier: MemoryTier, importance: u8, text: &str) -> MemoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: agent.to_string(),
        tenant_id: "default".to_string(),
        tier,
        content: MemoryContent::Text(text.to_string()),
        importance,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        tags: Vec::new(),
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
        memory_type: plico::memory::MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        deleted_at: None,
        superseded_by: None,
    }
}

#[test]
fn test_evict_low_importance_discarded() {
    // Bug fix test: entries with importance < 70 should be DISCARDED
    let mem = LayeredMemory::new();

    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "discard1"))
        .unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 10, "discard2"))
        .unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 0, "discard3"))
        .unwrap();

    let discarded = mem.evict_ephemeral("a1");

    // These should be discarded (returned)
    assert_eq!(
        discarded.len(),
        3,
        "Low-importance entries should be discarded and returned"
    );

    // Nothing should remain in Ephemeral
    assert_eq!(mem.get_tier("a1", MemoryTier::Ephemeral).len(), 0);
}

#[test]
fn test_evict_ephemeral_does_not_create_durable_entries() {
    let mem = LayeredMemory::new();

    mem.store(make_entry("a1", MemoryTier::Ephemeral, 90, "p1")).unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "d1")).unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 71, "p2")).unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 30, "d2")).unwrap();

    let discarded = mem.evict_ephemeral("a1");

    assert_eq!(discarded.len(), 4);
    assert!(mem.get_tier("a1", MemoryTier::Working).is_empty());

    // Ephemeral should be empty
    assert_eq!(mem.get_tier("a1", MemoryTier::Ephemeral).len(), 0);
}

#[test]
fn test_evict_nonexistent_agent() {
    let mem = LayeredMemory::new();
    let discarded = mem.evict_ephemeral("nonexistent");
    assert_eq!(discarded.len(), 0);
}

#[test]
fn test_get_by_tags() {
    let mem = LayeredMemory::new();

    let mut e1 = make_entry("a1", MemoryTier::Ephemeral, 50, "text1");
    e1.tags = vec!["meeting".to_string(), "project-x".to_string()];

    let mut e2 = make_entry("a1", MemoryTier::Ephemeral, 50, "text2");
    e2.tags = vec!["project-x".to_string()];

    let mut e3 = make_entry("a1", MemoryTier::Ephemeral, 50, "text3");
    e3.tags = vec!["meeting".to_string()];

    mem.store(e1).unwrap();
    mem.store(e2).unwrap();
    mem.store(e3).unwrap();

    let results = mem.get_by_tags("a1", MemoryTier::Ephemeral, &["meeting".to_string()]);
    assert_eq!(results.len(), 2);

    let results = mem.get_by_tags("a1", MemoryTier::Ephemeral, &["project-x".to_string()]);
    assert_eq!(results.len(), 2);

    let results = mem.get_by_tags("a1", MemoryTier::Ephemeral, &["nonexistent".to_string()]);
    assert_eq!(results.len(), 0);
}

#[test]
fn test_memory_entry_display() {
    use plico::memory::MemoryContent;

    let text = MemoryContent::Text("hello world".to_string());
    assert_eq!(text.display(), "hello world");

    let objref = MemoryContent::ObjectRef("abc123".to_string());
    assert_eq!(objref.display(), "[ObjectRef: abc123]");

    let structured = MemoryContent::Structured(serde_json::json!({"key": "value"}));
    let display = structured.display();
    assert!(display.contains("key"));
}

#[test]
fn test_memory_entry_access() {
    let mut entry = MemoryEntry::ephemeral("agent1", "original text");
    assert_eq!(entry.access_count, 0);

    entry.on_memory_access();
    assert_eq!(entry.access_count, 1);
    entry.on_memory_access();
    assert_eq!(entry.access_count, 2);

    // First access was before the on_memory_access() call, so now it's been accessed 3 times total
    // (0 initial + 2 explicit accesses = but initial is 0, so 2)
    assert_eq!(entry.access_count, 2);
}

#[test]
fn test_memory_entry_factory() {
    let entry = MemoryEntry::ephemeral("my_agent", "my content");
    assert_eq!(entry.agent_id, "my_agent");
    assert_eq!(entry.tier, MemoryTier::Ephemeral);
    assert_eq!(entry.importance, 50);
    assert!(matches!(entry.content, MemoryContent::Text(ref s) if s == "my content"));
}

#[test]
fn test_memory_tier_display() {
    assert_eq!(format!("{}", MemoryTier::Ephemeral), "ephemeral");
    assert_eq!(format!("{}", MemoryTier::Working), "working");
    assert_eq!(format!("{}", MemoryTier::LongTerm), "long_term");
    assert_eq!(format!("{}", MemoryTier::Procedural), "procedural");
}

#[test]
fn test_memory_tier_priority() {
    assert!(MemoryTier::Ephemeral.priority() > MemoryTier::Working.priority());
    assert!(MemoryTier::Working.priority() > MemoryTier::LongTerm.priority());
    assert!(MemoryTier::LongTerm.priority() > MemoryTier::Procedural.priority());
    // Ephemeral is highest priority (most urgent eviction candidate)
    assert_eq!(MemoryTier::Ephemeral.priority(), 3);
    assert_eq!(MemoryTier::Procedural.priority(), 0); // Never evicted
}

// ─── Runtime-only ephemeral eviction ────────────────────────────────────────

#[test]
fn test_eviction_low_importance() {
    // Ephemeral entries with importance < 70 should be discarded on eviction
    let mem = LayeredMemory::new();

    // These entries have importance < 70 so they should be discarded
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "discard1"))
        .unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 50, "discard2"))
        .unwrap();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 30, "discard3"))
        .unwrap();

    let discarded = mem.evict_ephemeral("a1");

    assert_eq!(discarded.len(), 3);

    let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
    assert_eq!(ephemeral.len(), 0);
}
