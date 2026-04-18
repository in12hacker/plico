# Plico Wire Protocol (PWP) v1.0

## Overview

The Plico Wire Protocol (PWP) is the internal data exchange protocol of the Plico AI-OS.
It defines how all components — agents, tools, the kernel, and external adapters —
communicate through a single, type-safe, versioned request/response interface.

**Design principle**: The kernel speaks PWP natively. External protocols (OpenAI, MCP, A2A)
are thin adapters that translate to/from PWP. When an external protocol becomes obsolete,
delete the adapter. When a new one emerges, add one. The kernel never changes.

```
External World
  ├── Cloud APIs (OpenAI, Anthropic, DeepSeek, Groq)
  ├── Local inference (vLLM, SGLang, Ollama, llama.cpp)
  ├── MCP servers/clients (tool integration)
  └── A2A agents (agent collaboration)
         ↕  Protocol Adapters (pluggable, disposable)
PWP (ApiRequest / ApiResponse)
         ↕  Stable internal interfaces
AI Kernel (CAS, Memory, FS, Scheduler, KG)
```

## Transport

PWP is transport-agnostic. Current transports:

| Transport | Format | Use case |
|-----------|--------|----------|
| **In-process** | Rust enum dispatch | CLI (`aicli`), tests |
| **TCP** | JSON over newline-delimited TCP | Daemon (`plicod`) |
| **HTTP** | JSON over HTTP POST | Dashboard, future REST API |

All JSON serialization uses `serde_json` with `#[serde(tag = "method")]` discriminant.

## Request Format

Every request is an `ApiRequest` enum variant, serialized as:

```json
{"method": "<variant_name>", ...fields}
```

The `method` field is the discriminant tag. All other fields are variant-specific.

## Response Format

All responses share a common `ApiResponse` structure:

```json
{
  "ok": true|false,
  "error": "...",           // present only when ok=false
  "cid": "...",             // content ID (create/update operations)
  "node_id": "...",         // KG node ID
  "data": "...",            // raw content (read operations)
  "results": [...],         // search results
  "agents": [...],          // agent listings
  "memory": [...],          // memory recall
  "events": [...],          // event listings
  "nodes": [...],           // KG node listings
  "paths": [[...]],         // KG path results
  "edges": [...],           // KG edge listings
  "tools": [...],           // tool catalog
  "tool_result": {...},     // tool invocation result
  "resolved_intents": [...],// intent resolution results
  "messages": [...],        // agent messages
  "context_data": {...},    // layered context
  "total_count": N,         // pagination: total items
  "has_more": true|false    // pagination: more pages?
}
```

Fields are `null`-omitted (`#[serde(skip_serializing_if = "Option::is_none")]`).
Only fields relevant to the specific operation are populated.

## Content Encoding

Binary-safe payloads use `content_encoding`:

| Value | Description |
|-------|-------------|
| `"utf8"` (default) | Plain UTF-8 string |
| `"base64"` | Base64-encoded (RFC 4648 standard alphabet) |

Use `"base64"` for images, audio, video, or any non-text data.

---

## Operations Reference

### CAS (Content-Addressed Storage)

#### `create` — Store a new object

```json
{
  "method": "create",
  "content": "Hello world",
  "content_encoding": "utf8",
  "tags": ["greeting", "example"],
  "agent_id": "agent1",
  "intent": "store meeting notes"
}
```

Response: `{"ok": true, "cid": "sha256-..."}`

#### `read` — Retrieve object by CID

```json
{"method": "read", "cid": "sha256-...", "agent_id": "agent1"}
```

Response: `{"ok": true, "data": "Hello world", "tags": ["greeting"]}`

#### `search` — Semantic + tag search

```json
{
  "method": "search",
  "query": "meeting notes about project X",
  "agent_id": "agent1",
  "limit": 10,
  "offset": 0,
  "require_tags": ["meeting"],
  "exclude_tags": ["draft"],
  "since": 1713398400000,
  "until": 1713484800000
}
```

