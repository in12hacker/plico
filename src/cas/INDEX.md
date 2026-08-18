# Module: cas

Content-Addressed Storage — SHA-256 hash as object identity, automatic deduplication, immutability by default.

Status: stable | Fan-in: 5 | Fan-out: 0

## Dependents (Fan-in: 5)

- `src/fs/semantic_fs.rs` → CASStorage (read/write objects, rebuild index)
- `src/fs/context_loader.rs` → CASStorage (L2 full content reads)
- `src/memory/persist.rs` → CASStorage, AIObject, AIObjectMeta (memory persistence)
- `src/kernel/mod.rs` → CASStorage, AIObject, AIObjectMeta (kernel wiring)
- `src/bin/plicod.rs` [indirect via kernel] → CAS operations through API

## Modification Risk

- Add field to `AIObjectMeta` → compatible if `#[serde(default)]`, update all constructors
- Change `ContentType` variants → BREAKING, update all `match` arms in fs/kernel/api
- Change CID algorithm (SHA-256) → BREAKING, invalidates all stored objects
- Change shard layout → BREAKING, existing objects become unfindable

## Task Routing

- Add new content type → modify `src/cas/object.rs` ContentType enum + Display + from_extension
- Fix CID computation → modify `src/cas/object.rs` AIObject::compute_cid
- Change storage layout → modify `src/cas/storage.rs` shard_dir/object_path
- Add metadata field → modify `src/cas/object.rs` AIObjectMeta + all callers
- Change offline migration lock/copy/exchange → modify `offline_migration.rs`; binaries must not touch host files directly

## P3-A vault/storage core boundary

ADR-0005 requires one `PersonalVaultStorage` to own the existing parent-level exclusive lock for the
runtime lifetime, then issue fixed handles for `memory-ledger`, the single
`projection-store/{manifest,artifacts}` lifecycle, and object CAS. Opening another
`ImmutableLedgerStorage` for the same vault is not an allowed projection design. The sealed projection
core uses staging + whole-parent `NOREPLACE`/`RENAME_EXCHANGE`, a two-phase reset marker, and bounded
NOFOLLOW/NO_XDEV quarantine recovery; it never exposes a direct live-create writer or recursively deletes
an untrusted exchanged tree. Artifact bytes must be owner-only, durable and hash-verified before the
projection manifest root pointer can publish Ready; unsupported atomic exchange remains fail closed.

## v53 execution-observation fixture boundary

ADR-0008 adds a default-off, fixed `execution-observation-fixture-ledger` namespace. Only the sealed
`ExecutionObservationFixtureStorage` may claim it from an existing `Arc<PersonalVaultStorage>`; the generic
immutable-ledger entrypoint rejects this namespace. The sealed handle exposes bounded object/pointer reads,
bounded collision writes and dual-slot publish, but no host path, arbitrary namespace or unbounded inventory.
Collision publication is a single `NOREPLACE` attempt followed, only for an existing winner, by a bounded
read/compare; it never enters the generic ledger's pre-read or unbounded collision path.
The sealed handle has no general `flush` capability: immutable publication and active-pointer publication own
their durability boundaries, so an upper layer cannot create a redundant post-publish uncertainty window.
Existing topology is validated exactly and is never chmod-repaired or completed. This is an internal WP2
substrate, not a public API or a live execution-observation writer.

`PersonalVaultStorage::with_existing_execution_observation_readonly` (WP3A.2-A) is the existing-only read
counterpart: a closure-bounded `ExistingExecutionObservationReadOnly` view with exactly two bounded reads
(`read_active_bounded`, `get_immutable_bounded`). It never creates, completes, chmods or claims the namespace
(an absent namespace yields `None`, damaged topology fails closed), never touches the candidate slot or host
paths, and takes no locks — a writer may hold the namespace claim on the same vault while readers observe only
complete pre- or post-exchange active pointers.

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIObject` | `object.rs` | Fundamental data unit — CID + data + metadata |
| `AIObjectMeta` | `object.rs` | Semantic metadata (tags, content_type, created_by) |
| `ContentType` | `object.rs` | Content classification enum (Text/Image/Audio/Video/etc.) |
| `CASStorage` | `storage.rs` | Disk-backed content-addressed store |
| `CASError` | `storage.rs` | Typed errors (NotFound, IntegrityFailed, Io, Serialization) |
| `OfflineMigrationVault` | `offline_migration.rs` | Feature-gated lock, exact reads, bounded copy, seal verification, exchange and rollback backup |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `object.rs` | ~213 | AIObject, AIObjectMeta, ContentType definitions |
| `storage.rs` | ~254 | CASStorage engine: put/get/delete/list with sharding |
| `ledger_store.rs` | Generic immutable object/root-slot storage used by canonical memory |
| `execution_observation_store.rs` | Fixed sealed and bounded CAS capability for ADR-0008 |
| `execution_observation_store/tests.rs` | Capability, topology and crash-window adversarial tests |
| `offline_migration.rs` | CAS-owned offline migration filesystem boundary |
| `mod.rs` | ~18 | Re-exports |

## Dependencies (Fan-out: 0)

None — CAS is the lowest layer, depends only on std + external crates (sha2, serde, serde_json).

## Interface Contract

- `CASStorage::put()`: idempotent — same content always returns same CID; integrity verified before write
- `CASStorage::get()`: returns `CASError::NotFound` if CID absent; integrity verified on read
- `AIObject::new()`: CID computed automatically from data bytes via SHA-256
- Thread safety: `CASStorage` is safe for concurrent reads; concurrent writes to different CIDs are safe (different shard dirs)
- Side effect: `put()` creates shard directories and writes files atomically (temp file → rename)

## Tests

- Unit: `src/cas/object.rs` mod tests, `src/cas/storage.rs` mod tests
- Integration: `tests/kernel_test.rs` (exercises CAS through kernel)
- Critical: `test_put_and_get`, `test_deduplication`, `test_cid_is_content_hash`
- Migration failure injection: `src/cas/offline_migration.rs` inline tests
- Execution-observation capability: `src/cas/execution_observation_store/tests.rs`
