# 太初 (Plico) — AI-Native Operating System Kernel

**Languages / 语言：** [English](README.md) · [简体中文](README_zh.md)

An operating system kernel designed **entirely from an AI perspective**. No human-first CLI/GUI, no path-centric filesystem. Upper-layer agents interact through **semantic APIs** (content, tags, intents, graphs). The stack is **model-agnostic**: embeddings and optional LLM routing can use local backends (Ollama, ONNX) or stubs for tests.

"太初" means "Genesis / In the Beginning" — the primordial state where an AI-OS becomes self-aware.

## Status

**Genesis (Node 25) — 131 source files, 49,489 lines of Rust, 1,388 tests (0 failures).**

Core stack: CAS, semantic filesystem (vectors + BM25 + knowledge graph with redb), layered memory (4-tier + MemoryScope), agent scheduler, kernel event bus (pub/sub + filtering + persistent log), permission guardrails, hook system (5 interception points), intent system (DAG decomposition + autonomous execution), context budget engine (L0/L1/L2), tool registry (37 built-in + external MCP), agent lifecycle (checkpoint/restore/discover/delegate), learning loop (execution stats + skill discovery + self-healing), `plicod` (TCP+UDS daemon), `plico-mcp` (stdio JSON-RPC), and `aicli` (semantic CLI).

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
│  ├─ Knowledge graph (redb, 14 edge types)         │
│  └─ Layered context loader (L0/L1/L2)             │
└────────────────────────────────────────────────────┘
```

**Daemon-First**: `plicod` hosts the kernel. Clients connect via UDS or TCP using length-prefixed JSON framing. `--embedded` mode available for testing.

## Quick start

```bash
# Build
cargo build --release

# Run all tests (1,388 tests)
cargo test

# Start the daemon (recommended)
cargo run --bin plicod -- --port 7878

# CLI (connects to daemon by default)
aicli agent --name my-agent
aicli put --content "knowledge about Plico architecture" --tags "plico,arch"
aicli search "architecture"
aicli remember --content "important insight" --tier working --agent my-agent
aicli recall --agent my-agent

# CLI in embedded mode (no daemon needed)
aicli --embedded put --content "hello" --tags "test"

# MCP adapter (stdio JSON-RPC 2.0)
cargo run --bin plico-mcp
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
| 8 | **Causation before correlation** | KG records CausedBy chains |
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
│   ├── embedding/  # EmbeddingProvider (Ollama, local ONNX, stub)
│   ├── search/     # SemanticSearch (BM25, HNSW)
│   └── graph/      # KnowledgeGraph (redb backend, 14 edge types)
├── kernel/         # AIKernel — orchestration, tools, hooks, persistence
│   ├── hook.rs     # Hook registry (5 interception points)
│   ├── event_bus.rs # Typed pub/sub + persistent event log
│   └── ops/        # 24 operation modules
├── api/            # ApiRequest / ApiResponse + permission + auth
├── tool/           # Tool trait and registry ("everything is a tool")
├── llm/            # LlmProvider trait (Ollama / OpenAI-compatible / stub)
├── mcp/            # MCP client — external tool integration
├── client.rs       # KernelClient trait (Embedded / UDS / TCP)
└── bin/
    ├── plicod.rs       # Daemon (TCP + UDS, length-prefixed JSON)
    ├── plico_mcp/      # MCP stdio server (JSON-RPC 2.0)
    └── aicli/          # Semantic CLI (daemon-first, --embedded fallback)

tests/              # 33 integration test files
docs/
├── genesis-reference.md    # Complete reference document
├── genesis-audit-n25*.md   # Audit reports
└── design-node*.md         # 24 design documents (N2-N25)
```

## Design documents

- `system-v2.md` — Soul 2.0: 10 axioms from AI's first-person perspective (Chinese)
- `docs/genesis-reference.md` — Complete Genesis reference (Chinese)
- `AGENTS.md` — Detailed directory map + navigation for AI agents
