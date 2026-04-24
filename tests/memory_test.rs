//! Memory module unit tests
//!
//! Tests cover: tier storage, eviction, promotion, importance thresholds,
//! and the fix for the evict_ephemeral() return-value bug.

use plico::memory::{LayeredMemory, MemoryEntry, MemoryTier, MemoryContent};

fn make_entry(agent: &str, tier: MemoryTier, importance: u8, text: &str) -> MemoryEntry {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    MemoryEntry {
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
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    }
}

#[test]
fn test_store_and_retrieve_by_tier() {
    let mem = LayeredMemory::new();

    // Store in different tiers
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 50, "ephemeral1"));
    mem.store(make_entry("a1", MemoryTier::Working, 50, "working1"));
    mem.store(make_entry("a1", MemoryTier::LongTerm, 50, "longterm1"));
    mem.store(make_entry("a1", MemoryTier::Procedural, 50, "procedural1"));

    assert_eq!(mem.get_tier("a1", MemoryTier::Ephemeral).len(), 1);
    assert_eq!(mem.get_tier("a1", MemoryTier::Working).len(), 1);
    assert_eq!(mem.get_tier("a1", MemoryTier::LongTerm).len(), 1);
    assert_eq!(mem.get_tier("a1", MemoryTier::Procedural).len(), 1);

    // Other agent has nothing
    assert_eq!(mem.get_tier("other", MemoryTier::Ephemeral).len(), 0);
}

#[test]
fn test_get_all_tiers() {
    let mem = LayeredMemory::new();
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 50, "e1"));
    mem.store(make_entry("a1", MemoryTier::Working, 50, "w1"));
    mem.store(make_entry("a1", MemoryTier::LongTerm, 50, "l1"));

    let all = mem.get_all("a1");
    assert_eq!(all.len(), 3);
}

#[test]
fn test_evict_high_importance_promoted() {
    // Bug fix test: entries with importance >= 70 should be PROMOTED (saved in Working Memory)
    let mem = LayeredMemory::new();

    mem.store(make_entry("a1", MemoryTier::Ephemeral, 80, "important1"));
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 70, "threshold1"));

    let discarded = mem.evict_ephemeral("a1");

    // These should be discarded: they were promoted
    assert_eq!(discarded.len(), 0, "High-importance entries should NOT be discarded");

    // They should now be in Working Memory
    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 2);
    assert!(working.iter().all(|e| e.tier == MemoryTier::Working));
}

#[test]
fn test_evict_low_importance_discarded() {
    // Bug fix test: entries with importance < 70 should be DISCARDED
    let mem = LayeredMemory::new();

    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "discard1"));
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 10, "discard2"));
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 0, "discard3"));

    let discarded = mem.evict_ephemeral("a1");

    // These should be discarded (returned)
    assert_eq!(discarded.len(), 3, "Low-importance entries should be discarded and returned");

    // Nothing should remain in Ephemeral
    assert_eq!(mem.get_tier("a1", MemoryTier::Ephemeral).len(), 0);
}

#[test]
fn test_evict_mixed_importance() {
    // Mixed: some promoted, some discarded
    let mem = LayeredMemory::new();

    mem.store(make_entry("a1", MemoryTier::Ephemeral, 90, "p1")); // promote
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "d1")); // discard
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 71, "p2")); // promote
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 30, "d2")); // discard

    let discarded = mem.evict_ephemeral("a1");

    // 2 discarded (importance < 70)
    assert_eq!(discarded.len(), 2);

    // 2 promoted to Working Memory
    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 2);

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

    mem.store(e1);
    mem.store(e2);
    mem.store(e3);

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

#[test]
fn test_recall_semantic_with_embeddings() {
    let mem = LayeredMemory::new();

    let mut entry1 = make_entry("a", MemoryTier::LongTerm, 50, "Rust programming");
    entry1.embedding = Some(vec![1.0, 0.0, 0.0]);

    let mut entry2 = make_entry("a", MemoryTier::LongTerm, 50, "Python scripting");
    entry2.embedding = Some(vec![0.0, 1.0, 0.0]);

    let mut entry3 = make_entry("a", MemoryTier::LongTerm, 50, "Rust systems");
    entry3.embedding = Some(vec![0.9, 0.1, 0.0]);

    mem.store(entry1);
    mem.store(entry2);
    mem.store(entry3);

    let query = vec![1.0, 0.0, 0.0]; // most similar to "Rust programming"
    let results = mem.recall_semantic("a", &query, 2);

    assert_eq!(results.len(), 2);
    assert!(results[0].1 > results[1].1, "Results should be sorted by similarity");
    assert_eq!(results[0].0.content.display(), "Rust programming");
}

#[test]
fn test_recall_semantic_skips_entries_without_embeddings() {
    let mem = LayeredMemory::new();

    let entry_with = {
        let mut e = make_entry("a", MemoryTier::LongTerm, 50, "embedded entry");
        e.embedding = Some(vec![1.0, 0.0]);
        e
    };
    let entry_without = make_entry("a", MemoryTier::LongTerm, 50, "no embedding");

    mem.store(entry_with);
    mem.store(entry_without);

    let results = mem.recall_semantic("a", &[1.0, 0.0], 10);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.content.display(), "embedded entry");
}

