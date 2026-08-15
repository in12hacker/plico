# 太初 (Plico) — AI-Native Operating System Kernel

[![CI](https://github.com/in12hacker/plico/actions/workflows/ci.yml/badge.svg)](https://github.com/in12hacker/plico/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021-edition.svg)](https://www.rust-lang.org)

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An AI-native kernel for a **personal digital twin**. Canonical objects and memories are the primary
data; documents, spreadsheets, slides, and graphical interfaces are projections generated when a
person needs them. AI clients use the typed `plico.personal.v2`
memory/object/session/projection API rather than path-centric files or internal kernel controls. The
inference stack remains provider-independent.

"太初" means "Genesis / In the Beginning" — the primordial state where an AI-OS becomes self-aware.

## Status

Active development. Current verification evidence is reported per change rather than kept as a hand-maintained counter here.

Core stack: CAS, semantic filesystem (vectors + BM25 + knowledge graph), layered memory, agent scheduler, kernel event bus, permission guardrails, intent and context-budget systems, retrieval fusion, model-independent inference providers, `plicod`, `plico-mcp`, and `aicli`.

## Architecture

```
Personal AI clients / MCP clients
        ↓  semantic JSON
┌────────────────────────────────────────────────────┐
│  Interface Adapters                                │
│          ┌─────────┐  ┌───────────┐                 │
│          │  aicli  │  │ plico-mcp │                 │
│          └────┬────┘  └─────┬─────┘                 │
│               └─────────────┤                       │
│               ┌───────▼────────┐                    │
│               │  KernelClient  │ (UDS / TCP / embed) │
│               └───────┬────────┘                    │
├───────────────────────┼────────────────────────────┤
│  AI Kernel            │                             │
│  ├─ Typed personal-vault public service (14 ops)   │
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

**One public contract**: `plicod`, `KernelClient`, `plico-mcp`, and `aicli` use the same 14 typed
operations: `capabilities.describe`, `runtime.readiness`, `object.put/get/search`,
`memory.create/get/recall/update/delete`, `projection.status/rebuild`, and `session.start/end`. UDS,
MCP, and embedded mode are trusted local-owner paths. TCP requires a bearer and defaults to loopback.
`projection.rebuild` is owner-only. The memory embedding control plane is supported, while Memory
vector/hybrid/BM25 retrieval remains unsupported; `memory.recall` is still lexical.

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

# Start daemon (recommended — binds 127.0.0.1:7878 by default).
# First start creates/reuses the personal-owner credential in the 0600
# agent_tokens.json under PLICO_ROOT; no token is printed or logged.
cargo run --bin plicod -- start

# Daemon lifecycle
cargo run --bin plicod -- stop       # graceful shutdown
cargo run --bin plicod -- status     # JSON status output

# CLI (UDS by default; operation names exactly match the public protocol)
cargo run --bin aicli -- capabilities.describe
cargo run --bin aicli -- object.put --content "knowledge about Plico" --tag plico --tag architecture
cargo run --bin aicli -- object.search --query "architecture"
cargo run --bin aicli -- memory.create --content "important insight" --tag insight
cargo run --bin aicli -- memory.recall --query "insight"
cargo run --bin aicli -- projection.status --revision-id UUID
cargo run --bin aicli -- projection.rebuild --all-eligible

# CLI in embedded mode (no daemon needed)
cargo run --bin aicli -- --embedded object.put --content "hello" --tag test

# TCP: read the owner-only credential locally; do not paste it into arguments/logs.
export PLICO_ROOT="${PLICO_ROOT:-$HOME/.plico}"
export PLICO_BEARER_TOKEN="$(jq -r '.\"personal-owner\".token' "$PLICO_ROOT/agent_tokens.json")"
cargo run --bin aicli -- --tcp 127.0.0.1:7878 runtime.readiness

# MCP adapter (stdio JSON-RPC 2.0)
cargo run --bin plico-mcp
```

## Inference Backend Configuration

Embedding and LLM backends are **inference-framework-agnostic** at the Object/chat adapter boundary.
Memory embedding projection is stricter: today only the Ollama path proves the immutable provider/model
identity required to publish a P3 builder. An arbitrary OpenAI-compatible endpoint must not be presented as
a verified Memory projection provider.

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

### Local serving decision

The current GB10 baseline keeps llama.cpp as the reproducible local text control, with Qwen2.5-7B Q4_K_M
as the latency tier and Qwen3.5-27B Q4_K_M as the larger local tier. Ollama remains the operational path for
the verified Qwen3-Embedding Memory builder. The next performance trials are TensorRT-LLM and then vLLM,
using the same pinned upstream GPT-OSS-20B checkpoint and tokenizer while sealing each runtime's own format,
quantization, and artifact digest. TensorRT Edge-LLM follows with a pinned supported Qwen checkpoint because
its current model matrix and ONNX/engine workflow differ. No framework replaces llama.cpp until it passes the
same quality, TTFT, throughput, p95, memory, and failure-rate checks. VLMs are reserved for image-bearing suites
and are not a default text or embedding backend.

Measured host snapshots, limitations, source links, and the acceptance matrix are frozen in
[benchmarks/README.md](benchmarks/README.md#本地推理选型2026-08-15-冻结).

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
| 4 | **Canonical before projection** | Personal memory/CAS are primary; human files are generated views |
| 5 | **Mechanism, not policy** | Kernel provides primitives, never decides for agents |
| 6 | **Structure before language** | JSON is the only kernel interface |
| 7 | **Proactive before reactive** | Intent prefetch, warm context, goal generation |
| 8 | **Causation before correlation** | KG records CausedBy / DependsOn / Produces chains |
| 9 | **Better with use** | AgentProfile accumulates, skills discovered |
| 10 | **Sessions are first-class** | durable session start/end and monotonic event watermarks |

## Crate Layout

```
src/
├── cas/                 # SHA-256 content-addressed object store
├── memory/              # Tiered memory (ephemeral → long-term) + persistence
├── intent/              # Internal NL intent routing; not the public wire protocol
├── scheduler/           # Agents, priorities, messaging, execution dispatch
├── fs/                  # Semantic store: tags, embeddings, graph, context loader
│   ├── embedding/       # EmbeddingProvider (OpenAI-compatible, Ollama, local worker, stub)
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
├── api/                 # plico.personal.v2 typed protocol + internal legacy commands
├── tool/                # Tool trait and registry ("everything is a tool")
├── temporal/            # Temporal reasoning (NL time → time ranges)
├── llm/                 # LlmProvider trait (OpenAI-compatible / Ollama / stub)
├── mcp/                 # MCP client — external tool integration
├── client.rs            # KernelClient trait (Embedded / UDS / TCP)
└── bin/
    ├── plicod.rs        # Daemon (TCP + UDS, start/stop/status lifecycle, PID file)
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
