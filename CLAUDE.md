# CLAUDE.md — 太初 (Plico) AI-OS

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## AI Navigation

When creating or updating `AGENTS.md`, `INDEX.md`, or any project navigation index, use the **ariadne-thread** skill. All module directories have `INDEX.md` (L1) with public API, dependencies, dependents, task routing, and modification risk. See `AGENTS.md` for full project structure.

## Project Overview

**太初 (Plico)** is an AI-native operating system kernel designed from scratch exclusively for AI agents — no human CLI/GUI, no human filesystem paths. All data management is performed by AI through AI-friendly semantic APIs. The system is model-agnostic and does not depend on any specific AI or agent. "太初" means "Genesis / In the Beginning" — the primordial state where an AI-OS becomes self-aware.

The design document is in `system.md` and `system-v2.md` (Soul 2.0, written in Chinese).
The complete reference is in `docs/genesis-reference.md`.

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

## Inference Backend

Embedding and LLM backends are **inference-framework-agnostic**. Any server exposing an OpenAI-compatible `/v1/embeddings` or `/v1/chat/completions` endpoint works (llama.cpp, vLLM, SGLang, TensorRT-LLM, Ollama, OpenAI, etc.).

**Defaults (auto-detect llama-server port from running process, fallback :8080):**
- `LLM_BACKEND=llama` → auto-detected llama-server URL
- `EMBEDDING_BACKEND=openai` → same auto-detected URL
- Model: `qwen2.5-coder-7b-instruct` (override via `LLAMA_MODEL`)

URL resolution priority: `LLAMA_URL` env > `OPENAI_API_BASE` env > `~/.plico/llama.url` file > auto-detect from `ps` > `:8080` fallback.

```bash
# Only for unit tests: stub backend (no external service)
export EMBEDDING_BACKEND=stub
export LLM_BACKEND=stub
```

**DO NOT** use `EMBEDDING_BACKEND=local` — it spawns a Python subprocess calling Ollama, extremely slow.

## Build & Test Commands

```bash
# Build
cargo build

# Run tests (1,435 tests)
cargo test

# Build release
cargo build --release

# Run CLI (embedded mode — direct kernel, no daemon)
cargo run --bin aicli -- --embedded put --content "test" --tags "test"

# Run CLI (daemon mode — default, requires running plicod)
cargo run --bin aicli -- put --content "test" --tags "test"

# Run daemon (start/stop/status lifecycle)
cargo run --bin plicod -- start --port 7878
cargo run --bin plicod -- stop
cargo run --bin plicod -- status
```

## Related Prior Art

- [AIOS (Rutgers)](https://github.com/agiresearch/AIOS) — full architecture reference
- VexFS — kernel-level vector search integration
- LanceDB — columnar vector + metadata storage
- OpenViking — context management reducing token usage by ~91%

# CLAUDE.md

Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:
- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:
- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:
- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:
- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:
```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

---

**These guidelines are working if:** fewer unnecessary changes in diffs, fewer rewrites due to overcomplication, and clarifying questions come before implementation rather than after mistakes.

## 5. No Compatibility Code (Pre-Release Policy)

**This project has no released versions. Only one branch is maintained. All backward-compatibility code is dead code.**

Rules:
- Do NOT write data migration code (old format → new format). No users have old data.
- Do NOT write version-checking logic (`is_compatible`, `is_deprecated`, `MIN_SUPPORTED`). There is only one version.
- Do NOT add CLI argument aliases "for compatibility". There are no existing scripts using old flags.
- Do NOT create `DeprecationNotice` types or deprecation-check functions. Nothing is deprecated.
- Do NOT add fallback paths (e.g., "try redb, fall back to JSON"). Pick one format and commit.

When to revisit: Only after the **first public release**. At that point, compatibility code becomes necessary.

**Lesson learned (2026-04-23):** A redb migration added `migrate_old_edge_keys()`, `bulk_persist_to_redb()`, `load_from_json()` fallback, and `DeprecationNotice` — all for formats that never existed in production. These 140+ lines of dead code were cleaned up immediately. The cost of premature compatibility code: wasted implementation time, inflated code size, test maintenance burden, and misleading code paths that confuse future AI agents reading the codebase.