Response: `{"ok": true, "results": [{"cid": "...", "relevance": 0.92, "tags": [...]}], "total_count": 42, "has_more": true}`

#### `update` — Replace object content

```json
{
  "method": "update",
  "cid": "sha256-...",
  "content": "Updated content",
  "content_encoding": "utf8",
  "new_tags": ["updated"],
  "agent_id": "agent1"
}
```

Response: `{"ok": true, "cid": "sha256-new..."}`

#### `delete` — Soft-delete object

```json
{"method": "delete", "cid": "sha256-...", "agent_id": "agent1"}
```

#### `list_deleted` — List soft-deleted objects

```json
{"method": "list_deleted", "agent_id": "agent1"}
```

Response: `{"ok": true, "deleted": [{"cid": "...", "deleted_at": 1713398400000, "tags": [...]}]}`

#### `restore` — Restore a soft-deleted object

```json
{"method": "restore", "cid": "sha256-...", "agent_id": "agent1"}
```

### Agent Lifecycle

#### `register_agent` — Create a new agent

```json
{"method": "register_agent", "name": "my-agent"}
```

Response: `{"ok": true, "agent_id": "my-agent"}`

#### `list_agents` — List all agents

```json
{"method": "list_agents"}
```

Response: `{"ok": true, "agents": [{"id": "...", "name": "...", "state": "running"}]}`

#### `agent_status` — Get agent state

```json
{"method": "agent_status", "agent_id": "my-agent"}
```

Response: `{"ok": true, "agent_state": "running", "pending_intents": 3}`

#### `agent_suspend` / `agent_resume` / `agent_terminate` / `agent_complete` / `agent_fail`

```json
{"method": "agent_suspend", "agent_id": "my-agent"}
{"method": "agent_resume", "agent_id": "my-agent"}
{"method": "agent_terminate", "agent_id": "my-agent"}
{"method": "agent_complete", "agent_id": "my-agent"}
{"method": "agent_fail", "agent_id": "my-agent", "reason": "out of memory"}
```

#### `agent_set_resources` — Set resource quotas

```json
{
  "method": "agent_set_resources",
  "agent_id": "target-agent",
  "memory_quota": 104857600,
  "cpu_time_quota": 60000,
  "allowed_tools": ["cas.search", "memory.store"],
  "caller_agent_id": "admin-agent"
}
```

### Memory

#### `remember` — Store a memory

```json
{"method": "remember", "agent_id": "agent1", "content": "User prefers concise responses"}
```

#### `recall` — Retrieve all memories

```json
{"method": "recall", "agent_id": "agent1"}
```

Response: `{"ok": true, "memory": ["User prefers concise responses", ...]}`

#### `memory_move` — Move entry between tiers

```json
{"method": "memory_move", "agent_id": "agent1", "entry_id": "e-123", "target_tier": "long_term"}
```

Tiers: `ephemeral`, `working`, `long_term`, `procedural`

#### `memory_delete` — Delete a memory entry

```json
{"method": "memory_delete", "agent_id": "agent1", "entry_id": "e-123"}
```

#### `evict_expired` — Evict expired entries

```json
{"method": "evict_expired", "agent_id": "agent1"}
```

### Knowledge Graph

#### `add_node` — Create a KG node

```json
{
  "method": "add_node",
  "label": "Project Alpha",
  "node_type": "entity",
  "properties": {"domain": "engineering"},
  "agent_id": "agent1"
}
```

Node types: `entity`, `fact`, `event`, `tag`

Response: `{"ok": true, "node_id": "n-abc123"}`

#### `add_edge` — Create a KG edge

```json
{
  "method": "add_edge",
  "src_id": "n-1",
  "dst_id": "n-2",
  "edge_type": "related_to",
  "weight": 0.8,
  "agent_id": "agent1"
}
```

