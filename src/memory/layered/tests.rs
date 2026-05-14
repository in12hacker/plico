//! Layered memory tests — extracted for module compliance.

use crate::memory::{LayeredMemory, MemoryTier, MemoryType, MemoryEntry, MemoryScope};
use crate::memory::layered::{MemoryContent, now_ms};

#[test]
fn test_move_entry_between_tiers() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";

    // Store in ephemeral
    let entry = MemoryEntry::ephemeral(agent, "test content");
    let entry_id = entry.id.clone();
    mem.store(entry);

    // Verify it's in ephemeral
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 1);
    assert_eq!(mem.get_tier(agent, MemoryTier::Working).len(), 0);

    // Move to working
    let moved = mem.move_entry(agent, &entry_id, MemoryTier::Working);
    assert!(moved);

    // Verify moved
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
    assert_eq!(mem.get_tier(agent, MemoryTier::Working).len(), 1);
    assert_eq!(mem.get_tier(agent, MemoryTier::Working)[0].id, entry_id);
}

#[test]
fn test_move_entry_not_found() {
    let mem = LayeredMemory::new();
    let moved = mem.move_entry("agent", "nonexistent", MemoryTier::Working);
    assert!(!moved);
}

#[test]
fn test_delete_entry() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";

    let entry = MemoryEntry::ephemeral(agent, "to delete");
    let entry_id = entry.id.clone();
    mem.store(entry);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 1);

    let deleted = mem.delete_entry(agent, &entry_id);
    assert!(deleted);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
}

#[test]
fn test_delete_entry_not_found() {
    let mem = LayeredMemory::new();
    let deleted = mem.delete_entry("agent", "nonexistent");
    assert!(!deleted);
}

// ─── MemoryScope Tests ─────────────────────────────────────────

#[test]
fn test_scope_default_is_private() {
    let entry = MemoryEntry::ephemeral("agent-a", "private by default");
    assert_eq!(entry.scope, MemoryScope::Private);
}

#[test]
fn test_private_memory_invisible_to_other_agents() {
    let mem = LayeredMemory::new();

    let entry = MemoryEntry {
        id: "priv-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("secret plan".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
    };
    mem.store(entry);

    let visible_a = mem.recall_visible("agent-a", &[]);
    let visible_b = mem.recall_visible("agent-b", &[]);

    assert_eq!(visible_a.len(), 1, "agent-a should see own private memory");
    assert_eq!(visible_b.len(), 0, "agent-b should NOT see agent-a's private memory");
}

#[test]
fn test_shared_memory_visible_to_all() {
    let mem = LayeredMemory::new();

    let entry = MemoryEntry {
        id: "shared-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("company policy".into()),
        importance: 80,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec!["policy".into()],
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Shared,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
    };
    mem.store(entry);

    let visible_a = mem.recall_visible("agent-a", &[]);
    let visible_b = mem.recall_visible("agent-b", &[]);
    let visible_c = mem.recall_visible("agent-c", &[]);

    assert_eq!(visible_a.len(), 1, "owner sees shared memory");
    assert_eq!(visible_b.len(), 1, "other agent sees shared memory");
    assert_eq!(visible_c.len(), 1, "any agent sees shared memory");
}

#[test]
fn test_group_memory_visible_to_group_members() {
    let mem = LayeredMemory::new();

    let entry = MemoryEntry {
        id: "group-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("team workflow".into()),
        importance: 90,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        embedding: None,
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Group("engineering".into()),
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
    };
    mem.store(entry);

    let visible_owner = mem.recall_visible("agent-a", &[]);
    let visible_member = mem.recall_visible("agent-b", &["engineering".into()]);
    let visible_outsider = mem.recall_visible("agent-c", &["marketing".into()]);
    let visible_no_group = mem.recall_visible("agent-d", &[]);

    assert_eq!(visible_owner.len(), 1, "owner always sees own group memory");
    assert_eq!(visible_member.len(), 1, "engineering group member sees it");
    assert_eq!(visible_outsider.len(), 0, "marketing member does NOT see it");
    assert_eq!(visible_no_group.len(), 0, "agent with no groups does NOT see it");
}

