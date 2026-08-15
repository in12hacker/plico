//! Layered memory tests — extracted for module compliance.

use crate::memory::layered::{now_ms, KnowledgePiece, MemoryContent};
use crate::memory::{
    CASCanonicalLedger, CanonicalLedger, DurableMemoryMutationError, LayeredMemory, MemoryEntry, MemoryScope,
    MemoryTier, MemoryType,
};

fn durable_working_entry(agent_id: &str, namespace: &str, entry_id: &str, content: &str) -> MemoryEntry {
    let revision_id = uuid::Uuid::parse_str(entry_id)
        .map(|id| id.to_string())
        .unwrap_or_else(|_| uuid::Uuid::new_v4().to_string());
    let mut entry = MemoryEntry::ephemeral(agent_id, content).with_root_revision_id(revision_id);
    entry.tenant_id = namespace.to_string();
    entry.tier = MemoryTier::Working;
    entry
}

#[derive(Debug)]
struct FailingCanonicalLedger;

impl crate::memory::CanonicalLedger for FailingCanonicalLedger {
    fn commit_expected(
        &self,
        _role_id: &str,
        _tier: MemoryTier,
        _expected_head: crate::memory::ExpectedHead,
        _revision: crate::memory::CanonicalRevision,
    ) -> Result<crate::memory::LedgerCommit, crate::memory::LedgerError> {
        Err(crate::memory::LedgerError::Cas("forced persistence failure".into()))
    }

    fn commit_roots(
        &self,
        _role_id: &str,
        _tier: MemoryTier,
        _revisions: Vec<crate::memory::CanonicalRevision>,
    ) -> Result<Vec<crate::memory::LedgerCommit>, crate::memory::LedgerError> {
        Err(crate::memory::LedgerError::Cas("forced persistence failure".into()))
    }

    fn rebuild_origin_role(
        &self,
        _role_id: &str,
    ) -> Result<Vec<(MemoryTier, Vec<crate::memory::CanonicalRevision>)>, crate::memory::LedgerError> {
        Ok(Vec::new())
    }

    fn list_origin_roles(&self) -> Result<Vec<String>, crate::memory::LedgerError> {
        Ok(Vec::new())
    }

    fn readable_active_revision_ids(
        &self,
        _role_id: &str,
    ) -> Result<Vec<crate::memory::MemoryRevisionId>, crate::memory::LedgerError> {
        Ok(Vec::new())
    }

    fn origin_for_revision(
        &self,
        _role_id: &str,
        _revision_id: &str,
        _write: bool,
    ) -> Result<Option<String>, crate::memory::LedgerError> {
        Ok(Some("role-a".to_string()))
    }

    fn flush(&self) -> Result<(), crate::memory::LedgerError> {
        Ok(())
    }
}

#[test]
fn durable_working_update_failure_never_publishes_candidate() {
    let memory = LayeredMemory::new();
    let original = durable_working_entry("role-a", "default", "old", "original");
    let original_id = original.id.clone();
    memory.store_test_entry(original);
    memory.set_ledger(std::sync::Arc::new(FailingCanonicalLedger));

    let result = memory.update_working_durable("role-a", "default", &original_id, "corrected".into());
    assert!(matches!(result, Err(DurableMemoryMutationError::Ledger(_))));

    let entries = memory.get_tier("role-a", MemoryTier::Working);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, original_id);
    assert_eq!(entries[0].content.as_text(), Some("original"));
    assert!(entries[0].superseded_by.is_none());
}

