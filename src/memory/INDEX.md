# Module: memory

Layered memory management — 4-tier cognitive hierarchy (Ephemeral/Working/LongTerm/Procedural) for AI agents.

Status: stable | Fan-in: 2 | Fan-out: 1

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → LayeredMemory, MemoryEntry, CASPersister, MemoryPersister (kernel wiring + remember/recall)
- `src/bin/plicod.rs` [indirect via kernel] → memory operations through API

## Modification Risk

- Add field to `MemoryEntry` → compatible if `#[serde(default)]`, update constructors
- Change `MemoryTier` variants → BREAKING, update all match arms in kernel/API/tests
- Change persistence index format → BREAKING, invalidates persisted memory data
- Change eviction policy → compatible, behavioral change only

## Task Routing

- Add memory tier → modify `src/memory/layered.rs` MemoryTier enum + priority/name
- Change eviction logic → modify `src/memory/layered.rs` LayeredMemory::evict
- Fix persistence → modify `src/memory/persist.rs` CASPersister
- Add memory content type → modify `src/memory/layered.rs` MemoryContent enum

## Public API

| Export | File | Description |
|--------|------|-------------|
| `LayeredMemory` | `layered.rs` | 4-tier in-memory store with eviction |
| `MemoryTier` | `layered.rs` | Tier enum: Ephemeral, Working, LongTerm, Procedural |
| `MemoryEntry` | `layered.rs` | Single memory entry with importance/access tracking |
| `MemoryContent` | `layered.rs` | Content enum: Text, ObjectRef, Structured, Procedure, Knowledge |
| `MemoryError` | `layered.rs` | Typed memory errors |
| `CASPersister` | `persist.rs` | Persists memory entries to CAS |
| `MemoryPersister` | `persist.rs` | Trait for pluggable persistence backends |
| `MemoryLoader` | `persist.rs` | Restores memory entries from CAS on startup |
| `MemoryQuery` | `mod.rs` | Query struct for memory retrieval |
| `MemoryResult` | `mod.rs` | Result struct for memory queries |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `layered.rs` | ⚠ ~482 | LayeredMemory, MemoryTier, MemoryEntry — needs split |
| `persist.rs` | ~384 | CASPersister, MemoryLoader, PersistenceIndex |
| `mod.rs` | ~44 | MemoryQuery, MemoryResult, re-exports |

## Dependencies (Fan-out: 1)

- `src/cas/` — CASPersister stores serialized memory entries as CAS objects

## Interface Contract

- `LayeredMemory::store()`: adds entry to specified tier; auto-evicts if tier capacity exceeded
- `LayeredMemory::recall()`: returns entries by agent_id + tier, sorted by last_accessed
- `CASPersister::persist()`: serializes all entries for an agent to CAS; returns persisted CIDs
- `MemoryLoader::load()`: restores entries from CAS using persistence index
- Thread safety: all public methods use `RwLock` — safe for concurrent access

## Tests

- Unit: `src/memory/layered.rs` mod tests
- Integration: `tests/memory_test.rs`, `tests/memory_persist_test.rs`
- Critical: `test_store_and_recall`, `test_eviction_by_importance`