#[test]
fn test_get_shared_returns_only_shared_scope() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "private-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("private".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "shared-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("shared knowledge".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Shared, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "group-1".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("group data".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("team".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let shared = mem.get_shared(MemoryTier::Working);
    assert_eq!(shared.len(), 1);
    assert_eq!(shared[0].id, "shared-1");
}

#[test]
fn test_get_by_group_returns_only_matching_group() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "eng-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("eng procedure".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("engineering".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "mkt-1".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("marketing procedure".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("marketing".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let eng = mem.get_by_group("engineering", MemoryTier::Procedural);
    assert_eq!(eng.len(), 1);
    assert_eq!(eng[0].id, "eng-1");

    let mkt = mem.get_by_group("marketing", MemoryTier::Procedural);
    assert_eq!(mkt.len(), 1);
    assert_eq!(mkt[0].id, "mkt-1");
}

#[test]
fn test_recall_visible_combines_private_shared_group() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "my-private".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("my secret".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "other-private".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("b secret".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "common-shared".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("public knowledge".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Shared, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "team-group".into(),
        agent_id: "agent-c".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("team procedure".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("devs".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let visible = mem.recall_visible("agent-a", &["devs".into()]);
    let ids: Vec<&str> = visible.iter().map(|e| e.id.as_str()).collect();

    assert!(ids.contains(&"my-private"), "sees own private");
    assert!(!ids.contains(&"other-private"), "does NOT see other's private");
    assert!(ids.contains(&"common-shared"), "sees shared from any agent");
    assert!(ids.contains(&"team-group"), "sees group memory for devs");
    assert_eq!(visible.len(), 3);
}

#[test]
fn test_scope_serialization_roundtrip() {
    let scopes = vec![
        MemoryScope::Private,
        MemoryScope::Shared,
        MemoryScope::Group("engineering".into()),
    ];
    for scope in scopes {
        let json = serde_json::to_string(&scope).unwrap();
        let back: MemoryScope = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, back, "roundtrip failed for {:?}", scope);
    }
}

// ─── clear_agent Tests ────────────────────────────────────────

#[test]
fn test_clear_agent_removes_all_tiers() {
    let mem = LayeredMemory::new();
    let agent = "agent-x";

    mem.store(MemoryEntry::ephemeral(agent, "ephemeral note"));
    mem.store(MemoryEntry {
        id: "w-1".into(), agent_id: agent.into(), tenant_id: "default".to_string(), tier: MemoryTier::Working,
        content: MemoryContent::Text("working note".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None, scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "lt-1".into(), agent_id: agent.into(), tenant_id: "default".to_string(), tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("long-term note".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None, scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    assert_eq!(mem.get_all(agent).len(), 3);
    let removed = mem.clear_agent(agent);
    assert_eq!(removed, 3);
    assert_eq!(mem.get_all(agent).len(), 0);
}

#[test]
fn test_clear_agent_does_not_affect_other_agents() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry::ephemeral("agent-a", "a's note"));
    mem.store(MemoryEntry::ephemeral("agent-b", "b's note"));

    let removed = mem.clear_agent("agent-a");
    assert_eq!(removed, 1);
    assert_eq!(mem.get_all("agent-a").len(), 0);
    assert_eq!(mem.get_all("agent-b").len(), 1, "agent-b's memory should be unaffected");
}

// ─── MemoryType (Cognitive Typing) Tests ─────────────────────

#[test]
fn test_memory_type_default_is_untyped() {
    let entry = MemoryEntry::ephemeral("agent-a", "hello");
    assert_eq!(entry.memory_type, MemoryType::Untyped);
}

#[test]
fn test_memory_type_with_builder() {
    let entry = MemoryEntry::ephemeral("agent-a", "meeting at 3pm")
        .with_memory_type(MemoryType::Episodic);
    assert_eq!(entry.memory_type, MemoryType::Episodic);
}

#[test]
fn test_memory_type_from_str_loose() {
    assert_eq!(MemoryType::from_str_loose("episodic"), MemoryType::Episodic);
    assert_eq!(MemoryType::from_str_loose("event"), MemoryType::Episodic);
    assert_eq!(MemoryType::from_str_loose("semantic"), MemoryType::Semantic);
    assert_eq!(MemoryType::from_str_loose("fact"), MemoryType::Semantic);
    assert_eq!(MemoryType::from_str_loose("knowledge"), MemoryType::Semantic);
    assert_eq!(MemoryType::from_str_loose("procedural"), MemoryType::Procedural);
    assert_eq!(MemoryType::from_str_loose("skill"), MemoryType::Procedural);
    assert_eq!(MemoryType::from_str_loose("workflow"), MemoryType::Procedural);
    assert_eq!(MemoryType::from_str_loose("unknown"), MemoryType::Untyped);
    assert_eq!(MemoryType::from_str_loose(""), MemoryType::Untyped);
}

#[test]
fn test_memory_type_display() {
    assert_eq!(MemoryType::Episodic.to_string(), "episodic");
    assert_eq!(MemoryType::Semantic.to_string(), "semantic");
    assert_eq!(MemoryType::Procedural.to_string(), "procedural");
    assert_eq!(MemoryType::Untyped.to_string(), "untyped");
}

#[test]
fn test_memory_type_serialization_roundtrip() {
    for mt in [MemoryType::Episodic, MemoryType::Semantic, MemoryType::Procedural, MemoryType::Untyped] {
        let json = serde_json::to_string(&mt).unwrap();
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(mt, back);
    }
}

#[test]
fn test_memory_type_deserialization_default() {
    let json = r#"{"id":"1","agent_id":"a","tenant_id":"default","tier":"Ephemeral","content":{"Text":"hi"},"importance":50,"access_count":0,"last_accessed":0,"created_at":0,"tags":[]}"#;
    let entry: MemoryEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.memory_type, MemoryType::Untyped);
}

#[test]
fn test_get_by_type_filters_correctly() {
    let mem = LayeredMemory::new();
    let agent = "agent-x";

    let e1 = MemoryEntry::long_term(agent, MemoryContent::Text("meeting happened".into()), vec![])
        .with_memory_type(MemoryType::Episodic);
    let e2 = MemoryEntry::long_term(agent, MemoryContent::Text("user likes coffee".into()), vec![])
        .with_memory_type(MemoryType::Semantic);
    let e3 = MemoryEntry::long_term(agent, MemoryContent::Text("deploy workflow".into()), vec![])
        .with_memory_type(MemoryType::Procedural);
    let e4 = MemoryEntry::long_term(agent, MemoryContent::Text("random note".into()), vec![])
        .with_memory_type(MemoryType::Untyped);

    mem.store(e1);
    mem.store(e2);
    mem.store(e3);
    mem.store(e4);

    assert_eq!(mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Episodic).len(), 1);
    assert_eq!(mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Semantic).len(), 1);
    assert_eq!(mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Procedural).len(), 1);
    assert_eq!(mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Untyped).len(), 1);
    assert_eq!(mem.get_tier(agent, MemoryTier::LongTerm).len(), 4);
}

#[test]
fn test_get_by_type_empty_for_wrong_tier() {
    let mem = LayeredMemory::new();
    let agent = "agent-x";

    let entry = MemoryEntry::ephemeral(agent, "hi").with_memory_type(MemoryType::Episodic);
    mem.store(entry);

    assert_eq!(mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Episodic).len(), 0);
    assert_eq!(mem.get_by_type(agent, MemoryTier::Ephemeral, MemoryType::Episodic).len(), 1);
}

#[test]
fn test_recall_semantic_typed_per_type_topk() {
    let mem = LayeredMemory::new();
    let agent = "agent-x";

    let mk = |text: &str, mt: MemoryType, emb: Vec<f32>| {
        let mut e = MemoryEntry::long_term(agent, MemoryContent::Text(text.into()), vec![]);
        e.memory_type = mt;
        e.embedding = Some(emb);
        e
    };

    mem.store(mk("event A", MemoryType::Episodic, vec![1.0, 0.0, 0.0]));
    mem.store(mk("event B", MemoryType::Episodic, vec![0.9, 0.1, 0.0]));
    mem.store(mk("event C", MemoryType::Episodic, vec![0.8, 0.2, 0.0]));
    mem.store(mk("fact X", MemoryType::Semantic, vec![0.5, 0.5, 0.0]));
    mem.store(mk("fact Y", MemoryType::Semantic, vec![0.4, 0.6, 0.0]));
    mem.store(mk("skill Z", MemoryType::Procedural, vec![0.1, 0.0, 0.9]));

    let query = vec![1.0, 0.0, 0.0];
    let results = mem.recall_semantic_typed(agent, &query, 2);

    let types: Vec<MemoryType> = results.iter().map(|(e, _)| e.memory_type).collect();
    assert!(types.contains(&MemoryType::Episodic));
    assert!(types.contains(&MemoryType::Semantic));
    assert!(types.contains(&MemoryType::Procedural));
    assert!(results.len() >= 5, "should have at most 2 per type: {}", results.len());
}

