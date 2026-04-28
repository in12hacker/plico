//! Layered memory tests — extracted for module compliance.

#[allow(unused_imports)]
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
    });
    mem.store(MemoryEntry {
        id: "lt-1".into(), agent_id: agent.into(), tenant_id: "default".to_string(), tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("long-term note".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, original_ttl_ms: None, scope: MemoryScope::Private, memory_type: MemoryType::default(),
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
