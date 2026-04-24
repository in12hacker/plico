# Module: bin/aicli/commands/handlers

CLI command handlers — one file per command group. In `--embedded` mode, handlers translate CLI args into `ApiRequest` calls via `AIKernel` directly. In daemon mode (default), the `build_remote_request` function in `main.rs` constructs `ApiRequest` variants sent to `plicod` via `RemoteClient`.

Status: active | Fan-in: 1 | Fan-out: 2

## Dependents (Fan-in: 1)

- `src/bin/aicli/commands/mod.rs` → all `cmd_*` functions (dispatch table)

## Dependencies (Fan-out: 2)

- `src/kernel/` → `AIKernel` (all handlers call kernel methods)
- `src/api/` → `ApiRequest`, `ApiResponse` (protocol types)

## Task Routing

| Task | File |
|------|------|
| Add/fix put/get/search/update/delete/history/rollback | `crud.rs` |
| Add/fix agent register/status/suspend/resume/terminate/checkpoint/restore/discover/delegate | `agent.rs` |
| Add/fix remember/recall/memmove/memdelete | `memory.rs` |
| Add/fix KG node/edge/paths/explore | `graph.rs` |
| Add/fix deleted/restore (recycle bin) | `deleted.rs` |
| Add/fix intent resolution | `intent.rs` |
| Add/fix send/messages/ack | `messaging.rs` |
| Add/fix tool list/describe/call | `tool.rs` |
| Add/fix events list/by-time | `events.rs` |
| Add/fix context assembly | `context.rs` |
| Add/fix skills register/discover | `skills.rs` |
| Add/fix session-start/session-end/growth | `session.rs` |
| Add/fix delta change tracking | `delta.rs` |
| Add/fix hybrid Graph-RAG retrieval | `hybrid.rs` |
| Add/fix permission grant/revoke/list | `permission.rs` |

## Files

| File | Lines | Has Tests | Purpose |
|------|-------|-----------|---------|
| `mod.rs` | ~56 | — | Module declarations + re-exports |
| `crud.rs` | ~289 | 5 | put/get/search/update/delete/history/rollback |
| `agent.rs` | ~292 | 7 | Agent lifecycle commands |
| `memory.rs` | ~229 | 7 | Memory tier commands |
| `graph.rs` | ~323 | 5 | KG commands |
| `events.rs` | ~182 | 7 | Event commands |
| `intent.rs` | ~169 | 5 | Intent resolution |
| `session.rs` | ~152 | 7 | Session lifecycle |
| `skills.rs` | ~185 | 5 | Procedural skills |
| `permission.rs` | ~98 | 0 | Permission management |
| `context.rs` | ~75 | 0 | Context loading |
| `messaging.rs` | ~42 | 0 | Inter-agent messaging |
| `deleted.rs` | ~31 | 0 | Recycle bin |
| `tool.rs` | ~31 | 0 | Tool management |
| `delta.rs` | ~29 | 0 | Delta tracking |
| `hybrid.rs` | ~43 | 0 | Hybrid retrieval |
| `hook.rs` | ~87 | 4 | Hook register/list |

## Interface Contract

- All `cmd_*` functions: `fn(kernel: &AIKernel, args: &[String]) -> ApiResponse`
- Handlers call `kernel.handle_api_request(ApiRequest::...)` and return `ApiResponse`
- In daemon mode, `build_remote_request` in `main.rs` constructs `ApiRequest` directly from CLI args

## Modification Risk

- Change `cmd_*` signature → update dispatch table in `commands/mod.rs`
- Change args parsing → affects CLI users
- Add new handler → add file, re-export in `mod.rs`, add dispatch case in `commands/mod.rs`

## Tests

- Unit: 8 handler files have inline tests (agent, crud, events, graph, intent, memory, session, skills)
- Integration: `tests/cli_test.rs`, `tests/cli_behavior_test.rs`
- Untested: permission, context, messaging, deleted, tool, delta, hybrid handlers