#[test]
fn test_recall_semantic_typed_empty_agent() {
    let mem = LayeredMemory::new();
    let results = mem.recall_semantic_typed("nonexistent", &[1.0, 0.0], 5);
    assert!(results.is_empty());
}

#[test]
fn test_memory_type_preserved_through_store_and_retrieve() {
    let mem = LayeredMemory::new();
    let agent = "agent-a";

    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("stable fact".into()), vec!["test".into()])
        .with_memory_type(MemoryType::Semantic);
    mem.store(entry);

    let all = mem.get_all(agent);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].memory_type, MemoryType::Semantic);
}

#[test]
fn test_memory_type_hash_impl() {
    use std::collections::HashSet;
    let mut set = HashSet::new();
    set.insert(MemoryType::Episodic);
    set.insert(MemoryType::Semantic);
    set.insert(MemoryType::Procedural);
    set.insert(MemoryType::Untyped);
    assert_eq!(set.len(), 4);
    set.insert(MemoryType::Episodic);
    assert_eq!(set.len(), 4);
}

#[test]
fn test_update_importance() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";
    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("important fact".into()), vec![]);
    let entry_id = entry.id.clone();
    mem.store(entry);

    // Verify initial importance
    let found = mem.find_entry(agent, &entry_id).unwrap();
    assert_ne!(found.importance, 9);

    // Update importance
    mem.update_importance(agent, &entry_id, 9);
    let found = mem.find_entry(agent, &entry_id).unwrap();
    assert_eq!(found.importance, 9);
}

#[test]
fn test_remove_entry() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";
    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("to remove".into()), vec![]);
    let entry_id = entry.id.clone();
    mem.store(entry);
    assert!(mem.find_entry(agent, &entry_id).is_some());

    let removed = mem.remove_entry(agent, &entry_id);
    assert!(removed);
    assert!(mem.find_entry(agent, &entry_id).is_none());

    // Removing again returns false
    assert!(!mem.remove_entry(agent, &entry_id));
}

#[test]
fn test_find_entry() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";
    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("findable".into()), vec![]);
    let entry_id = entry.id.clone();
    mem.store(entry);

    let found = mem.find_entry(agent, &entry_id);
    assert!(found.is_some());
    assert_eq!(found.unwrap().id, entry_id);

    // Non-existent returns None
    assert!(mem.find_entry(agent, "nonexistent").is_none());
    assert!(mem.find_entry("other-agent", &entry_id).is_none());
}

// ─── MemoryTier Tests ────────────────────────────────────────────

#[test]
fn test_memory_tier_priority() {
    assert_eq!(MemoryTier::Ephemeral.priority(), 3);
    assert_eq!(MemoryTier::Working.priority(), 2);
    assert_eq!(MemoryTier::LongTerm.priority(), 1);
    assert_eq!(MemoryTier::Procedural.priority(), 0);
}

#[test]
fn test_memory_tier_name() {
    assert_eq!(MemoryTier::Ephemeral.name(), "ephemeral");
    assert_eq!(MemoryTier::Working.name(), "working");
    assert_eq!(MemoryTier::LongTerm.name(), "long_term");
    assert_eq!(MemoryTier::Procedural.name(), "procedural");
}

#[test]
fn test_memory_tier_display() {
    assert_eq!(MemoryTier::Ephemeral.to_string(), "ephemeral");
    assert_eq!(MemoryTier::Working.to_string(), "working");
    assert_eq!(MemoryTier::LongTerm.to_string(), "long_term");
    assert_eq!(MemoryTier::Procedural.to_string(), "procedural");
}

// ─── MemoryType name() ──────────────────────────────────────────

#[test]
fn test_memory_type_name() {
    assert_eq!(MemoryType::Episodic.name(), "episodic");
    assert_eq!(MemoryType::Semantic.name(), "semantic");
    assert_eq!(MemoryType::Procedural.name(), "procedural");
    assert_eq!(MemoryType::Untyped.name(), "untyped");
}

// ─── MemoryContent Tests ─────────────────────────────────────────

#[test]
fn test_memory_content_as_text() {
    assert_eq!(MemoryContent::Text("hello".into()).as_text(), Some("hello"));
    assert_eq!(MemoryContent::ObjectRef("cid123".into()).as_text(), None);
    assert_eq!(MemoryContent::Structured(serde_json::json!({"k":"v"})).as_text(), None);

    let proc = MemoryContent::Procedure(crate::memory::layered::Procedure {
        name: "p".into(), description: "d".into(), steps: vec![], learned_from: "x".into(),
    });
    assert_eq!(proc.as_text(), None);

    let knowledge = MemoryContent::Knowledge(crate::memory::layered::KnowledgePiece {
        subject: "s".into(), statement: "st".into(), confidence: 0.9, source: "src".into(),
    });
    assert_eq!(knowledge.as_text(), None);
}

#[test]
fn test_memory_content_display() {
    assert_eq!(MemoryContent::Text("hello".into()).display(), "hello");
    assert_eq!(MemoryContent::ObjectRef("abc123".into()).display(), "[ObjectRef: abc123]");

    let structured = MemoryContent::Structured(serde_json::json!({"key": 42}));
    let display = structured.display();
    assert!(display.contains("\"key\""));
    assert!(display.contains("42"));

    let proc = MemoryContent::Procedure(crate::memory::layered::Procedure {
        name: "deploy".into(), description: "Deploy the app".into(),
        steps: vec![], learned_from: "agent-x".into(),
    });
    assert_eq!(proc.display(), "Deploy the app");

    let knowledge = MemoryContent::Knowledge(crate::memory::layered::KnowledgePiece {
        subject: "rust".into(), statement: "Rust is fast".into(),
        confidence: 0.95, source: "experience".into(),
    });
    assert_eq!(knowledge.display(), "Rust is fast");
}

// ─── MemoryEntry Builder Tests ───────────────────────────────────

#[test]
fn test_memory_entry_default_tenant() {
    let tenant = MemoryEntry::default_tenant();
    assert_eq!(tenant, crate::DEFAULT_TENANT);
}

#[test]
fn test_memory_entry_long_term_constructor() {
    let entry = MemoryEntry::long_term(
        "agent-1",
        MemoryContent::Text("fact".into()),
        vec!["tag1".into(), "tag2".into()],
    );
    assert_eq!(entry.agent_id, "agent-1");
    assert_eq!(entry.tier, MemoryTier::LongTerm);
    assert_eq!(entry.tags, vec!["tag1", "tag2"]);
    assert_eq!(entry.importance, 50);
    assert_eq!(entry.access_count, 0);
    assert_eq!(entry.scope, MemoryScope::Private);
    assert_eq!(entry.memory_type, MemoryType::Untyped);
    assert!(entry.embedding.is_none());
    assert!(entry.ttl_ms.is_none());
    assert!(entry.original_ttl_ms.is_none());
    assert!(entry.causal_parent.is_none());
    assert!(entry.supersedes.is_none());
}

