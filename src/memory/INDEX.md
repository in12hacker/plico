# Module: memory — Layered Memory Management

Four-tier memory system with optional CAS persistence.

Status: stable | Fan-in: 1 (kernel) | Fan-out: 1 (cas via persist.rs)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `LayeredMemory` | `layered.rs` | 4-tier memory store: store/get_tier/get_all/evict_ephemeral |
| `MemoryTier` | `layered.rs` | Enum: Ephemeral/Working/LongTerm/Procedural |
| `MemoryEntry` | `layered.rs` | Entry: id/agent_id/tier/content/importance/access_count |
| `MemoryContent` | `layered.rs` | Enum: Text/ObjectRef/Structured/Procedure/Knowledge |
| `MemoryError` | `layered.rs` | Error: NotFound, Serialization, TierCapacityExceeded |
| `MemoryQuery` | `mod.rs` | Query: query text + tier filter + limit + agent_id |
| `MemoryResult` | `mod.rs` | Result: entries + tier + total count |
| `MemoryPersister` | `persist.rs` | Trait: persist/load/list_persisted/has_persisted |
| `CASPersister` | `persist.rs` | CAS-backed persister implementation |
| `MemoryLoader` | `persist.rs` | Loads persisted memories from CAS |
| `PersistError` | `persist.rs` | Error: Io, Serialization, CAS, AgentNotFound |
| `PersistenceIndex` | `persist.rs` | Index: maps agent_id → Vec\<PersistedTier\> |

## Dependencies (Fan-out: 0)

None — leaf module. `persist.rs` depends on `cas` (imported via `crate::cas`).

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `LayeredMemory::store`, `get_all`, `evict_ephemeral`, `persist_all`, `restore_agent`
- `src/bin/plicod.rs` → `MemoryContent` (recall handler)

## Interface Contract

- `LayeredMemory::store(entry)`: Inserts into tier based on `entry.tier`. **Side effect**: increments op counter; auto-persists after 50 ops (Working/LongTerm/Procedural tiers only).
- `LayeredMemory::evict_ephemeral(agent_id)`: Removes ephemeral entries. Entries with `importance >= 70` promoted to Working tier.
- `LayeredMemory::set_persister(p)`: Attaches a persister for L1/L2 durability.
- `LayeredMemory::persist_all()`: Persists all Working/LongTerm/Procedural entries to CAS.
- `LayeredMemory::restore_agent(agent_id)`: Restores persisted entries from CAS for one agent.
- `CASPersister::persist(agent, tier, entries)`: Serializes entries → JSON → CAS. Updates `memory_index.json`.
- Ephemeral tier entries are **never persisted** (in-memory only).

## Modification Risk

- Add new `MemoryTier` variant → **BREAKING** for tier ordering, matching logic
- Change eviction threshold (70) → affects memory pressure behavior
- Change `MemoryContent` enum variants → **BREAKING** for all pattern matches
- Change `DEFAULT_PERSIST_OP_COUNT` (50) → affects auto-persist frequency

## Task Routing

- Add new memory tier → modify `MemoryTier` + `LayeredMemory::store()` + eviction logic
- Change eviction policy → modify `LayeredMemory::evict_ephemeral()`
- Change persistence trigger → modify `LayeredMemory::tick()` / `DEFAULT_PERSIST_OP_COUNT`
- Add semantic search to memory → new method in `LayeredMemory`, update kernel

## Tests

- Unit tests in `persist.rs` — round-trip, index persistence, multi-tier isolation (co-located)
- Integration tests: `tests/memory_test.rs` — tier behavior, eviction, promotion (12 tests)
- Integration tests: `tests/memory_persist_test.rs` — full persist → restart → restore cycle (5 tests)
