# Module: memory

Personal-vault memory runtime and immutable canonical revision ledger. Ephemeral is runtime-only; Working, LongTerm, and Procedural commits are append-only canonical streams. Embeddings, access counters, TTL, importance, and heat are rebuildable projections and never participate in canonical content hashes.

Status: V1-B canonical Truth Firewall and P3-A memory_embedding control-plane cutover active | Fan-in: kernel | Fan-out: CAS filesystem boundary

## Dependents

- `src/kernel/mod.rs` owns the sole ledger instance and rebuilds runtime projections at startup.
- `src/kernel/public_service.rs` exposes typed memory create/get/recall/update/delete and projection status/rebuild operations.
- `src/kernel/ops/projection_runtime.rs` and `projection_controller/` reconcile and build manifest-backed embedding projections after canonical commit.
- `src/bin/plico_memory_migrate/` is the feature-gated offline legacy-vault migrator.

## Modification risk

- Changing revision, policy, segment, current-view, root, pointer, or manifest schemas is a storage-format break.
- Changing hash domains or JCS behavior invalidates immutable object identities.
- Durable writes must use expected-head commits and publish runtime state only after a ledger receipt.
- Policy actor, origin role, and caller are distinct: `committed_by_role` is audit evidence; `origin_role_id` owns the stream; current policy controls visibility and mutation.
- CAS is the only module allowed to touch the host filesystem.

## Files

| File | Purpose |
|------|---------|
| `layered/mod.rs` | Runtime tiers, typed IDs/content hash, post-receipt projection publication |
| `layered/tests.rs` | Runtime, concurrency, restart, and hash invariants |
| `ledger/model.rs` | Revision/policy/relation/segment/root/current-view schemas |
| `ledger/hash.rs` | Domain-separated RFC 8785/JCS hashes |
| `ledger/validate.rs` | Revision and policy graph validation |
| `ledger/current_view.rs` | Deterministic active-stream materialization |
| `ledger/store.rs` | Expected-head writer, replay loader, offline typed publisher |
| `ledger/migration_manifest.rs` | Runtime-verifiable source/target migration evidence |

## Canonical contract

- A logical memory has stable `memory_id`; each immutable revision has a distinct `revision_id` and optional `parent_revision_id`.
- Content hash domain is `plico.memory.content.v1\0`; Structured JSON uses RFC 8785/JCS and rejects integers outside ±(2^53−1).
- Create appends a root revision plus Private policy in one generation. Update/delete append one expected-head child; delete is an idempotent tombstone and never erases history.
- Private policy readers/writers are the origin role plus `personal-owner`. Personal owner may read/write/delete every stream; unrelated roles see NotFound.
- Segment, CurrentView, and LedgerRoot are immutable objects. The active root pointer is published by a durable two-slot `RENAME_EXCHANGE`; old objects remain addressable.
- Startup verifies raw bytes are canonical, every hash/domain/schema, the complete historical root prefixes, graph invariants, policies, migration manifests, and rebuilt CurrentView.
- Runtime rejects legacy `memory_index.json` before vault mutation. Offline migration owns preflight, evidence manifests, staging replay, typed seal, exchange, and backup.
- Migrated Shared/Group scope becomes a static `ExplicitRoleSet` captured at the migration cutoff; runtime reads and mutations consult that effective policy without duplicating canonical revisions.
- Logs include operation/phase/outcome and low-cardinality role kind, never memory body, tags, query, bearer, raw role/tenant, host path, or full content hash.

## Runtime API boundary

- `LayeredMemory::store_checked` accepts Ephemeral only; durable tiers must use canonical create/batch primitives.
- `create_working_durable`, `update_working_durable`, and `delete_working_durable` are crate-internal commit-then-publish operations.
- Owner/caller authorization is resolved from the current canonical policy before locating the origin runtime bucket.
- Embedding workers append only typed projection manifest transitions/artifacts and never write canonical ledger objects.
- `flush_canonical_memory` acknowledges only canonical ledger durability; auxiliary subsystem persistence is explicitly best-effort.

## P3-A control-plane boundary

- ADR-0005 admits only `memory_embedding`; Object HNSW/BM25, Memory BM25/KG/summary, vector recall and thermal are out of scope.
- Manifest records/segment/root/current-view become the only `memory_embedding` state source. Artifact bytes are durable before a Ready root becomes visible.
- Every status binds stable `memory_id`, `revision_id`, canonical content hash and the full ledger root/generation/revision/policy/relation watermark.
- The single cutover deletes `MemoryEntry.embedding`, the inferred three-state status, runtime retry truth and LongTerm write-time semantic dedup; no fallback or dual write remains.
- The sealed core stores manifest and artifacts under one `projection-store` lifecycle, supports typed read-only inspection, owner rebuild, GenesisOnly orphan maintenance, and owner-only two-phase whole-pair reset/recovery. Raw store writers remain module-private.
- `plico.personal.v2` exposes manifest-backed `projection.status` and owner-only `projection.rebuild`.
  It exposes no inline embedding field and does not activate vector/hybrid/BM25 recall.

## Tests

- Unit: `src/memory/layered/tests.rs`, inline `src/memory/ledger/store.rs` tests.
- Integration: `tests/memory_test.rs`, `tests/kernel_test.rs`, public-service protocol tests.
- Required gates: restart replay, stale-head conflict, tombstone immutability/idempotency, owner/role visibility matrix, raw JCS rejection, root-history validation, OS vault lock, publish indeterminate poison, offline migration staging/seal/replay.