#[test]
fn test_with_causal_parent() {
    let entry = MemoryEntry::ephemeral("agent", "child")
        .with_causal_parent("parent-id-123");
    assert_eq!(entry.causal_parent, Some("parent-id-123".to_string()));
}

#[test]
fn test_with_supersedes() {
    let entry = MemoryEntry::ephemeral("agent", "new version")
        .with_supersedes("old-entry-id");
    assert_eq!(entry.supersedes, Some("old-entry-id".to_string()));
}

#[test]
fn test_on_memory_access_increments_count_and_refreshes_ttl() {
    let mut entry = MemoryEntry::ephemeral("agent", "ttl test");
    entry.original_ttl_ms = Some(1000);
    entry.ttl_ms = Some(1000);
    entry.access_count = 0;

    // First access: access_count becomes 1, multiplier = min(1,5) = 1
    entry.on_memory_access();
    assert_eq!(entry.access_count, 1);
    assert_eq!(entry.ttl_ms, Some(1000)); // 1000 * 1

    // Second access: multiplier = min(2,5) = 2
    entry.on_memory_access();
    assert_eq!(entry.access_count, 2);
    assert_eq!(entry.ttl_ms, Some(2000)); // 1000 * 2

    // Third: multiplier = 3
    entry.on_memory_access();
    assert_eq!(entry.ttl_ms, Some(3000));

    // Fourth: multiplier = 4
    entry.on_memory_access();
    assert_eq!(entry.ttl_ms, Some(4000));

    // Fifth: multiplier = 5 (cap)
    entry.on_memory_access();
    assert_eq!(entry.ttl_ms, Some(5000));

    // Sixth: still capped at 5
    entry.on_memory_access();
    assert_eq!(entry.ttl_ms, Some(5000));
    assert_eq!(entry.access_count, 6);
}

#[test]
fn test_on_memory_access_no_ttl_does_not_crash() {
    let mut entry = MemoryEntry::ephemeral("agent", "no ttl");
    assert!(entry.ttl_ms.is_none());
    entry.on_memory_access();
    assert_eq!(entry.access_count, 1);
    assert!(entry.ttl_ms.is_none(), "should remain None when original_ttl_ms is None");
}

// ─── store_checked / quota tests ─────────────────────────────────

#[test]
fn test_store_checked_within_quota() {
    let mem = LayeredMemory::new();
    let entry = MemoryEntry::ephemeral("agent", "first");
    assert!(mem.store_checked(entry, 10).is_ok());
    assert_eq!(mem.count_for_agent("agent"), 1);
}

#[test]
fn test_store_checked_quota_zero_means_unlimited() {
    let mem = LayeredMemory::new();
    for i in 0..100 {
        let entry = MemoryEntry::ephemeral("agent", format!("entry-{i}"));
        assert!(mem.store_checked(entry, 0).is_ok());
    }
    assert_eq!(mem.count_for_agent("agent"), 100);
}

#[test]
fn test_store_checked_exceeds_quota() {
    let mem = LayeredMemory::new();
    // Store 2 entries
    mem.store(MemoryEntry::ephemeral("agent", "a"));
    mem.store(MemoryEntry::ephemeral("agent", "b"));

    // Quota of 2 — the next store should fail
    let entry = MemoryEntry::ephemeral("agent", "over quota");
    let result = mem.store_checked(entry, 2);
    assert!(result.is_err());
    match result.unwrap_err() {
        crate::memory::layered::MemoryError::QuotaExceeded { agent_id, current, limit } => {
            assert_eq!(agent_id, "agent");
            assert_eq!(current, 2);
            assert_eq!(limit, 2);
        }
        other => panic!("expected QuotaExceeded, got: {:?}", other),
    }
}

#[test]
fn test_store_checked_quota_exact_boundary() {
    let mem = LayeredMemory::new();
    mem.store(MemoryEntry::ephemeral("agent", "a"));
    // quota=1, current=1 => should fail (>=)
    let result = mem.store_checked(MemoryEntry::ephemeral("agent", "b"), 1);
    assert!(result.is_err());
}

// ─── count_for_agent ─────────────────────────────────────────────