#[test]
fn post_exchange_sync_failure_poisoning_requires_restart_reconciliation() {
    let directory = tempfile::tempdir().unwrap();
    let vault = std::sync::Arc::new(
        crate::cas::PersonalVaultStorage::open(directory.path(), Some("memory_index.json")).unwrap(),
    );
    let ledger = std::sync::Arc::new(CASCanonicalLedger::new(vault).unwrap());
    ledger.inject_post_exchange_sync_failure_once();
    let memory = LayeredMemory::new();
    memory.set_ledger(ledger.clone());

    let first = durable_working_entry(
        "role-a",
        crate::DEFAULT_TENANT,
        &uuid::Uuid::new_v4().to_string(),
        "published outcome uncertain",
    );
    let result = memory.create_working_durable(first, 0);
    assert!(matches!(
        result,
        Err(DurableMemoryMutationError::Ledger(
            crate::memory::LedgerError::CommitIndeterminate
        ))
    ));
    assert!(memory.get_tier("role-a", MemoryTier::Working).is_empty());

    let second = durable_working_entry(
        "role-a",
        crate::DEFAULT_TENANT,
        &uuid::Uuid::new_v4().to_string(),
        "must not branch",
    );
    assert!(matches!(
        memory.create_working_durable(second, 0),
        Err(DurableMemoryMutationError::Ledger(
            crate::memory::LedgerError::WriterPoisoned
        ))
    ));
    drop(memory);
    drop(ledger);

    let vault = std::sync::Arc::new(
        crate::cas::PersonalVaultStorage::open(directory.path(), Some("memory_index.json")).unwrap(),
    );
    let restarted = CASCanonicalLedger::new(vault).unwrap();
    let restored = restarted.rebuild_origin_role("role-a").unwrap();
    let revisions: Vec<_> = restored.into_iter().flat_map(|(_, revisions)| revisions).collect();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].content.as_text(), Some("published outcome uncertain"));
}

#[test]
fn durable_working_delete_failure_never_publishes_tombstone() {
    let memory = LayeredMemory::new();
    let original = durable_working_entry("role-a", "default", "target", "keep me");
    let original_id = original.id.clone();
    memory.store_test_entry(original);
    memory.set_ledger(std::sync::Arc::new(FailingCanonicalLedger));

    let result = memory.delete_working_durable("role-a", "default", &original_id);
    assert!(matches!(result, Err(DurableMemoryMutationError::Ledger(_))));
    assert!(memory.find_entry("role-a", &original_id).unwrap().deleted_at.is_none());
}

#[test]
fn durable_working_mutation_requires_a_persister() {
    let memory = LayeredMemory::new();
    let original = durable_working_entry("role-a", "default", "target", "canonical");
    let original_id = original.id.clone();
    memory.store_test_entry(original);

    assert!(matches!(
        memory.update_working_durable("role-a", "default", &original_id, "corrected".into()),
        Err(DurableMemoryMutationError::PersistenceUnavailable)
    ));
    assert!(matches!(
        memory.delete_working_durable("role-a", "default", &original_id),
        Err(DurableMemoryMutationError::PersistenceUnavailable)
    ));
    let live = memory.find_entry("role-a", &original_id).unwrap();
    assert_eq!(live.content.as_text(), Some("canonical"));
    assert!(live.deleted_at.is_none());
    assert!(live.superseded_by.is_none());
}

