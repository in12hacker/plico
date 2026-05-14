# 太初 (Plico) — AI-Native Operating System Kernel

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An operating system kernel designed **entirely from an AI perspective**. No human-first CLI/GUI, no path-centric filesystem. Upper-layer agents interact through **semantic APIs** (content, tags, intents, graphs). The stack is **inference-framework-agnostic**: both embedding and LLM backends support any server exposing an OpenAI-compatible API (llama.cpp, vLLM, SGLang, TensorRT-LLM, Ollama, etc.), plus local ONNX or stubs for tests.

"太初" means "Genesis / In the Beginning" — the primordial state where an AI-OS becomes self-aware.

## Status

**v46 — 212 source files, 85,116 lines of Rust, 2,075 unit tests (0 failures), 44 integration test files.**

Core stack: CAS, semantic filesystem (vectors + BM25 + knowledge graph with redb, 17 edge types), layered memory (4-tier + MemoryScope), agent scheduler, kernel event bus (pub/sub + filtering + persistent log), permission guardrails, hook system (5 interception points), intent system (DAG decomposition + autonomous execution), context budget engine (L0/L1/L2), tool registry (37 built-in + external MCP), agent lifecycle (checkpoint/restore/discover/delegate), learning loop (execution stats + skill discovery + self-healing), retrieval fusion engine (RFE, 7-signal adaptive ranking), unified configuration (`config.json` + env vars + CLI), `plicod` (TCP+UDS daemon with `start/stop/status` lifecycle), `plico-sse` (A2A SSE adapter), `plico-mcp` (stdio JSON-RPC), and `aicli` (semantic CLI).

Soul 3.0 alignment: **94.7%**. Architecture red lines: **9/9 (100%)**.

## Architecture

```
External AI agents / MCP clients
        ↓  semantic JSON
┌────────────────────────────────────────────────────┐
│  Interface Adapters                                │
│  ┌─────────┐  ┌───────────┐  ┌──────────┐         │
│  │  aicli   │  │ plico-mcp │  │ plico-sse│         │
│  └────┬─────┘  └─────┬─────┘  └────┬─────┘         │
│       └───────────────┼─────────────┘               │
│               ┌───────▼────────┐                    │
│               │  KernelClient  │ (UDS / TCP / embed) │
│               └───────┬────────┘                    │
├───────────────────────┼────────────────────────────┤
│  AI Kernel            │                             │
│  ├─ Agent scheduler + dispatch loop                │
│  ├─ Layered memory (4-tier + MemoryScope)          │
│  ├─ Event bus (typed pub/sub + persistent log)     │
│  ├─ Hook system (5 interception points)            │
│  ├─ Intent system (DAG decomposition + executor)   │
│  ├─ Context budget engine (L0/L1/L2)              │
│  ├─ Built-in tool registry (37 tools)             │
│  ├─ Cognitive engine (Soul v3.0)                   │
│  └─ Permission guardrails + agent auth (HMAC)     │
├────────────────────────────────────────────────────┤
│  AI-Native File System                             │
│  ├─ Content-Addressed Storage (CAS, SHA-256)      │
│  ├─ Semantic search (BM25 + HNSW vectors)         │
│  ├─ Knowledge graph (redb, 17 edge types)         │
│  └─ Layered context loader (L0/L1/L2)             │
└────────────────────────────────────────────────────┘
```

**Daemon-First**: `plicod` hosts the kernel with `start/stop/status` lifecycle commands and PID-file multi-instance protection. Clients connect via UDS or TCP using length-prefixed JSON framing. `--embedded` mode available for testing.

## Quick Start

```bash
# Build
cargo build --release

# Run tests (stub backend, no external dependencies)
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# Run only lib tests (fastest, ~2s)
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# Coverage measurement
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# Clippy (zero warnings required)
cargo clippy -- -D warnings

# Start daemon (recommended — binds 127.0.0.1:7878 by default)
cargo run --bin plicod -- start
cargo run --bin plicod -- start --host 0.0.0.0 --port 9000

# Daemon lifecycle
cargo run --bin plicod -- stop       # graceful shutdown
cargo run --bin plicod -- status     # JSON status output

# CLI (connects to daemon by default)
cargo run --bin aicli -- agent --name my-agent
cargo run --bin aicli -- put --content "knowledge about Plico architecture" --tags "plico,arch"
cargo run --bin aicli -- search "architecture"
cargo run --bin aicli -- remember --content "important insight" --tier working --agent my-agent
cargo run --bin aicli -- recall --agent my-agent

# CLI in embedded mode (no daemon needed)
cargo run --bin aicli -- --embedded put --content "hello" --tags "test"

# SSE adapter (A2A protocol, binds 127.0.0.1:7879 by default)
cargo run --bin plico-sse

# MCP adapter (stdio JSON-RPC 2.0)
cargo run --bin plico-mcp
```

## Inference Backend Configuration

Embedding and LLM backends are **inference-framework-agnostic**. Any server exposing an OpenAI-compatible `/v1/embeddings` or `/v1/chat/completions` endpoint works.

