# Module: memory — Layered Memory Management

Four-tier memory system mirroring AI cognitive architecture.

Status: stable | Fan-in: 2 (kernel, scheduler) | Fan-out: 0

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

## Dependencies (Fan-out: 0)

None — leaf module.

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `LayeredMemory::store`, `get_all`, `evict_ephemeral`
- `src/bin/plicod.rs` → `MemoryContent` (recall handler)

## Interface Contract

- `LayeredMemory::store(entry)`: Inserts into tier based on `entry.tier`. **Side effect**: acquires write lock.
- `LayeredMemory::evict_ephemeral(agent_id)`: Removes ephemeral entries. Entries with `importance >= 70` promoted to Working tier.
- `MemoryEntry::ephemeral()`: Factory — tier=Ephemeral, importance=50.
- `MemoryContent::display()`: Returns human-readable string for any variant.

## Modification Risk

- Add new `MemoryTier` variant → **BREAKING** for tier ordering, matching logic
- Change eviction threshold (70) → affects memory pressure behavior
- Change `MemoryContent` enum variants → **BREAKING** for all pattern matches

## Task Routing

- Add new memory tier → modify `MemoryTier` + `LayeredMemory::store()` + eviction logic
- Change eviction policy → modify `LayeredMemory::evict_ephemeral()`
- Add semantic search to memory → new method in `LayeredMemory`, update kernel

## Tests

- Unit tests in `layered.rs` — tier behavior, eviction, promotion
- Integration: kernel `remember`/`recall` operations (in daemon tests)
