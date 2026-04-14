# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

## Related Prior Art

- [AIOS (Rutgers)](https://github.com/agiresearch/AIOS) — full architecture reference
- VexFS — kernel-level vector search integration
- LanceDB — columnar vector + metadata storage
- OpenViking — context management reducing token usage by ~91%
