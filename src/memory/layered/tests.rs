//! Layered memory tests — extracted for module compliance.

#[allow(unused_imports)]
use crate::memory::{LayeredMemory, MemoryTier, MemoryEntry};

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
