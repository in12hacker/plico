//! Layered memory tests — extracted for module compliance.

#[allow(unused_imports)]
use crate::memory::{LayeredMemory, MemoryTier, MemoryEntry, MemoryScope};
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
        scope: MemoryScope::Private,
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
        scope: MemoryScope::Shared,
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
        scope: MemoryScope::Group("engineering".into()),
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
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Private,
    });
    mem.store(MemoryEntry {
        id: "shared-1".into(),
        agent_id: "agent-a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("shared knowledge".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Shared,
    });
    mem.store(MemoryEntry {
        id: "group-1".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("group data".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Group("team".into()),
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
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Group("engineering".into()),
    });
    mem.store(MemoryEntry {
        id: "mkt-1".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("marketing procedure".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Group("marketing".into()),
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
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Private,
    });
    mem.store(MemoryEntry {
        id: "other-private".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("b secret".into()),
        importance: 50, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Private,
    });
    mem.store(MemoryEntry {
        id: "common-shared".into(),
        agent_id: "agent-b".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("public knowledge".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Shared,
    });
    mem.store(MemoryEntry {
        id: "team-group".into(),
        agent_id: "agent-c".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("team procedure".into()),
        importance: 90, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None,
        scope: MemoryScope::Group("devs".into()),
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
        tags: vec![], embedding: None, ttl_ms: None, scope: MemoryScope::Private,
    });
    mem.store(MemoryEntry {
        id: "lt-1".into(), agent_id: agent.into(), tenant_id: "default".to_string(), tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("long-term note".into()),
        importance: 80, access_count: 0, last_accessed: now_ms(), created_at: now_ms(),
        tags: vec![], embedding: None, ttl_ms: None, scope: MemoryScope::Private,
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