#[test]
fn durable_working_update_and_delete_survive_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();

    let vault = std::sync::Arc::new(crate::cas::PersonalVaultStorage::open(&root, Some("memory_index.json")).unwrap());
    let ledger = crate::memory::CASCanonicalLedger::new(vault).unwrap();
    let memory = LayeredMemory::new();
    memory.set_ledger(std::sync::Arc::new(ledger));
    let original = memory
        .create_working_durable(durable_working_entry("role-a", "default", "old", "original"), 0)
        .unwrap();

    let corrected = memory
        .update_working_durable("role-a", "default", &original.id, "corrected".into())
        .unwrap();
    let corrected_id = corrected.id.clone();
    drop(memory);

    let vault = std::sync::Arc::new(crate::cas::PersonalVaultStorage::open(&root, Some("memory_index.json")).unwrap());
    let restarted_ledger = crate::memory::CASCanonicalLedger::new(vault).unwrap();
    let restarted = LayeredMemory::new();
    restarted.set_ledger(std::sync::Arc::new(restarted_ledger));
    restarted.restore_agent("role-a").unwrap();
    let old = restarted.find_entry("role-a", &original.id).unwrap();
    assert!(old.superseded_by.is_none());
    assert_eq!(
        restarted.find_entry("role-a", &corrected_id).unwrap().content.as_text(),
        Some("corrected")
    );

    let tombstone = restarted
        .delete_working_durable("role-a", "default", &corrected_id)
        .unwrap();
    drop(restarted);

    let vault = std::sync::Arc::new(crate::cas::PersonalVaultStorage::open(&root, Some("memory_index.json")).unwrap());
    let final_ledger = crate::memory::CASCanonicalLedger::new(vault).unwrap();
    let final_memory = LayeredMemory::new();
    final_memory.set_ledger(std::sync::Arc::new(final_ledger));
    final_memory.restore_agent("role-a").unwrap();
    assert!(final_memory
        .find_entry("role-a", &corrected_id)
        .unwrap()
        .deleted_at
        .is_none());
    assert!(final_memory
        .find_entry("role-a", &tombstone.id)
        .unwrap()
        .deleted_at
        .is_some());
    assert!(final_memory.get_active("role-a").is_empty());
}

#[test]
fn personal_owner_can_read_update_and_delete_origin_memory_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().to_path_buf();
    let memory = LayeredMemory::new();
    let vault = std::sync::Arc::new(crate::cas::PersonalVaultStorage::open(&root, Some("memory_index.json")).unwrap());
    memory.set_ledger(std::sync::Arc::new(
        crate::memory::CASCanonicalLedger::new(vault).unwrap(),
    ));
    let original = memory
        .create_working_durable(durable_working_entry("role-a", "default", "root", "original"), 0)
        .unwrap();

    assert!(memory
        .find_active_authorized("role-b", MemoryTier::Working, &original.id)
        .unwrap()
        .is_none());
    assert_eq!(
        memory
            .find_active_authorized(crate::PERSONAL_OWNER_ROLE_ID, MemoryTier::Working, &original.id)
            .unwrap()
            .unwrap()
            .agent_id,
        "role-a"
    );
    assert!(memory
        .recall_working_lexical_authorized("role-b", "default", "original", 5)
        .unwrap()
        .is_empty());
    assert_eq!(
        memory
            .recall_working_lexical_authorized(crate::PERSONAL_OWNER_ROLE_ID, "default", "original", 5)
            .unwrap()
            .len(),
        1
    );

    let updated = memory
        .update_working_durable(
            crate::PERSONAL_OWNER_ROLE_ID,
            "default",
            &original.id,
            "owner corrected".into(),
        )
        .unwrap();
    assert_eq!(updated.agent_id, "role-a");
    drop(memory);

    let restarted = LayeredMemory::new();
    let vault = std::sync::Arc::new(crate::cas::PersonalVaultStorage::open(&root, Some("memory_index.json")).unwrap());
    restarted.set_ledger(std::sync::Arc::new(
        crate::memory::CASCanonicalLedger::new(vault).unwrap(),
    ));
    restarted.restore_agent("role-a").unwrap();
    let deleted = restarted
        .delete_working_durable(crate::PERSONAL_OWNER_ROLE_ID, "default", &updated.id)
        .unwrap();
    assert_eq!(deleted.agent_id, "role-a");
    assert!(deleted.deleted_at.is_some());
}