Edge types: `related_to`, `part_of`, `created_by`, `tagged_with`, `derived_from`, `caused_by`, `references`, `supersedes`, `contains`, `depends_on`, `temporal_next`

#### `get_node` — Get a single node by ID

```json
{"method": "get_node", "node_id": "n-1", "agent_id": "agent1"}
```

#### `list_nodes` — List nodes with optional type filter

```json
{"method": "list_nodes", "node_type": "entity", "agent_id": "agent1", "limit": 20, "offset": 0}
```

#### `list_nodes_at_time` — Temporal node query

```json
{"method": "list_nodes_at_time", "node_type": "fact", "agent_id": "agent1", "t": 1713398400000}
```

#### `list_edges` — List edges, optionally filtered by node

```json
{"method": "list_edges", "agent_id": "agent1", "node_id": "n-1", "limit": 50}
```

#### `find_paths` — Find paths between nodes

```json
{
  "method": "find_paths",
  "src_id": "n-1",
  "dst_id": "n-5",
  "max_depth": 4,
  "weighted": true,
  "agent_id": "agent1"
}
```

Response: `{"ok": true, "paths": [[{"id": "n-1", ...}, {"id": "n-3", ...}, {"id": "n-5", ...}]]}`

#### `update_node` — Update node label/properties

```json
{"method": "update_node", "node_id": "n-1", "label": "New Label", "properties": {"updated": true}, "agent_id": "agent1"}
```

#### `remove_node` / `remove_edge` — Delete graph elements

```json
{"method": "remove_node", "node_id": "n-1", "agent_id": "agent1"}
{"method": "remove_edge", "src_id": "n-1", "dst_id": "n-2", "edge_type": "related_to", "agent_id": "agent1"}
```

#### `explore` — Neighborhood exploration

```json
{"method": "explore", "cid": "n-1", "edge_type": "related_to", "depth": 2, "agent_id": "agent1"}
```

Response: `{"ok": true, "neighbors": [{"node_id": "...", "label": "...", "node_type": "entity", "edge_type": "related_to", "authority_score": 0.85}]}`

### Events (Temporal)

#### `create_event` — Create a temporal event

```json
{
  "method": "create_event",
  "label": "Team standup",
  "event_type": "meeting",
  "start_time": 1713398400000,
  "end_time": 1713400200000,
  "location": "Room 301",
  "tags": ["team", "standup"],
  "agent_id": "agent1"
}
```

#### `list_events` — Query events by time range and tags

```json
{
  "method": "list_events",
  "since": 1713312000000,
  "until": 1713484800000,
  "tags": ["team"],
  "event_type": "meeting",
  "agent_id": "agent1",
  "limit": 20,
  "offset": 0
}
```

#### `list_events_text` — Natural language time query

```json
{
  "method": "list_events_text",
  "time_expression": "last week",
  "tags": [],
  "event_type": null,
  "agent_id": "agent1"
}
```

#### `event_attach` — Link event to another entity

```json
{
  "method": "event_attach",
  "event_id": "n-evt-1",
  "target_id": "sha256-...",
  "relation": "references",
  "agent_id": "agent1"
}
```

### Intent System

#### `submit_intent` — Submit a pending intent

```json
{
  "method": "submit_intent",
  "description": "Summarize all meeting notes from this week",
  "priority": "high",
  "action": "{\"method\":\"search\",\"query\":\"meeting notes\",\"agent_id\":\"agent1\"}",
  "agent_id": "agent1"
}
```

Response: `{"ok": true, "intent_id": "i-abc123"}`

#### `intent_resolve` — Resolve natural language to intent

```json
{"method": "intent_resolve", "text": "find my recent documents", "agent_id": "agent1"}
```

Response: `{"ok": true, "resolved_intents": [{"routing_action": "single_action", "confidence": 0.92, "action": {...}, "explanation": "..."}]}`

### Tools

#### `tool_call` — Invoke a registered tool

```json
{"method": "tool_call", "tool": "cas.search", "params": {"query": "test"}, "agent_id": "agent1"}
```

