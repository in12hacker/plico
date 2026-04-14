# Module: api — Permission Guardrails & Semantic Protocol

Enforces access control and provides JSON serialization for IPC.

Status: stable | Fan-in: 3 (kernel, plicod, aicli) | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `PermissionGuard` | `permission.rs` | Access control: check/grant/revoke_all |
| `PermissionContext` | `permission.rs` | Agent identity + granted permissions |
| `PermissionAction` | `permission.rs` | Enum: Read/Write/Delete/Network/Execute/All |
| `PermissionGrant` | `permission.rs` | Grant with scope + expiration |
| `ApiRequest` | `semantic.rs` | JSON request enum (serde tag) |
| `ApiResponse` | `semantic.rs` | JSON response: ok/cid/data/results/error |
| `SearchResultDto` | `semantic.rs` | DTO: cid + relevance + tags |
| `AgentDto` | `semantic.rs` | DTO: id + name + state |

## Dependencies (Fan-out: 0)

Leaf module.

## Dependents (Fan-in: 3)

- `src/kernel/mod.rs` → `PermissionGuard::check()`
- `src/bin/plicod.rs` → `ApiRequest`/`ApiResponse` for JSON protocol
- `src/bin/aicli.rs` → `ApiRequest`/`ApiResponse` for TCP mode

## Interface Contract

- `PermissionGuard::check(ctx, action)`: Returns `Ok(())` if allowed. Trusted agents (`kernel`, `system`) bypass all checks.
- Default policy: `Read`/`Write` allowed by default; `Delete`/`Network`/`Execute` require explicit grant.
- `ApiRequest` is a serde-tagged enum: `{"method": "create", "params": {...}}`
- `ApiResponse` fields skipped if `None` (serde `skip_serializing_if`)

## Modification Risk

- Add new `PermissionAction` → update `PermissionGuard::check()` matching, all `covers()` implementations
- Change default policy (allow → deny) → **BREAKING** for all agents
- Add new `ApiRequest` variant → update both `plicod.rs` and `aicli.rs` handlers
- Add authentication (token-based) → extend `PermissionContext` with token field

## Task Routing

- Add OAuth/API key auth → extend `PermissionContext`, add auth middleware in plicod
- Add rate limiting → new module `api/ratelimit.rs`, wire into `plicod::handle_connection`
- Add WebSocket support → new module `api/websocket.rs`, upgrade from TCP to async
- Add request batching → extend `ApiRequest` with `Batch { requests: Vec<ApiRequest> }` variant

## Tests

- Permission tests: unit tests for trust bypass, action coverage
- Protocol tests: serialize/deserialize round-trip for all `ApiRequest`/`ApiResponse` variants
- Integration: `plicod` TCP echo test (send request, validate response JSON)