#[test]
fn concurrent_updates_yield_one_commit_and_one_head_conflict() {
    let directory = tempfile::tempdir().unwrap();
    let memory = std::sync::Arc::new(LayeredMemory::new());
    let vault = std::sync::Arc::new(
        crate::cas::PersonalVaultStorage::open(directory.path(), Some("memory_index.json")).unwrap(),
    );
    memory.set_ledger(std::sync::Arc::new(
        crate::memory::CASCanonicalLedger::new(vault).unwrap(),
    ));
    let original = memory
        .create_working_durable(durable_working_entry("role-a", "default", "root", "original"), 0)
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut workers = Vec::new();
    for content in ["first", "second"] {
        let memory = std::sync::Arc::clone(&memory);
        let barrier = std::sync::Arc::clone(&barrier);
        let revision = original.id.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            memory.update_working_durable("role-a", "default", &revision, content.into())
        }));
    }
    barrier.wait();
    let results = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                Err(DurableMemoryMutationError::Ledger(
                    crate::memory::LedgerError::HeadConflict { .. }
                ))
            ))
            .count(),
        1
    );
    assert_eq!(memory.get_tier("role-a", MemoryTier::Working).len(), 2);
}

#[test]
fn repeated_delete_returns_the_same_tombstone_without_appending() {
    let directory = tempfile::tempdir().unwrap();
    let memory = LayeredMemory::new();
    let vault = std::sync::Arc::new(
        crate::cas::PersonalVaultStorage::open(directory.path(), Some("memory_index.json")).unwrap(),
    );
    memory.set_ledger(std::sync::Arc::new(
        crate::memory::CASCanonicalLedger::new(vault).unwrap(),
    ));
    let original = memory
        .create_working_durable(durable_working_entry("role-a", "default", "root", "original"), 0)
        .unwrap();
    let first = memory
        .delete_working_durable("role-a", "default", &original.id)
        .unwrap();
    let second = memory
        .delete_working_durable("role-a", "default", &original.id)
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(memory.get_tier("role-a", MemoryTier::Working).len(), 2);
}

// ─── MemoryScope Tests ─────────────────────────────────────────

#[test]
fn test_scope_default_is_private() {
    let entry = MemoryEntry::ephemeral("agent-a", "private by default");
    assert_eq!(entry.scope, MemoryScope::Private);
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

// ─── MemoryType (Cognitive Typing) Tests ─────────────────────

#[test]
fn test_memory_type_default_is_untyped() {
    let entry = MemoryEntry::ephemeral("agent-a", "hello");
    assert_eq!(entry.memory_type, MemoryType::Untyped);
}

#[test]
fn test_memory_type_with_builder() {
    let entry = MemoryEntry::ephemeral("agent-a", "meeting at 3pm").with_memory_type(MemoryType::Episodic);
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
    for mt in [
        MemoryType::Episodic,
        MemoryType::Semantic,
        MemoryType::Procedural,
        MemoryType::Untyped,
    ] {
        let json = serde_json::to_string(&mt).unwrap();
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(mt, back);
    }
}

#[test]
fn old_memory_schema_is_rejected_without_runtime_compatibility() {
    let json = r#"{"id":"1","agent_id":"a","tenant_id":"default","tier":"Ephemeral","content":{"Text":"hi"},"importance":50,"access_count":0,"last_accessed":0,"created_at":0,"tags":[]}"#;
    assert!(serde_json::from_str::<MemoryEntry>(json).is_err());
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

    mem.store_test_entry(e1);
    mem.store_test_entry(e2);
    mem.store_test_entry(e3);
    mem.store_test_entry(e4);

    assert_eq!(
        mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Episodic).len(),
        1
    );
    assert_eq!(
        mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Semantic).len(),
        1
    );
    assert_eq!(
        mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Procedural)
            .len(),
        1
    );
    assert_eq!(
        mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Untyped).len(),
        1
    );
    assert_eq!(mem.get_tier(agent, MemoryTier::LongTerm).len(), 4);
}

#[test]
fn test_get_by_type_empty_for_wrong_tier() {
    let mem = LayeredMemory::new();
    let agent = "agent-x";

    let entry = MemoryEntry::ephemeral(agent, "hi").with_memory_type(MemoryType::Episodic);
    mem.store_test_entry(entry);

    assert_eq!(
        mem.get_by_type(agent, MemoryTier::LongTerm, MemoryType::Episodic).len(),
        0
    );
    assert_eq!(
        mem.get_by_type(agent, MemoryTier::Ephemeral, MemoryType::Episodic)
            .len(),
        1
    );
}

