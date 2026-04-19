# Plico — AI-Native Operating System

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An operating system designed **entirely from an AI perspective** — no human-first CLI/GUI, no path-centric filesystem as the primary abstraction. Upper-layer agents interact through **semantic APIs** (content, tags, intents, graphs). The stack is **model-agnostic**: embeddings and optional LLM routing can use local backends (Ollama, ONNX) or stubs for tests.

## Status

**Active development — core stack implemented and covered by integration tests.** Implemented areas: CAS, semantic filesystem (vectors + BM25 + knowledge graph), layered memory (4-tier), agent scheduler (dispatch loop + result consumer), kernel event bus (pub/sub with filtering + persisted event log), permission guardrails, natural-language intent routing (heuristic + optional LLM), tool registry (built-in + external via MCP), KG-backed skill discovery, temporal helpers, agent checkpoints, discovery / delegation / quota APIs, LLM provider abstraction (Ollama / OpenAI-compatible / stub), `plicod` (TCP-only daemon), `plico-mcp` (stdio JSON-RPC), and `aicli` (semantic CLI).

Design rationale and philosophy: `system.md` (Chinese).

## Architecture

```
External AI agents / MCP clients
        ↓  semantic JSON (TCP / CLI / MCP stdio)
┌──────────────────────────────────────────────┐
│  AI Kernel                                    │
│  ├─ Agent scheduler + dispatch loop           │
│  ├─ Layered memory + persistence              │
│  ├─ Event bus (typed pub/sub + persisted log) │
│  ├─ Built-in tool registry & execution        │
│  └─ Permission guardrails                     │
│                                               │
│  Intent layer — NL → ApiRequest (optional)    │
│                                               │
│  AI-native "filesystem"                       │
│  ├─ Content-Addressed Storage (CAS)          │
│  ├─ Semantic / hybrid search (vectors, BM25) │
│  ├─ Knowledge graph (PetgraphBackend)        │
│  └─ Layered context loader (L0/L1/L2)       │
└──────────────────────────────────────────────┘
```

`plicod` is **TCP-only** (default `0.0.0.0:7878`). No separate HTTP dashboard — system state is queried via `ApiRequest::SystemStatus` (wire JSON `{"system_status":null}`). From CLI: `aicli system-status`.

## Quick start

```bash
# Build
cargo build --release

# Run tests
cargo test

# AI-friendly CLI (in-process kernel; storage under --root)
cargo run --bin aicli -- --root /tmp/plico put --content "hello" --tags "greeting"
cargo run --bin aicli -- --root /tmp/plico get <CID>
cargo run --bin aicli -- --root /tmp/plico search --query "greeting"

# Same CLI against a running daemon
cargo run --bin aicli -- --tcp 127.0.0.1:7878 search --query "hello"

# Long-running daemon (TCP semantic API + dispatch loop + result consumer)
cargo run --bin plicod -- --port 7878 --root /tmp/plico
# or: PLICO_ROOT=/tmp/plico cargo run --bin plicod

# System health (local kernel mode)
cargo run --bin aicli -- --root /tmp/plico system-status

# MCP adapter (stdio JSON-RPC)
PLICO_ROOT=/tmp/plico cargo run --bin plico-mcp
```

Use `cargo run --bin aicli -- --help` for the full command list (CRUD, search, agents, memory, graph, tools, events, intents, skills, etc.).

## Crate layout

```
src/
├── cas/            # SHA-256 content-addressed object store
├── memory/         # Tiered memory (ephemeral → long-term) + persistence
│   └── layered/    # LayeredMemory core + tests
├── intent/         # Natural language → structured ApiRequest + execution helpers
├── scheduler/      # Agents, priorities, messaging, execution dispatch
├── fs/             # Semantic store: tags, embeddings, graph, context loader
│   ├── semantic_fs/ # SemanticFS core + events + tests
│   ├── embedding/   # EmbeddingProvider (Ollama, local ONNX, stub, JSON-RPC)
│   ├── search/      # SemanticSearch (in-memory, BM25, HNSW)
│   └── graph/       # KnowledgeGraph trait + PetgraphBackend
├── kernel/         # AIKernel — orchestration, tools, persistence, dispatch
│   ├── event_bus.rs # Typed pub/sub + persisted event log
│   └── ops/         # Operation groups (fs, agent, memory, events, graph, …)
├── api/            # ApiRequest / ApiResponse protocol + permission layer
├── tool/           # Tool trait and registry ("everything is a tool")
├── temporal/       # NL time ranges (heuristics + optional LLM)
├── llm/            # LlmProvider trait (Ollama / OpenAI-compatible / stub)
├── mcp/            # MCP client — external tool integration
├── bin/
│   ├── plicod.rs       # Async TCP server (JSON ApiRequest/ApiResponse)
│   ├── plico_mcp.rs    # MCP stdio server (JSON-RPC 2.0)
│   └── aicli/          # Semantic CLI (handlers split by command group)
├── lib.rs
└── main.rs

tests/               # Integration tests (kernel, CLI, FS, memory, search, MCP, intent, permissions, …)
AGENTS.md            # Detailed directory map + navigation for contributors and agents
CLAUDE.md            # Maintainer / agent guidance
```

## Design document

See `system.md` for the full AI-native OS design (in Chinese).
