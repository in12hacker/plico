# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## AI Navigation

When creating or updating `AGENTS.md`, `INDEX.md`, or any project navigation index, use the **ariadne-thread** skill. All module directories have `INDEX.md` (L1) with public API, dependencies, dependents, task routing, and modification risk. See `AGENTS.md` for full project structure.

## Project Overview

**Plico** is an AI-native operating system designed from scratch exclusively for AI agents — no human CLI/GUI, no human filesystem paths. All data management (files, images, audio, video) is performed by AI through AI-friendly semantic APIs. The system is model-agnostic and does not depend on any specific AI or agent.

The design document is in `system.md` (written in Chinese).

## Architecture

The system is designed as four layers:

```
Application Layer (AI Agent Ecosystem)
        ↓
AI-Friendly Interface Layer (Semantic API/CLI, Natural Language Interface)
        ↓
AI Kernel Layer
  ├─ Agent Scheduler      — lifecycle management (create/pause/resume/destroy)
  ├─ Layered Memory       — Ephemeral Context → Working Memory → Long-term Memory → Procedural Memory
  ├─ Model & Tool Runtime — load/run/unload models; external tools as "skills"
  └─ Permission & Safety Guardrails
        ↓
AI-Native File System
  ├─ Content-Addressed Storage (CAS) — SHA-256 hash as file address, auto-dedup
  ├─ Semantic Vector Index           — embedding per file for semantic search
  ├─ Knowledge Graph                 — auto-correlates files into a knowledge network
  └─ Layered Context Loading         — L0 (~100 tokens), L1 (~2k tokens), L2 (full)
```

**Core philosophy shift:**
- Unit of management: processes/files → **agents/intents**
- Storage addressing: filesystem paths → **content hashes + semantic tags**
- Indexing: filenames → **vectors + knowledge graphs**
- Everything is a Tool (analogous to Unix's "everything is a file")

## Recommended Implementation Language

**Rust** — for memory safety, low-level control, and Cargo tooling.

## Recommended Start Point

Begin with the **Content-Addressed Storage (CAS) layer** — it is the most independent component with the fewest dependencies and embodies the core AI-perspective philosophy. Implementation plan:

1. **Weeks 1–2**: Tag-based index layer on top of CAS (`get_by_tag(...)`)
2. **Weeks 3–4**: Vector semantic search via `ort` or `candle` + LanceDB
3. **Weeks 5–6**: Expose a TCP service or CLI (`aios put/get/search`) for external AI programs to call

## Key Data Structures (from design doc)

```rust
pub struct AIObject {
    pub cid: String,          // SHA-256 of content
    pub data: Vec<u8>,
    pub meta: AIObjectMeta,
}

pub struct AIObjectMeta {
    pub content_type: String, // MIME type
    pub tags: Vec<String>,    // semantic tags, not paths
    pub created_by: String,   // agent ID
    pub created_at: u64,
}
```

## Semantic CRUD (no cp/mv — AI uses semantic APIs)

- **Create**: pass content + type + metadata; system handles storage
- **Read**: natural language query or hybrid vector+metadata search; return L0/L1/L2 layers as needed
- **Update**: full operation log for rollback
- **Delete**: logical delete (soft-delete / recycle bin), never immediate physical delete

## Tools

### Web Search (MCP)

The MiniMax MCP server provides `web_search` and `understand_image` tools. Configure it via:

```bash
# Install uvx first (required by the MCP server)
curl -LsSf https://astral.sh/uv/install.sh | sh

# Add MCP server (already configured in ~/.claude.json)
claude mcp add -s user MiniMax \
  --env MINIMAX_API_KEY=<key> \
  --env MINIMAX_API_HOST=https://api.minimaxi.com \
  -- uvx minimax-coding-plan-mcp -y
```

Tools are registered at user scope — available in all sessions. Use `claude mcp list` to verify.

**Important**: The built-in `WebSearch` tool does NOT work with MiniMax API (MiniMax does not support it). Always use `web_search` from the MiniMax MCP server instead.

## Embedding Backend

Local embeddings are powered by `bge-small-en-v1.5` (384d, 24MB, MTEB 62.17) via Python subprocess with ONNX Runtime. Configure via environment:

```bash
export EMBEDDING_BACKEND=local          # "local" (default) | "ollama" | "stub"
export EMBEDDING_MODEL_ID=BAAI/bge-small-en-v1.5   # HuggingFace model ID
export EMBEDDING_PYTHON=python3       # python interpreter path

# Setup (one-time):
pip install transformers huggingface_hub onnxruntime
# Model auto-downloads (~24MB for bge-small-en-v1.5)
```

The subprocess uses JSON-RPC over stdio — fully decoupled, no shared memory.

## Build & Test Commands

```bash
# Build
cargo build

# Run tests (all 38 tests)
cargo test

# Build release
cargo build --release

# Run CLI
cargo run --bin aicli -- --root /tmp/plico put --content "test" --tags "test"

# Run daemon
cargo run --bin plicod -- --port 7878 --root /tmp/plico
```

## Related Prior Art

- [AIOS (Rutgers)](https://github.com/agiresearch/AIOS) — full architecture reference
- VexFS — kernel-level vector search integration
- LanceDB — columnar vector + metadata storage
- OpenViking — context management reducing token usage by ~91%