**Defaults (auto-detect llama-server port, fallback :8080):**
- `LLM_BACKEND=llama` → auto-detected llama-server URL
- `EMBEDDING_BACKEND=openai` → same auto-detected URL
- Model: `qwen2.5-coder-7b-instruct` (override via `LLAMA_MODEL`)

URL resolution priority: `LLAMA_URL` env > `OPENAI_API_BASE` env > `~/.plico/llama.url` file > auto-detect from `ps` > `:8080` fallback.

```bash
# For unit tests: stub backend (no external service)
export EMBEDDING_BACKEND=stub
export LLM_BACKEND=stub
```

## Configuration

Plico uses a three-layer cascade (lowest → highest priority):

1. **Built-in defaults** — zero-config works out of the box
2. **Config file** — `~/.plico/config.json` (or `$PLICO_ROOT/config.json`)
3. **Environment variables** — `PLICO_HOST`, `PLICO_DAEMON_PORT`, `EMBEDDING_BACKEND`, etc.
4. **CLI flags** — `--host`, `--port`, `--root` (highest priority)

## 10 Axioms (Soul 3.0)

| # | Axiom | Implication |
|---|-------|-------------|
| 1 | **Token is the scarcest resource** | Layered return L0/L1/L2, track consumption |
| 2 | **Intent before operation** | Agent declares intent, OS assembles context |
| 3 | **Memory crosses boundaries** | 4-tier memory, checkpoint/restore across "death" |
| 4 | **Sharing before duplication** | MemoryScope: Private / Shared / Group |
| 5 | **Mechanism, not policy** | Kernel provides primitives, never decides for agents |
| 6 | **Structure before language** | JSON is the only kernel interface |
| 7 | **Proactive before reactive** | Intent prefetch, warm context, goal generation |
| 8 | **Causation before correlation** | KG records CausedBy / DependsOn / Produces chains |
| 9 | **Better with use** | AgentProfile accumulates, skills discovered |
| 10 | **Sessions are first-class** | session-start/end, warm_context, delta tracking |

## Crate Layout

```
src/
├── cas/                 # SHA-256 content-addressed object store
├── memory/              # Tiered memory (ephemeral → long-term) + persistence
├── intent/              # NL → structured ApiRequest (interface layer, NOT kernel)
├── scheduler/           # Agents, priorities, messaging, execution dispatch
├── fs/                  # Semantic store: tags, embeddings, graph, context loader
│   ├── embedding/       # EmbeddingProvider (OpenAI-compatible, Ollama, ONNX, stub)
│   ├── search/          # SemanticSearch (BM25, HNSW)
│   ├── graph/           # KnowledgeGraph (redb backend, 17 edge types)
│   ├── semantic_fs/     # Core CRUD + event storage
│   ├── query_decompose.rs # Query decomposition engine
│   └── retrieval_router.rs # Intent-routed retrieval
├── kernel/              # AIKernel — orchestration, tools, hooks, persistence
│   ├── cognition/       # Soul v3.0 cognitive engine (12 files)
│   ├── handlers/        # 14 domain handler modules
│   ├── tools/           # 7 built-in tool handlers
│   ├── hook.rs          # Hook registry (5 interception points)
│   ├── event_bus.rs     # Typed pub/sub + persistent event log
│   └── ops/             # 24 operation modules
├── api/                 # ApiRequest / ApiResponse + permission + auth
├── tool/                # Tool trait and registry ("everything is a tool")
├── temporal/            # Temporal reasoning (NL time → time ranges)
├── llm/                 # LlmProvider trait (OpenAI-compatible / Ollama / stub)
├── mcp/                 # MCP client — external tool integration
├── client.rs            # KernelClient trait (Embedded / UDS / TCP)
└── bin/
    ├── plicod.rs        # Daemon (TCP + UDS, start/stop/status lifecycle, PID file)
    ├── plico_sse.rs     # SSE adapter (A2A protocol)
    ├── plico_mcp/       # MCP stdio server (JSON-RPC 2.0)
    └── aicli/           # Semantic CLI (daemon-first, --embedded fallback)

tests/                   # 44 integration test files
benchmarks/              # Custom benchmark framework (Python, uv)
docs/
├── genesis-reference.md # Complete reference document
├── milestones/          # Milestone documents with template
├── plans/               # Active plans
└── design/              # Architecture design documents
```

## Development

This project follows a **milestone-driven development workflow** with strict quality gates:

1. **Milestone planning** — `docs/milestones/TEMPLATE.md`
2. **Module development** — per-module with tests
3. **Quality gates** — `cargo test` + `cargo llvm-cov --lib` ≥ 90% + `cargo clippy` zero warnings
4. **Regression detection** — `tests/perf_regression.rs` (P50/P95 thresholds)
5. **E2E validation** — benchmark suite (`benchmarks/`)

See `CLAUDE.md` for detailed development workflow rules.

## Design Documents

- `system-v3.md` — Soul 3.0: 10 axioms from AI's first-person perspective (Chinese)
- `docs/genesis-reference.md` — Complete Genesis reference (Chinese)
- `AGENTS.md` — AI agent navigation (directory map + quick navigation)
- `CLAUDE.md` — Project-level rules for AI coding assistants
- `benchmarks/README.md` — Benchmark framework documentation
