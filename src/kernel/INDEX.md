# Module: kernel

AI Kernel — the central orchestrator that wires together all Plico subsystems (CAS, memory, scheduler, FS, permissions, intent, tools, messaging).

Status: active | Fan-in: 3 | Fan-out: 7

## Dependents (Fan-in: 4)

- `src/bin/plicod.rs` → AIKernel (daemon hosts kernel, serves via UDS + TCP)
- `src/bin/plico_mcp/` → AIKernel (MCP server creates kernel for JSON-RPC dispatch)
- `src/bin/aicli/main.rs` → AIKernel via `KernelClient` (daemon-first, `--embedded` fallback)
- `src/client.rs` → `EmbeddedClient` wraps AIKernel directly

## Modification Risk

- Add `AIKernel` public method → compatible if callers updated in binaries
- Change `AIKernel::new()` signature → BREAKING, update all 3 binaries
- Remove kernel method → BREAKING, update plicod + plico_mcp + aicli dispatch
- Change `execute_tool` dispatch → affects all tool clients; check `builtin_tools.rs`
- Change `handle_public_request` or its context → BREAKING for plicod, clients, MCP, and aicli
- Change legacy `handle_api_request` → internal-only migration risk; it is not a public wire contract

## Task Routing

- Add a public operation → first amend ADR-0003 and `api/public/`; then update
  `public_service.rs`, MCP/aicli mappings, capability parity tests, and every transport in one cutover
- Built-in tool registration / `execute_tool` → `builtin_tools.rs`
- Persistence / restore / embedding bootstrap → `persistence.rs`
- Event bus / event log / sequenced events → `event_bus.rs`
- Operation-specific logic → see `ops/INDEX.md`, including projection runtime/controller ownership
- Cognitive optimization / skill extraction / intent network → `cognition/`

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Central orchestrator — all subsystem access, including side-effect-free runtime readiness and cognitive indexing watermarks |
| `handle_public_request` | `public_service.rs` | Direct typed dispatch for the 14-operation `plico.personal.v2` contract |
| `ensure_personal_owner_credential` | `public_service.rs` | Durable local daemon bootstrap; returns only the owner-only credential file path, never the bearer |
| `authenticate_public_bearer` | `public_service.rs` | Resolves a TCP bearer to a trusted local role with a stable, non-enumerating error |

The public product path is `handle_public_request`; it never constructs or imports legacy
`ApiRequest`/`ApiResponse`. Scheduler, graph mutation, model control, permission mutation, generic
tools, and legacy semantic handlers are internal surfaces, not remotely advertised capabilities.

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~574 | AIKernel struct, orchestration core |
| `public_service.rs` | Typed personal-vault context, auth boundary and direct 14-operation service |
| `api_dispatch.rs` | ~250 | Thin API dispatch → 14 handler modules in `handlers/` |
| `handlers/` | 14 files | Domain-specific API request handlers (cas, memory, agent, graph, etc.) |
| `builtin_tools.rs` | ~480 | Tool registration + dispatch → 7 tool modules in `tools/` |
| `tools/` | 7 files | Built-in tool handlers (cas, memory, graph, agent, system, messaging, permission) |
| `hook.rs` | ~266 | HookRegistry — 5 interception points, Block/Continue results |
| `persistence.rs` | ~366 | Persist/restore agents, intents, memories, search index, event log |
| `event_bus.rs` | ~959 | EventBus — typed pub/sub, JSONL persistence, restore |
| `ops/` | operation modules | Operation groups, including manifest-backed projection control — see `ops/INDEX.md` |
| `cognition/` | 12 files | Soul v3.0 cognitive symbiotic engine — see `cognition/INDEX.md` |

## Dependencies (Fan-out: 8)

- `src/cas/` — CASStorage, AIObject
- `src/memory/` — LayeredMemory, persistence traits, relevance, context snapshot
- `src/scheduler/` — AgentScheduler, messaging, dispatch types
- `src/fs/` — SemanticFS, search, KG, embedding
- `src/api/public/` — sole external typed protocol; legacy semantic API remains internal migration debt
- `src/intent/` — ChainRouter, intent resolution
- `src/tool/` — ToolRegistry
- `src/kernel/cognition/` — CognitiveLoop, SkillForge, IntentSemanticNetwork (internal dependency, initialized in `AIKernel::new`)

## Interface Contract

- `AIKernel::new()`: initializes subsystems; embedding backend from env (`EMBEDDING_BACKEND`, etc.)
- `handle_public_request()`: receives a trusted `PublicRequestContext`; operation input never carries
  role, namespace, tier, scope, or permission grants
- `PublicAccess::LocalOwner` uses stable internal identity `personal-owner`; authenticated non-owner
  roles still pass Read/Write/Delete capability checks
- Working Memory public writes are canonical-first: persist, publish, then send a lossy projection wake;
  persistence failure is `DEPENDENCY_UNAVAILABLE` and cannot publish a successful mutation
- `pub(crate)` fields: library-internal only; crate integration tests in `tests/` must use public API
- Thread safety: kernel not `Clone`; daemon uses `Arc<AIKernel>`
- EventBus: JSONL append-on-emit, restore on startup via `restore_event_log()`
- Runtime readiness exposes the cognitive queue's accepted watermark, contiguous completed watermark,
  and in-flight count. These are observations only: reading them never probes a model, enqueues work,
  or changes the single-receiver/concurrent-task execution policy.

## P3-A projection orchestration

- The manifest worker treats its in-memory queue only as a wake-up hint; durable Queued/Building/Failed/Stale state and retry/lease truth live in the ADR-0005 manifest.
- Canonical commits remain first and independent. Projection append failure yields an unreconciled observation and is recovered by a full canonical-watermark scan; it never rolls back the memory write.
- Ready embedding artifacts do not activate Memory vector recall. Public recall remains lexical until a separate retrieval benchmark and ADR accept vector/hybrid execution.
- `ProjectionRuntime` owns the controller/worker lifecycle; public status/rebuild never receive a raw manifest writer.
- Startup with unavailable or changed provider identity keeps canonical/lexical service available and reports projection control-plane/worker health separately.
- `plico.personal.v2` is the only wire contract; personal.v1 has no reader, alias or adapter.

## Tests

- Integration: `tests/kernel_test.rs`, `tests/ai_experience_test.rs`
- Critical: exact capability catalog, canonical read-after-write, persistence failure rollback,
  role-scoped session changes, object-search execution diagnostics, bearer bootstrap/restart stability
