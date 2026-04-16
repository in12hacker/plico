# Module: api

AI-friendly semantic API — permission guardrails + JSON protocol types for TCP daemon and CLI.

Status: stable | Fan-in: 3 | Fan-out: 1

## Dependents (Fan-in: 3)

- `src/kernel/mod.rs` → PermissionGuard, PermissionContext, PermissionAction; DashboardStatus and related types
- `src/bin/plicod.rs` → ApiRequest, ApiResponse, SearchResultDto, AgentDto, NeighborDto, decode_content
- `src/bin/aicli.rs` → ApiRequest, ApiResponse, SearchResultDto (TCP mode)

## Modification Risk

- Add `ApiRequest` variant → compatible, add dispatch in plicod.rs
- Change `ApiResponse` fields → BREAKING, update all response construction sites
- Change `PermissionAction` variants → BREAKING, update all check() calls
- Change `PermissionGuard` policy → behavioral change, affects all agent access

## Task Routing

- Add new API method → modify `src/api/semantic.rs` ApiRequest + ApiResponse, then `src/bin/plicod.rs` dispatch
- Change permission model → modify `src/api/permission.rs`
- Add API response field → modify `src/api/semantic.rs` ApiResponse
- Fix content encoding → modify `src/api/semantic.rs` decode_content

## Public API

### Permission

| Export | File | Description |
|--------|------|-------------|
| `PermissionGuard` | `permission.rs` | Global access control registry |
| `PermissionContext` | `permission.rs` | Per-request agent identity + embedded grants |
| `PermissionAction` | `permission.rs` | Read/Write/ReadAny/Delete/Network/Execute/SendMessage/All |
| `PermissionGrant` | `permission.rs` | Grant with optional scope + expiry |

### Protocol

| Export | File | Description |
|--------|------|-------------|
| `ApiRequest` | `semantic.rs` | Tagged enum of all API operations (create/read/search/etc.) |
| `ApiResponse` | `semantic.rs` | Unified response with optional fields |
| `ContentEncoding` | `semantic.rs` | UTF-8 or Base64 encoding for binary payloads |
| `decode_content` | `semantic.rs` | Decode content string by encoding type |
| `SearchResultDto` | `semantic.rs` | Search result DTO for API responses |
| `DashboardStatus` | `semantic.rs` | ⚠ Development dashboard types (soul violation) |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `permission.rs` | ~229 | PermissionGuard, fine-grained access control |
| `semantic.rs` | ~654 | ApiRequest/ApiResponse, protocol types, dashboard types |
| `mod.rs` | ~7 | Re-exports |

## Dependencies (Fan-out: 1)

- `src/fs/` — imports EventType, EventRelation, EventSummary, UserFact, ActionSuggestion for API type definitions

## Interface Contract

- `PermissionGuard::check()`: returns `Ok(())` if allowed, `Err(PermissionDenied)` if denied
- Default policy: Read + Write allowed by default; Delete/Network/Execute require explicit grant
- Trusted agents ("kernel", "system") bypass all checks
- `PermissionGrant` supports optional scope restriction and expiry timestamp
- Thread safety: `PermissionGuard` is NOT internally synchronized — caller must manage mutability

## Tests

- Unit: `src/api/semantic.rs` mod tests (encoding roundtrips), `src/api/permission.rs` (implicit via integration)
- Integration: `tests/permission_test.rs`
- Critical: `test_default_policy`, `test_grant_and_check`, `test_trusted_agent_bypass`