#[test]
fn test_memory_type_preserved_through_store_and_retrieve() {
    let mem = LayeredMemory::new();
    let agent = "agent-a";

    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("stable fact".into()), vec!["test".into()])
        .with_memory_type(MemoryType::Semantic);
    mem.store_test_entry(entry);

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
fn test_find_entry() {
    let mem = LayeredMemory::new();
    let agent = "test-agent";
    let entry = MemoryEntry::long_term(agent, MemoryContent::Text("findable".into()), vec![]);
    let entry_id = entry.id.clone();
    mem.store_test_entry(entry);

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
        name: "p".into(),
        description: "d".into(),
        steps: vec![],
        learned_from: "x".into(),
    });
    assert_eq!(proc.as_text(), None);

    let knowledge = MemoryContent::Knowledge(crate::memory::layered::KnowledgePiece {
        subject: "s".into(),
        statement: "st".into(),
        confidence: 0.9,
        source: "src".into(),
    });
    assert_eq!(knowledge.as_text(), None);
}

#[test]
fn test_memory_content_display() {
    assert_eq!(MemoryContent::Text("hello".into()).display(), "hello");
    assert_eq!(
        MemoryContent::ObjectRef("abc123".into()).display(),
        "[ObjectRef: abc123]"
    );

    let structured = MemoryContent::Structured(serde_json::json!({"key": 42}));
    let display = structured.display();
    assert!(display.contains("\"key\""));
    assert!(display.contains("42"));

    let proc = MemoryContent::Procedure(crate::memory::layered::Procedure {
        name: "deploy".into(),
        description: "Deploy the app".into(),
        steps: vec![],
        learned_from: "agent-x".into(),
    });
    assert_eq!(proc.display(), "Deploy the app");

    let knowledge = MemoryContent::Knowledge(crate::memory::layered::KnowledgePiece {
        subject: "rust".into(),
        statement: "Rust is fast".into(),
        confidence: 0.95,
        source: "experience".into(),
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
    assert!(entry.ttl_ms.is_none());
    assert!(entry.original_ttl_ms.is_none());
    assert!(entry.causal_parent.is_none());
    assert!(entry.supersedes.is_none());
}

#[test]
fn test_with_causal_parent() {
    let entry = MemoryEntry::ephemeral("agent", "child").with_causal_parent("parent-id-123");
    assert_eq!(entry.causal_parent, Some("parent-id-123".to_string()));
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
    assert!(
        entry.ttl_ms.is_none(),
        "should remain None when original_ttl_ms is None"
    );
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
    mem.store_test_entry(MemoryEntry::ephemeral("agent", "a"));
    mem.store_test_entry(MemoryEntry::ephemeral("agent", "b"));

    // Quota of 2 — the next store should fail
    let entry = MemoryEntry::ephemeral("agent", "over quota");
    let result = mem.store_checked(entry, 2);
    assert!(result.is_err());
    match result.unwrap_err() {
        crate::memory::layered::MemoryError::QuotaExceeded {
            agent_id,
            current,
            limit,
        } => {
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
    mem.store_test_entry(MemoryEntry::ephemeral("agent", "a"));
    // quota=1, current=1 => should fail (>=)
    let result = mem.store_checked(MemoryEntry::ephemeral("agent", "b"), 1);
    assert!(result.is_err());
}

// ─── count_for_agent ─────────────────────────────────────────────

#[test]
fn test_count_for_agent_across_tiers() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.count_for_agent("agent"), 0);

    mem.store_test_entry(MemoryEntry::ephemeral("agent", "e1"));
    assert_eq!(mem.count_for_agent("agent"), 1);

    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "w1".into(),
        agent_id: "agent".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("w".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    assert_eq!(mem.count_for_agent("agent"), 2);

    mem.store_test_entry(MemoryEntry::long_term(
        "agent",
        MemoryContent::Text("lt".into()),
        vec![],
    ));
    assert_eq!(mem.count_for_agent("agent"), 3);

    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "p1".into(),
        agent_id: "agent".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("proc".into()),
        importance: 90,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    assert_eq!(mem.count_for_agent("agent"), 4);

    // Different agent is separate count
    assert_eq!(mem.count_for_agent("other"), 0);
}

// ─── evict_ephemeral ─────────────────────────────────────────────

#[test]
fn test_evict_ephemeral_empty_agent() {
    let mem = LayeredMemory::new();
    let discarded = mem.evict_ephemeral("nonexistent");
    assert!(discarded.is_empty());
}

#[test]
fn test_evict_ephemeral_never_promotes_to_durable_tier() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "boundary".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral,
        content: MemoryContent::Text("boundary".into()),
        importance: 70,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });

    let discarded = mem.evict_ephemeral(agent);
    assert_eq!(discarded.len(), 1);
    assert!(mem.get_tier(agent, MemoryTier::Working).is_empty());
}

