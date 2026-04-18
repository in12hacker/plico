# Plico — AI-Native Operating System

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An operating system designed **entirely from an AI perspective** — no human-first CLI/GUI, no path-centric filesystem as the primary abstraction. Upper-layer agents interact through **semantic APIs** (content, tags, intents, graphs). The stack is **model-agnostic**: embeddings and optional LLM routing can use local backends (for example Ollama) or stubs for tests.

## Status

**Active development — core stack is implemented and exercised by integration tests.** Foundations include CAS, semantic filesystem (vectors + knowledge graph), layered memory, agent scheduler with dispatch loop, permission guardrails, natural-language **intent routing** (heuristic + optional LLM), tool registry, temporal helpers, a **TCP daemon**, an **MCP server** (stdio JSON-RPC for editors/agents), and the **`aicli`** semantic CLI.

Design rationale and philosophy remain in `system.md` (Chinese).

## Architecture

```
External AI agents / MCP clients
        ↓  semantic JSON (TCP / CLI / MCP)
┌─────────────────────────────────────────────┐
│  AI Kernel                                   │
│  ├─ Agent scheduler + dispatch loop          │
│  ├─ Layered memory + persistence hooks       │
│  ├─ Built-in tool registry & execution       │
│  └─ Permission guardrails                    │
│                                              │
│  Intent layer — NL → ApiRequest (optional)   │
│                                              │
│  AI-native “filesystem”                      │
│  ├─ Content-Addressed Storage (CAS)        │
│  ├─ Semantic / hybrid search (vectors, BM25)│
│  ├─ Knowledge graph                          │
│  └─ Layered context loader (L0/L1/L2)      │
└─────────────────────────────────────────────┘
```

`plicod` also serves a small **HTTP dashboard** (default `http://127.0.0.1:7879`, see daemon output) alongside the main **TCP JSON** line protocol (default port **7878**).

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

# Same CLI against a running daemon (storage is whatever plicod was started with)
cargo run --bin aicli -- --tcp 127.0.0.1:7878 search --query "hello"

# Long-running daemon (TCP API + dispatch loop + dashboard)
cargo run --bin plicod -- --port 7878 --root /tmp/plico
# or: PLICO_ROOT=/tmp/plico cargo run --bin plicod

# MCP adapter (stdio); point PLICO_ROOT at the same store as the kernel when needed
PLICO_ROOT=/tmp/plico cargo run --bin plico-mcp
```

Use `cargo run --bin aicli -- --help` for the full command list (CRUD, search, agents, memory, graph, tools, events, intents, etc.).

## Crate layout

```
src/
├── cas/          # SHA-256 content-addressed object store
├── memory/       # Tiered memory (ephemeral → long-term) + persistence
├── intent/       # Natural language → structured ApiRequest
├── scheduler/    # Agents, priorities, messaging, execution dispatch
├── fs/           # Semantic store: tags, embeddings, graph, context loader
├── kernel/       # AIKernel — orchestration, tools, persistence, dispatch
├── api/          # ApiRequest / ApiResponse protocol + permission layer
├── tool/         # Tool trait and registry (“everything is a tool”)
├── temporal/     # NL time ranges (heuristics + optional LLM)
├── llm/          # Shared LLM client helpers
├── mcp/          # MCP-oriented helpers used by the plico-mcp binary
├── bin/
│   ├── plicod.rs      # Async TCP server + HTTP dashboard
│   ├── plico_mcp.rs   # MCP stdio server
│   └── aicli/         # Semantic CLI implementation
├── lib.rs
└── main.rs         # Stub — use plicod / aicli / plico-mcp binaries

tests/              # Integration tests (kernel, CLI, FS, memory, MCP, intent, …)
AGENTS.md           # Detailed directory map for contributors and agents
CLAUDE.md           # Maintainer / agent guidance
```

## Design document

See `system.md` for the full AI-native OS design (in Chinese).
