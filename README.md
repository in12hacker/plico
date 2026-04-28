# 太初 (Plico) — AI-Native Operating System Kernel

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An operating system kernel designed **entirely from an AI perspective**. No human-first CLI/GUI, no path-centric filesystem. Upper-layer agents interact through **semantic APIs** (content, tags, intents, graphs). The stack is **inference-framework-agnostic**: both embedding and LLM backends support any server exposing an OpenAI-compatible API (llama.cpp, vLLM, SGLang, TensorRT-LLM, Ollama, etc.), plus local ONNX or stubs for tests.

"太初" means "Genesis / In the Beginning" — the primordial state where an AI-OS becomes self-aware.

## Status

**Genesis (Node 31) — 191 source files, 62,176 lines of Rust, 1,035+ unit tests (0 failures).**

Core stack: CAS, semantic filesystem (vectors + BM25 + knowledge graph with redb, 17 edge types), layered memory (4-tier + MemoryScope), agent scheduler, kernel event bus (pub/sub + filtering + persistent log), permission guardrails, hook system (5 interception points), intent system (DAG decomposition + autonomous execution), context budget engine (L0/L1/L2), tool registry (37 built-in + external MCP), agent lifecycle (checkpoint/restore/discover/delegate), learning loop (execution stats + skill discovery + self-healing), retrieval fusion engine (RFE, 7-signal adaptive ranking), unified configuration (`config.json` + env vars + CLI), `plicod` (TCP+UDS daemon with `start/stop/status` lifecycle), `plico-sse` (A2A SSE adapter), `plico-mcp` (stdio JSON-RPC), and `aicli` (semantic CLI).

Soul 2.0 alignment: **94.7%**. Architecture red lines: **8/8 (100%)**.

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

## Quick start

```bash
# Build
cargo build --release

# Run all tests
cargo test

# Start the daemon (recommended — binds 127.0.0.1:7878 by default)
cargo run --bin plicod -- start
cargo run --bin plicod -- start --host 0.0.0.0 --port 9000  # custom bind

# Daemon lifecycle
cargo run --bin plicod -- stop       # graceful shutdown
cargo run --bin plicod -- status     # JSON status output

# CLI (connects to daemon by default)
aicli agent --name my-agent
aicli put --content "knowledge about Plico architecture" --tags "plico,arch"
aicli search "architecture"
aicli remember --content "important insight" --tier working --agent my-agent
aicli recall --agent my-agent

# CLI in embedded mode (no daemon needed)
aicli --embedded put --content "hello" --tags "test"

# SSE adapter (A2A protocol, binds 127.0.0.1:7879 by default)
cargo run --bin plico-sse
cargo run --bin plico-sse -- --host 0.0.0.0 --port 9000  # custom bind

# MCP adapter (stdio JSON-RPC 2.0)
cargo run --bin plico-mcp
```

## Configuration

Plico uses a three-layer cascade (lowest → highest priority):

1. **Built-in defaults** — zero-config works out of the box
2. **Config file** — `~/.plico/config.json` (or `$PLICO_ROOT/config.json`)
3. **Environment variables** — `PLICO_HOST`, `PLICO_DAEMON_PORT`, `EMBEDDING_BACKEND`, etc.
4. **CLI flags** — `--host`, `--port`, `--root` (highest priority)

```bash
# Generate default config
cargo run --bin plicod -- start  # creates ~/.plico/ if needed

# Override via environment
PLICO_HOST=0.0.0.0 PLICO_DAEMON_PORT=9000 cargo run --bin plicod -- start

# Override via config file (~/.plico/config.json)
cat > ~/.plico/config.json <<EOF
{
  "network": { "host": "127.0.0.1", "daemon_port": 7878, "sse_port": 7879 },
  "inference": { "embedding_backend": "openai", "llm_backend": "llama" },
  "tuning": { "persist_interval_secs": 300 }
}
EOF
```

## 10 Axioms (Soul 2.0)

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

## Crate layout

```
src/
├── cas/            # SHA-256 content-addressed object store
├── memory/         # Tiered memory (ephemeral → long-term) + persistence
├── intent/         # NL → structured ApiRequest (interface layer, NOT kernel)
├── scheduler/      # Agents, priorities, messaging, execution dispatch
├── fs/             # Semantic store: tags, embeddings, graph, context loader
│   ├── embedding/  # EmbeddingProvider (OpenAI-compatible, Ollama, ONNX, stub)
│   ├── search/     # SemanticSearch (BM25, HNSW)
│   └── graph/      # KnowledgeGraph (redb backend, 17 edge types)
├── kernel/         # AIKernel — orchestration, tools, hooks, persistence
│   ├── hook.rs     # Hook registry (5 interception points)
│   ├── event_bus.rs # Typed pub/sub + persistent event log
│   └── ops/        # 24 operation modules
├── api/            # ApiRequest / ApiResponse + permission + auth
├── tool/           # Tool trait and registry ("everything is a tool")
├── llm/            # LlmProvider trait (OpenAI-compatible / Ollama / llama.cpp / stub)
├── mcp/            # MCP client — external tool integration
├── config.rs       # Unified configuration (3-layer cascade)
├── client.rs       # KernelClient trait (Embedded / UDS / TCP)
└── bin/
    ├── plicod.rs       # Daemon (TCP + UDS, start/stop/status lifecycle, PID file)
    ├── plico_sse.rs    # SSE adapter (A2A protocol)
    ├── plico_mcp/      # MCP stdio server (JSON-RPC 2.0)
    └── aicli/          # Semantic CLI (daemon-first, --embedded fallback)

tests/              # 39 integration test files
docs/
├── genesis-reference.md    # Complete reference document
├── plico-v*-audit*.md      # Audit reports
└── design-node*.md         # Design documents
```

## Design documents

- `system-v2.md` — Soul 2.0: 10 axioms from AI's first-person perspective (Chinese)
- `docs/genesis-reference.md` — Complete Genesis reference (Chinese)
- `AGENTS.md` — Detailed directory map + navigation for AI agents