// ─── get_by_tags ─────────────────────────────────────────────────

#[test]
fn test_get_by_tags() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store_test_entry(MemoryEntry::long_term(
        agent,
        MemoryContent::Text("a".into()),
        vec!["rust".into(), "code".into()],
    ));
    mem.store_test_entry(MemoryEntry::long_term(
        agent,
        MemoryContent::Text("b".into()),
        vec!["python".into()],
    ));
    mem.store_test_entry(MemoryEntry::long_term(
        agent,
        MemoryContent::Text("c".into()),
        vec!["rust".into()],
    ));

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

// ─── recall_with_tracking ────────────────────────────────────────

#[test]
fn test_recall_with_tracking_updates_access() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    let entry = MemoryEntry::ephemeral(agent, "tracked");
    let entry_id = entry.id.clone();
    mem.store_test_entry(entry);

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
    mem.store_test_entry(MemoryEntry::ephemeral(agent, "short"));
    mem.store_test_entry(MemoryEntry::ephemeral(
        agent,
        "a much longer entry that takes more tokens",
    ));
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "lt1".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::Text("long term fact".into()),
        importance: 90,
        access_count: 5,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
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

// ─── clear_ephemeral ─────────────────────────────────────────────

#[test]
fn test_clear_ephemeral_only_clears_l0() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store_test_entry(MemoryEntry::ephemeral(agent, "eph1"));
    mem.store_test_entry(MemoryEntry::ephemeral(agent, "eph2"));
    mem.store_test_entry(MemoryEntry::long_term(
        agent,
        MemoryContent::Text("keep".into()),
        vec![],
    ));

    let removed = mem.clear_ephemeral(agent);
    assert_eq!(removed, 2);
    assert_eq!(mem.get_tier(agent, MemoryTier::Ephemeral).len(), 0);
    assert_eq!(
        mem.get_tier(agent, MemoryTier::LongTerm).len(),
        1,
        "long-term should be preserved"
    );
}

#[test]
fn test_clear_ephemeral_empty_agent() {
    let mem = LayeredMemory::new();
    assert_eq!(mem.clear_ephemeral("nobody"), 0);
}

// ─── is_cid_referenced ───────────────────────────────────────────

#[test]
fn test_is_cid_referenced() {
    let mem = LayeredMemory::new();

    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "ref1".into(),
        agent_id: "a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::ObjectRef("sha256:abc123".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    mem.store_test_entry(MemoryEntry::ephemeral("a", "plain text"));

    assert!(mem.is_cid_referenced("sha256:abc123"));
    assert!(!mem.is_cid_referenced("sha256:nonexistent"));
}

