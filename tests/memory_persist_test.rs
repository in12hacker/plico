//! Memory persistence integration tests
//!
//! Tests cover: persist → restart → restore, auto-persist on tick,
//! kernel integration, and multi-agent isolation.

use plico::memory::{CASPersister, MemoryLoader, MemoryPersister, LayeredMemory, MemoryEntry, MemoryContent, MemoryTier};
use tempfile::tempdir;

fn make_persister_and_memory() -> (CASPersister, LayeredMemory, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let cas = plico::cas::CASStorage::new(dir.path().join("cas")).unwrap();
    let persister = CASPersister::new(
        std::sync::Arc::new(cas),
        dir.path().to_path_buf(),
    ).unwrap();
    let memory = LayeredMemory::new();
    memory.set_persister(std::sync::Arc::new(persister.clone()));
    (persister, memory, dir)
}

#[test]
fn test_memory_persists_and_restores_on_restart() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();

    // First session: create persister + memory, store, persist
    let cas = plico::cas::CASStorage::new(root.join("cas")).unwrap();
    let persister = CASPersister::new(std::sync::Arc::new(cas), root.clone()).unwrap();
    let memory = LayeredMemory::new();
    memory.set_persister(std::sync::Arc::new(persister.clone()));

    let entry = MemoryEntry::long_term("agent1", MemoryContent::Text("Hello from past session".to_string()), vec!["greeting".to_string()]);
    memory.store(entry);
    memory.persist_all();

    // Simulate restart: new memory + new persister from same root
    drop(memory);
    drop(persister);

    let new_cas = plico::cas::CASStorage::new(root.join("cas")).unwrap();
    let new_persister = CASPersister::new(std::sync::Arc::new(new_cas), root).unwrap();
    let new_memory = LayeredMemory::new();
    new_memory.set_persister(std::sync::Arc::new(new_persister));

    new_memory.restore_agent("agent1").unwrap();

    let all = new_memory.get_all("agent1");
    assert!(!all.is_empty());
    assert!(all.iter().any(|e| e.content.display().contains("past session")));
}

#[test]
fn test_persist_isolated_between_agents() {
    let (persister, memory, _dir) = make_persister_and_memory();

    memory.store(MemoryEntry::long_term("alice", MemoryContent::Text("Alice's secret".to_string()), vec![]));
    memory.store(MemoryEntry::long_term("bob", MemoryContent::Text("Bob's secret".to_string()), vec![]));

    memory.persist_all();

    let alice_entries: Vec<MemoryEntry> = persister.load("alice", MemoryTier::LongTerm).unwrap();
    let bob_entries: Vec<MemoryEntry> = persister.load("bob", MemoryTier::LongTerm).unwrap();

    assert!(alice_entries.iter().any(|e| e.content.display().contains("Alice")));
    assert!(bob_entries.iter().any(|e| e.content.display().contains("Bob")));
    assert!(!alice_entries.iter().any(|e| e.content.display().contains("Bob")));
}

#[test]
fn test_auto_persist_on_tick_threshold() {
    let (persister, memory, _dir) = make_persister_and_memory();

    // Store 1 entry (far below threshold of 50)
    memory.store(MemoryEntry::ephemeral("agent1", "entry 1"));

    // Not persisted yet
    assert!(!persister.has_persisted("agent1"), "precondition: should not be persisted before loop");

    // Tick up to threshold (50 ops)
    // Note: entries must be in a persistable tier (Working/LongTerm/Procedural), not Ephemeral.
    for i in 0..50 {
        let mut entry = MemoryEntry::long_term(
            "agent1",
            MemoryContent::Text(format!("filler-{}", i)),
            vec![],
        );
        entry.tier = MemoryTier::Working; // explicitly Working tier (persisted)
        memory.store(entry);
    }

    // Should have triggered auto-persist
    assert!(persister.has_persisted("agent1"), "should have auto-persisted after 50 ops");
}

#[test]
fn test_memory_loader_restores_all_agents() {
    let (persister, memory, _dir) = make_persister_and_memory();

    memory.store(MemoryEntry::long_term("a1", MemoryContent::Text("agent1 data".to_string()), vec![]));
    memory.store(MemoryEntry::long_term("a2", MemoryContent::Text("agent2 data".to_string()), vec![]));
    memory.store(MemoryEntry::long_term("a3", MemoryContent::Text("agent3 data".to_string()), vec![]));
    memory.persist_all();

    let loader = MemoryLoader::new(std::sync::Arc::new(persister));
    let restored = loader.restore_all();

    assert_eq!(restored.len(), 3);
    assert!(restored.contains_key("a1"));
    assert!(restored.contains_key("a2"));
    assert!(restored.contains_key("a3"));
}

#[test]
fn test_empty_tier_not_persisted() {
    let (persister, _memory, _dir) = make_persister_and_memory();

    let result = persister.persist("agent1", MemoryTier::LongTerm, &[]);
    assert!(result.unwrap().is_empty()); // empty → no CID
}