Response: `{"ok": true, "tool_result": {"output": "...", "error": null}}`

#### `tool_list` — List available tools

```json
{"method": "tool_list", "agent_id": "agent1"}
```

Response: `{"ok": true, "tools": [{"name": "cas.search", "description": "...", "schema": {...}}]}`

#### `tool_describe` — Get tool schema

```json
{"method": "tool_describe", "tool": "cas.search", "agent_id": "agent1"}
```

### Agent Messaging

#### `send_message` — Send a message between agents

```json
{"method": "send_message", "from": "agent1", "to": "agent2", "payload": {"text": "task complete"}}
```

#### `read_messages` — Read inbox

```json
{"method": "read_messages", "agent_id": "agent1", "unread_only": true, "limit": 10}
```

#### `ack_message` — Acknowledge a message

```json
{"method": "ack_message", "agent_id": "agent1", "message_id": "msg-123"}
```

### Context Loading

#### `load_context` — Load content at specified detail layer

```json
{"method": "load_context", "cid": "sha256-...", "layer": "L1", "agent_id": "agent1"}
```

Layers: `L0` (~100 tokens summary), `L1` (~2k tokens), `L2` (full content)

Response: `{"ok": true, "context_data": {"cid": "...", "layer": "L1", "content": "...", "tokens_estimate": 1850}}`

### Temporal Edge History

#### `edge_history` — Query edge version history

```json
{
  "method": "edge_history",
  "src_id": "n-1",
  "dst_id": "n-2",
  "edge_type": "related_to",
  "agent_id": "agent1"
}
```

---

## Pagination

Operations that return lists support pagination via `limit` and `offset`:

```json
{"method": "search", "query": "...", "limit": 10, "offset": 20, "agent_id": "a"}
```

Response includes `total_count` and `has_more` for cursor-based iteration.

## Error Handling

Failed operations return `{"ok": false, "error": "description"}`.
All other response fields are absent on error.

## Versioning

PWP versions are tied to Plico releases. New operations are added as new `ApiRequest` variants.
Existing variants are never removed or modified — only deprecated.
The `method` tag provides forward compatibility: unknown methods return an error response,
allowing older kernels to reject newer requests gracefully.

## External Protocol Adapters

PWP is designed so external protocols map cleanly to/from it:

| External Protocol | Adapter Role | Maps To |
|-------------------|-------------|---------|
| **OpenAI `/v1/chat/completions`** | LLM inference | `LlmProvider` trait |
| **MCP (JSON-RPC)** | Tool integration | `tool_call` / `tool_list` |
| **A2A (Tasks/Artifacts)** | Agent collaboration | `send_message` / `submit_intent` |
| **REST/HTTP** | General API access | Full `ApiRequest` dispatch |

Each adapter is a thin translation layer — typically under 200 lines of code.
The adapter converts external protocol messages to `ApiRequest`, dispatches through
the kernel, and converts `ApiResponse` back to the external format.

### LlmProvider Adapter Pattern

The `LlmProvider` trait (`src/llm/mod.rs`) is the adapter interface for inference:

```rust
pub trait LlmProvider: Send + Sync {
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<String, LlmError>;
    fn model_name(&self) -> &str;
}
```

Current implementations:
- `OllamaProvider` — Ollama `/api/chat` endpoint
- `OpenAICompatibleProvider` — any `/v1/chat/completions` endpoint (vLLM, SGLang, cloud APIs)
- `StubProvider` — deterministic responses for testing

Configuration via environment:
```bash
LLM_BACKEND=openai|ollama|stub
OPENAI_API_BASE=https://api.openai.com/v1   # or http://localhost:8000/v1 for vLLM
OPENAI_API_KEY=sk-...                        # optional for local inference
PLICO_SUMMARIZER_MODEL=gpt-4o               # model for summarization
PLICO_INTENT_MODEL=gpt-4o                   # model for intent routing
```