#[test]
fn test_is_cid_referenced_across_all_tiers() {
    let mem = LayeredMemory::new();

    // Check ephemeral
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "e1".into(),
        agent_id: "a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Ephemeral,
        content: MemoryContent::ObjectRef("cid:eph".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    assert!(mem.is_cid_referenced("cid:eph"));

    // Check long-term
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "lt1".into(),
        agent_id: "a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::LongTerm,
        content: MemoryContent::ObjectRef("cid:lt".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    assert!(mem.is_cid_referenced("cid:lt"));

    // Check procedural
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "pr1".into(),
        agent_id: "a".into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::ObjectRef("cid:proc".into()),
        importance: 90,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
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
    mem.store_test_entry(entry);

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
    mem.store_test_entry(e1);

    let e2 = MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "wk".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("work".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    };
    mem.store_test_entry(e2);

    assert!(mem.touch_entry(agent, &e1_id));
    assert!(mem.touch_entry(agent, "wk"));
    assert!(!mem.touch_entry(agent, "nonexistent"));
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

    mem.store_test_entry(MemoryEntry::ephemeral(agent, "ephemeral note"));
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "w1".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("working data".into()),
        importance: 50,
        access_count: 3,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });
    mem.store_test_entry(MemoryEntry::long_term(
        agent,
        MemoryContent::Text("long term fact".into()),
        vec![],
    ));

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
    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "expiring".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Working,
        content: MemoryContent::Text("about to expire".into()),
        importance: 50,
        access_count: 0,
        last_accessed: now,
        created_at: now,
        tags: vec![],
        ttl_ms: Some(100),            // current TTL
        original_ttl_ms: Some(10000), // original was 10s
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });

    let stats = mem.get_stats();
    // remaining = 100 - ~0 = ~100. 10% of 10000 = 1000. 100 <= 1000 => about_to_expire
    assert_eq!(stats.about_to_expire_count, 1);
}

// Tick maintenance without a ledger.

#[test]
fn test_tick_increments_without_a_ledger() {
    let mem = LayeredMemory::new();

    // Without a ledger, the maintenance counter still advances.
    for _ in 0..49 {
        assert!(!mem.tick(), "should not trigger before threshold");
    }
    // The 50th tick reaches the maintenance threshold.
    assert!(mem.tick(), "50th tick should trigger");
}

// ─── ledger flush / restore without a ledger ─────────────────

#[test]
fn test_flush_without_ledger_reports_unavailable() {
    let mem = LayeredMemory::new();
    mem.store_test_entry(MemoryEntry::ephemeral("a", "data"));
    assert!(!mem.flush_ledger().unwrap());
}

// ─── find_entry in procedural tier ───────────────────────────────

#[test]
fn test_find_entry_in_procedural() {
    let mem = LayeredMemory::new();
    let agent = "agent";

    mem.store_test_entry(MemoryEntry {
        memory_id: Default::default(),
        parent_revision_id: None,
        canonical_content_hash: Default::default(),
        id: "proc-find".into(),
        agent_id: agent.into(),
        tenant_id: "default".to_string(),
        tier: MemoryTier::Procedural,
        content: MemoryContent::Text("findable proc".into()),
        importance: 90,
        access_count: 0,
        last_accessed: now_ms(),
        created_at: now_ms(),
        tags: vec![],
        ttl_ms: None,
        original_ttl_ms: None,
        scope: MemoryScope::Private,
        memory_type: MemoryType::default(),
        causal_parent: None,
        supersedes: None,
        superseded_by: None,
        deleted_at: None,
    });

    let found = mem.find_entry(agent, "proc-find");
    assert!(found.is_some());
    assert_eq!(found.unwrap().tier, MemoryTier::Procedural);
}

// ─── Serialization roundtrip for MemoryTier ──────────────────────