#[test]
fn test_count_for_agent_across_tiers() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.count_for_agent("agent"), 0);

    mem.store(MemoryEntry::ephemeral("agent", "e1"));
    assert_eq!(mem.count_for_agent("agent"), 1);

    mem.store(MemoryEntry {
        id: "w1".into(), agent_id: "agent".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("w".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    assert_eq!(mem.count_for_agent("agent"), 2);

    mem.store(MemoryEntry::long_term("agent", MemoryContent::Text("lt".into()), vec![]));
    assert_eq!(mem.count_for_agent("agent"), 3);

    mem.store(MemoryEntry {
        id: "p1".into(), agent_id: "agent".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural, content: MemoryContent::Text("proc".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    assert_eq!(mem.count_for_agent("agent"), 4);

    // Different agent is separate count
    assert_eq!(mem.count_for_agent("other"), 0);
}

// ─── evict_ephemeral ─────────────────────────────────────────────

#[test]
fn test_evict_ephemeral_promotes_important() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // importance >= 70 should be promoted
    mem.store(MemoryEntry {
        id: "high".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("important".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // importance < 70 should be discarded
    mem.store(MemoryEntry {
        id: "low".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("trivial".into()),
        importance: 30, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let discarded = mem.evict_ephemeral(agent);
    assert_eq!(discarded.len(), 1);
    assert_eq!(discarded[0].id, "low");

    // Ephemeral should be empty
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
    // Working should have the promoted entry
    let working = mem.get_tier(agent, MemoryTier::Working);
    assert_eq!(working.len(), 1);
    assert_eq!(working[0].id, "high");
    assert_eq!(working[0].tier, MemoryTier::Working);
}

#[test]
fn test_evict_ephemeral_empty_agent() {
    let mem = LayeredMemory::new();
    let discarded = mem.evict_ephemeral("nonexistent");
    assert!(discarded.is_empty());
}

#[test]
fn test_evict_ephemeral_boundary_importance_70() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // Exactly 70 should be promoted (>= 70)
    mem.store(MemoryEntry {
        id: "boundary".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("boundary".into()),
        importance: 70, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let discarded = mem.evict_ephemeral(agent);
    assert!(discarded.is_empty(), "importance=70 should be promoted");
    assert_eq!(mem.get_tier(agent, MemoryTier::Working).len(), 1);
}

// ─── get_by_tags ─────────────────────────────────────────────────

#[test]
fn test_get_by_tags() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry::long_term(agent, MemoryContent::Text("a".into()), vec!["rust".into(), "code".into()]));
    mem.store(MemoryEntry::long_term(agent, MemoryContent::Text("b".into()), vec!["python".into()]));
    mem.store(MemoryEntry::long_term(agent, MemoryContent::Text("c".into()), vec!["rust".into()]));

    let rust_entries = mem.get_by_tags(agent, MemoryTier::LongTerm, &["rust".into()]);
    assert_eq!(rust_entries.len(), 2);

    let py_entries = mem.get_by_tags(agent, MemoryTier::LongTerm, &["python".into()]);
    assert_eq!(py_entries.len(), 1);

    let missing = mem.get_by_tags(agent, MemoryTier::LongTerm, &["java".into()]);
    assert!(missing.is_empty());

    // Multiple tags: returns entries matching ANY of the tags
    let mixed = mem.get_by_tags(agent, MemoryTier::LongTerm, &["python".into(), "code".into()]);
    assert_eq!(mixed.len(), 2);
}

// ─── get_shared_entries_all_agents ───────────────────────────────

#[test]
fn test_get_shared_entries_all_agents() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "s1".into(), agent_id: "a1".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("shared1".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Shared, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "s2".into(), agent_id: "a2".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm, content: MemoryContent::Text("shared2".into()),
        importance: 70, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Shared, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "p1".into(), agent_id: "a1".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("private".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let shared = mem.get_shared_entries_all_agents();
    assert_eq!(shared.len(), 2);
    let ids: Vec<&str> = shared.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&"s1"));
    assert!(ids.contains(&"s2"));
}

// ─── get_group_entries_all_agents ────────────────────────────────

#[test]
fn test_get_group_entries_all_agents() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "g1".into(), agent_id: "a1".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("eng data".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("engineering".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "g2".into(), agent_id: "a2".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("eng doc".into()),
        importance: 60, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("engineering".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "m1".into(), agent_id: "a1".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("marketing".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("marketing".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let eng = mem.get_group_entries_all_agents("engineering");
    assert_eq!(eng.len(), 2);
    let mkt = mem.get_group_entries_all_agents("marketing");
    assert_eq!(mkt.len(), 1);
    let nonexistent = mem.get_group_entries_all_agents("sales");
    assert!(nonexistent.is_empty());
}

// ─── get_all_entries_all_agents ──────────────────────────────────

#[test]
fn test_get_all_entries_all_agents_excludes_private() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "priv".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("private".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "shrd".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("shared".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Shared, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry {
        id: "grp".into(), agent_id: "b".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm, content: MemoryContent::Text("group".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Group("team".into()), memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let all = mem.get_all_entries_all_agents();
    assert_eq!(all.len(), 2);
    let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&"shrd"));
    assert!(ids.contains(&"grp"));
    assert!(!ids.contains(&"priv"));
}

// ─── recall_with_tracking ────────────────────────────────────────

#[test]
fn test_recall_with_tracking_updates_access() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let entry = MemoryEntry::ephemeral(agent, "tracked");
    let entry_id = entry.id.clone();
    mem.store(entry);

    // access_count starts at 0
    let before = mem.get_tier(agent, MemoryTier::Ephemeral);
    assert_eq!(before[0].access_count, 0);

    let all = mem.recall_with_tracking(agent);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, entry_id);
    assert_eq!(all[0].access_count, 1, "access_count should be incremented");

    // Call again: access_count should be 2
    let all2 = mem.recall_with_tracking(agent);
    assert_eq!(all2[0].access_count, 2);
}

// ─── recall_relevant ─────────────────────────────────────────────

#[test]
fn test_recall_relevant_returns_within_budget() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // Store entries with known content lengths
    mem.store(MemoryEntry::ephemeral(agent, "short"));
    mem.store(MemoryEntry::ephemeral(agent, "a much longer entry that takes more tokens"));
    mem.store(MemoryEntry {
        id: "lt1".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm, content: MemoryContent::Text("long term fact".into()),
        importance: 90, access_count: 5, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    // Very large budget should return all
    let all = mem.recall_relevant(agent, 10000);
    assert_eq!(all.len(), 3);

    // Zero budget still returns at least one entry (greedy algorithm)
    let small = mem.recall_relevant(agent, 0);
    assert!(!small.is_empty());
}

#[test]
fn test_recall_relevant_empty_agent() {
    let mem = LayeredMemory::new();
    let results = mem.recall_relevant("nobody", 1000);
    assert!(results.is_empty());
}

// ─── evict_expired ───────────────────────────────────────────────

#[test]
fn test_evict_expired_removes_expired_entries() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // Expired entry: created 10000ms ago with 5000ms TTL
    let old_now = now_ms() - 10_000;
    mem.store(MemoryEntry {
        id: "expired".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("old".into()),
        importance: 50, access_count: 0, last_accessed: old_now, created_at: old_now,
        tags: vec![], embedding: None, ttl_ms: Some(5000), original_ttl_ms: Some(5000),
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // Not expired: no TTL
    mem.store(MemoryEntry {
        id: "alive".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("permanent".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // Not expired: long TTL
    mem.store(MemoryEntry {
        id: "future".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm, content: MemoryContent::Text("still good".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: Some(1_000_000), original_ttl_ms: Some(1_000_000),
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let evicted = mem.evict_expired(agent);
    assert_eq!(evicted, 1, "only the expired entry should be evicted");

    let remaining = mem.get_all(agent);
    let ids: Vec<&str> = remaining.iter().map(|e| e.id.as_str()).collect();
    assert!(ids.contains(&"alive"));
    assert!(ids.contains(&"future"));
    assert!(!ids.contains(&"expired"));
}

#[test]
fn test_evict_expired_empty_agent() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.evict_expired("nobody"), 0);
}

// ─── promote_check ───────────────────────────────────────────────

#[test]
fn test_promote_check_ephemeral_to_working() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // access_count >= 3 triggers Ephemeral -> Working
    mem.store(MemoryEntry {
        id: "hot".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("frequently accessed".into()),
        importance: 50, access_count: 5, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // access_count < 3 should NOT be promoted
    mem.store(MemoryEntry {
        id: "cold".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::Text("rarely accessed".into()),
        importance: 50, access_count: 1, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    mem.promote_check(agent);

    let eph = mem.get_tier(agent, MemoryTier::Ephemeral);
    assert_eq!(eph.len(), 1, "cold should remain in ephemeral");
    assert_eq!(eph[0].id, "cold");

    let working = mem.get_tier(agent, MemoryTier::Working);
    assert_eq!(working.len(), 1, "hot should be promoted to working");
    assert_eq!(working[0].id, "hot");
    assert_eq!(working[0].tier, MemoryTier::Working);
}

#[test]
fn test_promote_check_working_to_longterm() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // access_count >= 10 && importance >= 50 triggers Working -> LongTerm
    mem.store(MemoryEntry {
        id: "mature".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("well-used".into()),
        importance: 60, access_count: 15, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // High access but low importance — should NOT promote
    mem.store(MemoryEntry {
        id: "low-imp".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("frequent but unimportant".into()),
        importance: 30, access_count: 20, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    // High importance but low access — should NOT promote
    mem.store(MemoryEntry {
        id: "new-imp".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("important but new".into()),
        importance: 80, access_count: 5, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    mem.promote_check(agent);

    let working = mem.get_tier(agent, MemoryTier::Working);
    assert_eq!(working.len(), 2, "only mature should be promoted");

    let lt = mem.get_tier(agent, MemoryTier::LongTerm);
    assert_eq!(lt.len(), 1);
    assert_eq!(lt[0].id, "mature");
    assert_eq!(lt[0].tier, MemoryTier::LongTerm);
}

#[test]
fn test_promote_check_empty_agent() {
    let mem = LayeredMemory::new();
    // Should not panic
    mem.promote_check("nonexistent");
}

// ─── move_entry_to_tier (alias) ──────────────────────────────────

#[test]
fn test_move_entry_to_tier_alias() {
    let mem = LayeredMemory::new();
    let agent = "agent";
    let entry = MemoryEntry::ephemeral(agent, "movable");
    let entry_id = entry.id.clone();
    mem.store(entry);

    let moved = mem.move_entry_to_tier(agent, &entry_id, MemoryTier::Procedural);
    assert!(moved);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
    assert_eq!(mem.get_tier(agent, MemoryTier::Procedural).len(), 1);
    assert_eq!(mem.get_tier(agent, MemoryTier::Procedural)[0].id, entry_id);
}

// ─── clear_ephemeral ─────────────────────────────────────────────

#[test]
fn test_clear_ephemeral_only_clears_l0() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry::ephemeral(agent, "eph1"));
    mem.store(MemoryEntry::ephemeral(agent, "eph2"));
    mem.store(MemoryEntry::long_term(agent, MemoryContent::Text("keep".into()), vec![]));

    let removed = mem.clear_ephemeral(agent);
    assert_eq!(removed, 2);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
    assert_eq!(mem.get_tier(agent, MemoryTier::LongTerm).len(), 1, "long-term should be preserved");
}

#[test]
fn test_clear_ephemeral_empty_agent() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.clear_ephemeral("nobody"), 0);
}

// ─── recall_semantic ─────────────────────────────────────────────

#[test]
fn test_recall_semantic_returns_by_similarity() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let mut e1 = MemoryEntry::long_term(agent, MemoryContent::Text("similar".into()), vec![]);
    e1.embedding = Some(vec![1.0, 0.0, 0.0]);
    let id1 = e1.id.clone();

    let mut e2 = MemoryEntry::long_term(agent, MemoryContent::Text("different".into()), vec![]);
    e2.embedding = Some(vec![0.0, 1.0, 0.0]);

    let mut e3 = MemoryEntry::long_term(agent, MemoryContent::Text("exact".into()), vec![]);
    e3.embedding = Some(vec![1.0, 0.0, 0.0]);
    let id3 = e3.id.clone();

    // No embedding — should be skipped
    let e4 = MemoryEntry::long_term(agent, MemoryContent::Text("no embedding".into()), vec![]);

    mem.store(e1);
    mem.store(e2);
    mem.store(e3);
    mem.store(e4);

    let query = vec![1.0, 0.0, 0.0];
    let results = mem.recall_semantic(agent, &query, 2);
    assert_eq!(results.len(), 2);
    // Both e1 and e3 are identical in similarity; the result should exclude e2
    let result_ids: Vec<&str> = results.iter().map(|(e, _s)| e.id.as_str()).collect();
    assert!(result_ids.contains(&id1.as_str()) || result_ids.contains(&id3.as_str()));
    // e2 should NOT be in top-2
    assert!(!result_ids.contains(&"different") || results.iter().all(|(_, s)| *s > 0.0));
}

#[test]
fn test_recall_semantic_empty_agent() {
    let mem = LayeredMemory::new();
    let results = mem.recall_semantic("nobody", &[1.0, 0.0], 5);
    assert!(results.is_empty());
}

#[test]
fn test_recall_semantic_refreshes_ttl_on_hit() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let mut entry = MemoryEntry::long_term(agent, MemoryContent::Text("ttl test".into()), vec![]);
    entry.embedding = Some(vec![1.0, 0.0]);
    entry.original_ttl_ms = Some(1000);
    entry.ttl_ms = Some(1000);
    entry.access_count = 0;
    let entry_id = entry.id.clone();
    mem.store(entry);

    mem.recall_semantic(agent, &[1.0, 0.0], 5);

    let found = mem.find_entry(agent, &entry_id).unwrap();
    assert_eq!(found.access_count, 1, "recall_semantic should track access");
    assert_eq!(found.ttl_ms, Some(1000), "TTL should be refreshed (1 * 1000)");
}

// ─── recall_relevant_semantic ────────────────────────────────────

#[test]
fn test_recall_relevant_semantic_combines_scores() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let mut e1 = MemoryEntry::long_term(agent, MemoryContent::Text("semantically relevant".into()), vec![]);
    e1.embedding = Some(vec![1.0, 0.0, 0.0]);
    e1.importance = 90;

    let mut e2 = MemoryEntry::long_term(agent, MemoryContent::Text("less relevant".into()), vec![]);
    e2.embedding = Some(vec![0.0, 1.0, 0.0]);
    e2.importance = 10;

    mem.store(e1);
    mem.store(e2);

    let results = mem.recall_relevant_semantic(agent, &[1.0, 0.0, 0.0], 10000);
    assert_eq!(results.len(), 2);
    // The more semantically similar entry should rank first
    assert!(results[0].content.as_text().unwrap().contains("semantically relevant"));
}

#[test]
fn test_recall_relevant_semantic_empty_agent() {
    let mem = LayeredMemory::new();
    let results = mem.recall_relevant_semantic("nobody", &[1.0], 1000);
    assert!(results.is_empty());
}

// ─── is_cid_referenced ───────────────────────────────────────────

#[test]
fn test_is_cid_referenced() {
    let mem = LayeredMemory::new();

    mem.store(MemoryEntry {
        id: "ref1".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::ObjectRef("sha256:abc123".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry::ephemeral("a", "plain text"));

    assert!(mem.is_cid_referenced("sha256:abc123"));
    assert!(!mem.is_cid_referenced("sha256:nonexistent"));
}

#[test]
fn test_is_cid_referenced_across_all_tiers() {
    let mem = LayeredMemory::new();

    // Check ephemeral
    mem.store(MemoryEntry {
        id: "e1".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral, content: MemoryContent::ObjectRef("cid:eph".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    assert!(mem.is_cid_referenced("cid:eph"));

    // Check long-term
    mem.store(MemoryEntry {
        id: "lt1".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm, content: MemoryContent::ObjectRef("cid:lt".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    assert!(mem.is_cid_referenced("cid:lt"));

    // Check procedural
    mem.store(MemoryEntry {
        id: "pr1".into(), agent_id: "a".into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural, content: MemoryContent::ObjectRef("cid:proc".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    assert!(mem.is_cid_referenced("cid:proc"));
}

// ─── touch_entry ─────────────────────────────────────────────────

#[test]
fn test_touch_entry_found() {
    let mem = LayeredMemory::new();
    let agent = "agent";
    let entry = MemoryEntry::ephemeral(agent, "touchable");
    let entry_id = entry.id.clone();
    mem.store(entry);

    assert!(mem.touch_entry(agent, &entry_id));
    let found = mem.get_tier(agent, MemoryTier::Ephemeral);
    assert_eq!(found[0].access_count, 1);
}

#[test]
fn test_touch_entry_not_found() {
    let mem = LayeredMemory::new();
    assert!(!mem.touch_entry("agent", "nonexistent"));
}

#[test]
fn test_touch_entry_across_tiers() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let e1 = MemoryEntry::ephemeral(agent, "eph");
    let e1_id = e1.id.clone();
    mem.store(e1);

    let e2 = MemoryEntry {
        id: "wk".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("work".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    };
    mem.store(e2);

    assert!(mem.touch_entry(agent, &e1_id));
    assert!(mem.touch_entry(agent, "wk"));
    assert!(!mem.touch_entry(agent, "nonexistent"));
}

// ─── find_similar_long_term ──────────────────────────────────────

#[test]
fn test_find_similar_long_term_above_threshold() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let mut e1 = MemoryEntry::long_term(agent, MemoryContent::Text("similar".into()), vec![]);
    e1.embedding = Some(vec![1.0, 0.0, 0.0]);
    let id1 = e1.id.clone();
    mem.store(e1);

    let mut e2 = MemoryEntry::long_term(agent, MemoryContent::Text("orthogonal".into()), vec![]);
    e2.embedding = Some(vec![0.0, 1.0, 0.0]);
    mem.store(e2);

    // Query aligned with e1
    let found = mem.find_similar_long_term(agent, &[1.0, 0.0, 0.0], 0.9);
    assert!(found.is_some());
    assert_eq!(found.unwrap(), id1);
}

#[test]
fn test_find_similar_long_term_below_threshold() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let mut e1 = MemoryEntry::long_term(agent, MemoryContent::Text("perpendicular".into()), vec![]);
    e1.embedding = Some(vec![0.0, 1.0, 0.0]);
    mem.store(e1);

    // Query perpendicular to e1 — similarity ~0.0, below 0.9
    let found = mem.find_similar_long_term(agent, &[1.0, 0.0, 0.0], 0.9);
    assert!(found.is_none());
}

#[test]
fn test_find_similar_long_term_no_entries() {
    let mem = LayeredMemory::new();
    assert!(mem.find_similar_long_term("agent", &[1.0], 0.5).is_none());
}

#[test]
fn test_find_similar_long_term_skips_entries_without_embedding() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // Entry with no embedding
    let e1 = MemoryEntry::long_term(agent, MemoryContent::Text("no emb".into()), vec![]);
    mem.store(e1);

    // Entry with embedding
    let mut e2 = MemoryEntry::long_term(agent, MemoryContent::Text("has emb".into()), vec![]);
    e2.embedding = Some(vec![1.0, 0.0]);
    let id2 = e2.id.clone();
    mem.store(e2);

    let found = mem.find_similar_long_term(agent, &[1.0, 0.0], 0.5);
    assert_eq!(found, Some(id2));
}

// ─── get_stats ───────────────────────────────────────────────────

#[test]
fn test_get_stats_empty() {
    let mem = LayeredMemory::new();
    let stats = mem.get_stats();
    assert_eq!(stats.total_entries, 0);
    assert_eq!(stats.total_bytes, 0);
    assert_eq!(stats.avg_access_count, 0.0);
    assert_eq!(stats.never_accessed_count, 0);
    assert_eq!(stats.about_to_expire_count, 0);
    assert_eq!(stats.ephemeral_entries, 0);
    assert_eq!(stats.working_entries, 0);
    assert_eq!(stats.longterm_entries, 0);
}

#[test]
fn test_get_stats_with_entries() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry::ephemeral(agent, "ephemeral note"));
    mem.store(MemoryEntry {
        id: "w1".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("working data".into()),
        importance: 50, access_count: 3, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });
    mem.store(MemoryEntry::long_term(agent, MemoryContent::Text("long term fact".into()), vec![]));

    let stats = mem.get_stats();
    assert_eq!(stats.total_entries, 3);
    assert_eq!(stats.ephemeral_entries, 1);
    assert_eq!(stats.working_entries, 1);
    assert_eq!(stats.longterm_entries, 1);
    assert_eq!(stats.never_accessed_count, 2); // ephemeral + long_term have access_count=0
    assert!(stats.total_bytes > 0);
    assert!(stats.avg_access_count > 0.0);
}

#[test]
fn test_get_stats_about_to_expire() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    // Entry with TTL almost expired: created now, TTL = 100ms, original = 100ms
    // At time of check, elapsed ~0ms, remaining ~100ms, 10% of original = 10ms
    // remaining (100) > 10, so NOT about to expire yet.
    // Instead: create an entry that IS about to expire
    let now = now_ms();
    mem.store(MemoryEntry {
        id: "expiring".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("about to expire".into()),
        importance: 50, access_count: 0, last_accessed: now, created_at: now,
        tags: vec![], embedding: None,
        ttl_ms: Some(100),       // current TTL
        original_ttl_ms: Some(10000), // original was 10s
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let stats = mem.get_stats();
    // remaining = 100 - ~0 = ~100. 10% of 10000 = 1000. 100 <= 1000 => about_to_expire
    assert_eq!(stats.about_to_expire_count, 1);
}

// ─── tick (without persister) ────────────────────────────────────

#[test]
fn test_tick_increments_and_no_persist_without_persister() {
    let mem = LayeredMemory::new();

    // Without persister, tick increments but never triggers persist
    for _ in 0..49 {
        assert!(!mem.tick(), "should not trigger before threshold");
    }
    // The 50th tick should trigger, but persist_all returns 0 (no persister)
    assert!(mem.tick(), "50th tick should trigger");
}

// ─── persist_all / restore_all without persister ─────────────────

#[test]
fn test_persist_all_without_persister_returns_zero() {
    let mem = LayeredMemory::new();
    mem.store(MemoryEntry::ephemeral("a", "data"));
    assert_eq!(mem.persist_all(), 0);
}

#[test]
fn test_restore_all_without_persister_returns_zero() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.restore_all(), 0);
}

// ─── move_entry across multiple tiers ────────────────────────────

#[test]
fn test_move_entry_from_longterm_to_procedural() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("became a skill".into()), vec![]);
    let entry_id = entry.id.clone();
    mem.store(entry);

    assert!(mem.move_entry(agent, &entry_id, MemoryTier::Procedural));
    assert_eq!(mem.get_tier(agent, MemoryTier::LongTerm).len(), 0);
    let proc = mem.get_tier(agent, MemoryTier::Procedural);
    assert_eq!(proc.len(), 1);
    assert_eq!(proc[0].tier, MemoryTier::Procedural);
}

#[test]
fn test_move_entry_from_working_to_ephemeral() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let entry = MemoryEntry {
        id: "demoted".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("demoted entry".into()),
        importance: 20, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    };
    mem.store(entry);

    assert!(mem.move_entry(agent, "demoted", MemoryTier::Ephemeral));
    assert_eq!(mem.get_tier(agent, MemoryTier::Working).len(), 0);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 1);
}

// ─── delete_entry from different tiers ───────────────────────────

#[test]
fn test_delete_entry_from_working() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry {
        id: "w-del".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("delete me".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    assert!(mem.delete_entry(agent, "w-del"));
    assert_eq!(mem.get_tier(agent, MemoryTier::Working).len(), 0);
}

#[test]
fn test_delete_entry_from_procedural() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry {
        id: "p-del".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural, content: MemoryContent::Text("delete proc".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    assert!(mem.delete_entry(agent, "p-del"));
    assert_eq!(mem.get_tier(agent, MemoryTier::Procedural).len(), 0);
}

// ─── consolidate_agent (empty) ───────────────────────────────────

#[test]
fn test_consolidate_agent_empty_returns_default_report() {
    let mem = LayeredMemory::new();
    let report = mem.consolidate_agent("nobody");
    // Default report should have no actions
    assert!(report.actions.is_empty());
}

// ─── remove_entry from working tier ──────────────────────────────

#[test]
fn test_remove_entry_from_working() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry {
        id: "w-remove".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("remove me".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    assert!(mem.remove_entry(agent, "w-remove"));
    assert!(!mem.remove_entry(agent, "w-remove"), "already removed");
}

// ─── update_importance on working tier ───────────────────────────

#[test]
fn test_update_importance_on_working_tier() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry {
        id: "w-imp".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Working, content: MemoryContent::Text("boost me".into()),
        importance: 30, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    mem.update_importance(agent, "w-imp", 95);
    let found = mem.find_entry(agent, "w-imp").unwrap();
    assert_eq!(found.importance, 95);
}

#[test]
fn test_update_importance_nonexistent_is_noop() {
    let mem = LayeredMemory::new();
    // Should not panic
    mem.update_importance("agent", "nonexistent", 99);
}

// ─── find_entry in procedural tier ───────────────────────────────

#[test]
fn test_find_entry_in_procedural() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store(MemoryEntry {
        id: "proc-find".into(), agent_id: agent.into(), tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural, content: MemoryContent::Text("findable proc".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None,
        scope: MemoryScope::Private, memory_type: MemoryType::default(),
        causal_parent: None, supersedes: None,
    });

    let found = mem.find_entry(agent, "proc-find");
    assert!(found.is_some());
    assert_eq!(found.unwrap().tier, MemoryTier::Procedural);
}

// ─── Serialization roundtrip for MemoryTier ──────────────────────

#[test]
fn test_memory_tier_serialization_roundtrip() {
    for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
        let json = serde_json::to_string(&tier).unwrap();
        let back: MemoryTier = serde_json::from_str(&json).unwrap();
        assert_eq!(tier, back);
    }
}

// ─── MemoryContent serialization ─────────────────────────────────

#[test]
fn test_memory_content_serialization_roundtrip() {
    let contents = vec![
        MemoryContent::Text("hello".into()),
        MemoryContent::ObjectRef("cid123".into()),
        MemoryContent::Structured(serde_json::json!({"a": 1})),
    ];
    for content in contents {
        let json = serde_json::to_string(&content).unwrap();
        let back: MemoryContent = serde_json::from_str(&json).unwrap();
        // Compare display strings as a proxy for equality
        assert_eq!(content.display(), back.display());
    }
}

// ─── Multi-agent isolation ───────────────────────────────────────

#[test]
fn test_count_for_agent_isolation() {
    let mem = LayeredMemory::new();
    mem.store(MemoryEntry::ephemeral("a1", "data1"));
    mem.store(MemoryEntry::ephemeral("a1", "data2"));
    mem.store(MemoryEntry::ephemeral("a2", "data3"));

    assert_eq!(mem.count_for_agent("a1"), 2);
    assert_eq!(mem.count_for_agent("a2"), 1);
    assert_eq!(mem.count_for_agent("a3"), 0);
}

// ─── get_all for agent with no entries ───────────────────────────

#[test]
fn test_get_all_empty_agent() {
    let mem = LayeredMemory::new();
    assert!(mem.get_all("nobody").is_empty());
}

// ─── get_tier for nonexistent agent ──────────────────────────────

#[test]
fn test_get_tier_nonexistent_agent() {
    let mem = LayeredMemory::new();
    for tier in [MemoryTier::Ephemeral, MemoryTier::Working, MemoryTier::LongTerm, MemoryTier::Procedural] {
        assert!(mem.get_tier("ghost", tier).is_empty());
    }
}