// ─── Promotion Tests (v12.0 Memory Tier Automation) ─────────────────────────

#[test]
fn test_promotion_ephemeral_to_working() {
    // Ephemeral entry with access_count >= 3 should be promoted to Working
    let mem = LayeredMemory::new();

    // Create entry with access_count = 3 (meets threshold)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral,
        content: MemoryContent::Text("test content".to_string()),
        importance: 50,
        access_count: 3, // meets ephemeral_to_working_access threshold of 3
        last_accessed: now,
        created_at: now,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(entry);

    mem.promote_check("a1");

    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 1);
    assert_eq!(working[0].tier, MemoryTier::Working);

    let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
    assert_eq!(ephemeral.len(), 0);
}

#[test]
fn test_promotion_working_to_longterm() {
    // Working entry with access_count >= 10 && importance >= 50 should be promoted
    let mem = LayeredMemory::new();

    let entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("test content".to_string()),
        importance: 60,
        access_count: 10,
        last_accessed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(entry);

    mem.promote_check("a1");

    let longterm = mem.get_tier("a1", MemoryTier::LongTerm);
    assert_eq!(longterm.len(), 1);
    assert_eq!(longterm[0].tier, MemoryTier::LongTerm);

    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 0);
}

#[test]
fn test_no_promotion_below_threshold() {
    // Ephemeral entry with access_count < 3 should NOT be promoted
    let mem = LayeredMemory::new();

    // Create entry with access_count = 2 (below threshold of 3)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral,
        content: MemoryContent::Text("below threshold".to_string()),
        importance: 50,
        access_count: 2, // below ephemeral_to_working_access threshold of 3
        last_accessed: now,
        created_at: now,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(entry);

    mem.promote_check("a1");

    let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
    assert_eq!(ephemeral.len(), 1);

    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 0);
}

#[test]
fn test_no_promotion_working_low_importance() {
    // Working entry with access_count >= 10 but importance < 50 should NOT be promoted
    let mem = LayeredMemory::new();

    // Create entry with access_count = 10 but importance = 30 (below threshold of 50)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("low importance".to_string()),
        importance: 30, // below working_to_longterm_importance threshold of 50
        access_count: 10, // meets access threshold
        last_accessed: now,
        created_at: now,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(entry);

    mem.promote_check("a1");

    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 1);

    let longterm = mem.get_tier("a1", MemoryTier::LongTerm);
    assert_eq!(longterm.len(), 0);
}

#[test]
fn test_eviction_low_importance() {
    // Ephemeral entries with importance < 70 should be discarded on eviction
    let mem = LayeredMemory::new();

    // These entries have importance < 70 so they should be discarded
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 69, "discard1"));
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 50, "discard2"));
    mem.store(make_entry("a1", MemoryTier::Ephemeral, 30, "discard3"));

    let discarded = mem.evict_ephemeral("a1");

    assert_eq!(discarded.len(), 3);

    let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
    assert_eq!(ephemeral.len(), 0);
}

#[test]
fn test_move_entry_to_tier() {
    // Test the move_entry_to_tier method
    let mem = LayeredMemory::new();

    let entry = make_entry("a1", MemoryTier::Ephemeral, 50, "test entry");
    mem.store(entry);

    let entries = mem.get_tier("a1", MemoryTier::Ephemeral);
    let entry_id = entries[0].id.clone();

    let result = mem.move_entry_to_tier("a1", &entry_id, MemoryTier::Working);
    assert!(result);

    let working = mem.get_tier("a1", MemoryTier::Working);
    assert_eq!(working.len(), 1);
    assert_eq!(working[0].tier, MemoryTier::Working);

    let ephemeral = mem.get_tier("a1", MemoryTier::Ephemeral);
    assert_eq!(ephemeral.len(), 0);
}

#[test]
fn test_longterm_entries_not_promoted() {
    // LongTerm and Procedural entries should never be promoted
    let mem = LayeredMemory::new();

    let lt_entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("lt content".to_string()),
        importance: 100,
        access_count: 100,
        last_accessed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(lt_entry);

    let proc_entry = MemoryEntry {
        id: uuid::Uuid::new_v4().to_string(),
        agent_id: "a1".to_string(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("proc content".to_string()),
        importance: 100,
        access_count: 100,
        last_accessed: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64,
        tags: Vec::new(),
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: plico::memory::MemoryScope::Private,
    };
    mem.store(proc_entry);

    mem.promote_check("a1");

    // Both should remain in their tiers
    let longterm = mem.get_tier("a1", MemoryTier::LongTerm);
    assert_eq!(longterm.len(), 1);
    let procedural = mem.get_tier("a1", MemoryTier::Procedural);
    assert_eq!(procedural.len(), 1);
}
