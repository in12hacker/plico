# Module: api

Personal digital-twin protocol types, authentication, and internal semantic command types.

Status: personal.v2 single-track active | Fan-in: 4 | Fan-out: 1

## Dependents

- `src/kernel/` → permission/auth guardrails, typed public service, internal commands
- `src/client.rs` → public request/response transport after the single-track cutover
- `src/bin/plicod.rs` → TCP/UDS public protocol boundary
- `src/bin/plico_mcp/` and `src/bin/aicli/` → exact public capability consumers

## Modification Risk

- Changing `public/` is a public wire break: update service, client, plicod, MCP, aicli and schema snapshots together.
- Adding a public operation requires an accepted product decision and a real end-to-end handler; internal enum reachability is not evidence of a public capability.
- Changing permission/auth policy affects every transport. Public identities must come from trusted transport context, never payload role/tenant fields.
- `semantic.rs` is legacy internal command debt after cutover. Do not import it from the public service or build an adapter between protocols.

## Task Routing

- Public envelope, limits, operation/result DTOs and catalog → `public/`
- Bearer token resolution → `agent_auth.rs`
- Offline migration credential evidence → `offline_credentials.rs`; pure parsing/verification only, no filesystem access
- Permission model → `permission.rs`
- Legacy internal commands pending deletion/internalization → `semantic.rs`

## Public protocol

`public/` defines `plico.personal.v2` and an exact 14-operation catalog:

- `capabilities.describe`, `runtime.readiness`
- `object.put`, `object.get`, `object.search`
- `memory.create`, `memory.get`, `memory.recall`, `memory.update`, `memory.delete`
- `projection.status`, owner-only `projection.rebuild`
- `session.start`, `session.end`

Inputs deny unknown fields and never accept agent, tenant, tier, scope, importance, checkpoint, or thermal controls. Object search can report the existing CID-keyed BM25 path; Memory recall currently reports only entry-ID lexical overlap. Tenant/organization, cluster, KG mutation, internal control, Memory BM25, thermal/deep recall, and human document projections are not public capabilities.

`projection.status` distinguishes `observed/unreconciled/unavailable`; only observed has one of
Queued/Building/Ready/Failed/Stale/AbsentByPolicy. An authorized typed response may bind a full
canonical content hash, but tracing, metrics, errors and unauthenticated diagnostics must never record it.
The embedding manifest is the sole status source. Memory vector/hybrid/BM25 retrieval remains
unsupported and `memory.recall` remains lexical.
`runtime.readiness` reports projection `control_plane` and `worker` separately. Provider drift keeps
the verified control plane readable while marking the worker restart-required; canonical/lexical
availability therefore remains independent of embedding execution.

## Files

| File | Purpose |
|---|---|
| `public/mod.rs` | protocol identity, operation catalog and hard limits |
| `public/input.rs` | typed request envelope and validated inputs |
| `public/output.rs` | typed data/error response schema |
| `agent_auth.rs` | bearer issuance, persistence and constant-time role resolution |
| `offline_credentials.rs` | Feature-gated exact credential parsing, constant-time owner proof and redacted role-cutoff hash |
| `permission.rs` | internal fine-grained permission guard |
| `semantic.rs` | legacy internal commands; must leave external transports during cutover |
| `dto.rs`, `version.rs` | legacy DTO/version types pending call-graph cleanup |

## Interface contract

- TCP authenticates a bounded bearer and resolves a local role before domain dispatch.
- Owner-only UDS and Embedded clients inject trusted local context and reject self-reported role/tenant identity.
- Transport/framing failures are client errors, not domain responses.
- Success responses contain exactly one matching typed result; failures contain exactly one typed error.
- Request-level tracing carries request ID, operation, transport and authenticated role; never bearer, content, full query, provider raw errors or private host paths.
- Removed methods receive no compatibility alias or translation layer.

## Tests

- Unit schema/limit/roundtrip tests: `src/api/public/tests.rs`
- Auth and permission tests: `src/api/agent_auth.rs`, `tests/permission_test.rs`
- Protocol gate: exact 14-operation parity across catalog, service, TCP/UDS, MCP and aicli; personal.v1 and all old wire methods are rejected without state change.