#[test]
fn test_memory_tier_serialization_roundtrip() {
    for tier in [
        MemoryTier::Ephemeral,
        MemoryTier::Working,
        MemoryTier::LongTerm,
        MemoryTier::Procedural,
    ] {
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

#[test]
fn canonical_hash_ignores_runtime_and_projection_fields() {
    let mut entry = MemoryEntry::ephemeral("owner", "stable fact");
    let canonical = entry.canonical_content_hash.clone();
    entry.access_count = 99;
    entry.last_accessed = u64::MAX;
    entry.importance = 1;
    entry.tier = MemoryTier::LongTerm;
    assert_eq!(entry.content.canonical_content_hash().unwrap(), canonical);
}

#[test]
fn structured_hash_uses_rfc_8785_key_order() {
    let left = MemoryContent::Structured(serde_json::json!({"b": 2, "a": 1}));
    let right = MemoryContent::Structured(serde_json::json!({"a": 1, "b": 2}));
    assert_eq!(
        left.canonical_content_hash().unwrap(),
        right.canonical_content_hash().unwrap()
    );
}

#[test]
fn structured_hash_rejects_jcs_unsafe_integer() {
    for value in [9_007_199_254_740_992_i64, -9_007_199_254_740_992_i64] {
        let content = MemoryContent::Structured(serde_json::json!({"unsafe": value}));
        assert_eq!(content.canonical_content_hash(), Err("jcs_unsafe_integer"));
    }
}

#[test]
fn structured_hash_accepts_jcs_safe_integer_boundary() {
    for value in [9_007_199_254_740_991_i64, -9_007_199_254_740_991_i64] {
        let content = MemoryContent::Structured(serde_json::json!({"safe": value}));
        assert!(content.canonical_content_hash().is_ok());
    }
}

#[test]
fn knowledge_hash_rejects_non_finite_confidence() {
    let content = MemoryContent::Knowledge(KnowledgePiece {
        subject: "s".into(),
        statement: "p".into(),
        confidence: f32::NAN,
        source: "source".into(),
    });
    assert_eq!(content.canonical_content_hash(), Err("non_finite_knowledge_confidence"));
}

// ─── Multi-agent isolation ───────────────────────────────────────

#[test]
fn test_count_for_agent_isolation() {
    let mem = LayeredMemory::new();
    mem.store_test_entry(MemoryEntry::ephemeral("a1", "data1"));
    mem.store_test_entry(MemoryEntry::ephemeral("a1", "data2"));
    mem.store_test_entry(MemoryEntry::ephemeral("a2", "data3"));

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
    for tier in [
        MemoryTier::Ephemeral,
        MemoryTier::Working,
        MemoryTier::LongTerm,
        MemoryTier::Procedural,
    ] {
        assert!(mem.get_tier("ghost", tier).is_empty());
    }
}

#[test]
fn memory_entry_rejects_legacy_inline_embedding() {
    let entry = MemoryEntry::long_term("agent", MemoryContent::Text("canonical".into()), vec![]);
    let mut value = serde_json::to_value(entry).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("embedding".into(), serde_json::json!([0.1, 0.2]));
    assert!(serde_json::from_value::<MemoryEntry>(value).is_err());
}

#[test]
fn test_recall_lexical_is_same_domain_and_namespace_scoped() {
    let mem = LayeredMemory::new();
    let mut wanted = MemoryEntry::long_term(
        "agent",
        MemoryContent::Text("Plico remembers punctuation safely".into()),
        vec!["memory".into()],
    );
    wanted.id = "wanted".into();
    wanted.tenant_id = "personal".into();
    mem.store_test_entry(wanted);

    let mut other_namespace = MemoryEntry::long_term(
        "agent",
        MemoryContent::Text("Plico should not cross a local namespace".into()),
        vec![],
    );
    other_namespace.id = "other".into();
    other_namespace.tenant_id = "archive".into();
    mem.store_test_entry(other_namespace);

    let hits = mem.recall_lexical("agent", "personal", "What is Plico?", 10);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, "wanted");

    let mut chinese = MemoryEntry::long_term("agent", MemoryContent::Text("太初是个人用户的数字分身".into()), vec![]);
    chinese.id = "chinese".into();
    chinese.tenant_id = "personal".into();
    mem.store_test_entry(chinese);
    let hits = mem.recall_lexical("agent", "personal", "什么是太初？", 10);
    assert!(hits.iter().any(|(entry, _)| entry.id == "chinese"));
}
