# Plico — AI-Native Operating System

An operating system designed **entirely from AI perspective** — no human CLI/GUI, no filesystem paths, no traditional OS assumptions.

Every data operation (files, images, audio, video) is performed by AI through semantic APIs. The system is model-agnostic and exposes AI-friendly interfaces for upper-layer AI agents.

## Status

**Phase 0 — Project Initialization.** The design is documented in `system.md`. Implementation starts with the **Content-Addressed Storage (CAS) layer** as the foundational building block.

## Architecture

```
AI Agents (upper layer)
        ↓  semantic API / CLI
┌───────────────────────────────────────┐
│  AI Kernel                            │
│  ├─ Agent Scheduler                   │
│  ├─ Layered Memory Manager            │
│  ├─ Model & Tool Runtime              │
│  └─ Permission Guardrails             │
│                                       │
│  AI-Native Filesystem                 │
│  ├─ Content-Addressed Storage (CAS)   │  ← Start here
│  ├─ Semantic Vector Index             │
│  ├─ Knowledge Graph                   │
│  └─ Layered Context Loader (L0/L1/L2) │
└───────────────────────────────────────┘
```

## Quick Start

```bash
# Build
cargo build --release

# Run AI-friendly CLI
cargo run --bin aicli -- put --content "hello" --tags "greeting"
cargo run --bin aicli -- get <CID>
cargo run --bin aicli -- search --query "greeting"

# Run daemon
cargo run --bin plicod
```

## Directory Structure

```
src/
├── cas/          # Content-Addressed Storage (SHA-256, object store)
├── memory/       # Layered memory management (ephemeral → long-term)
├── scheduler/    # Agent lifecycle scheduler
├── fs/           # Semantic filesystem (CRUD, vector index, knowledge graph)
├── kernel/       # AI Kernel (orchestrates all subsystems)
├── api/          # AI-friendly semantic API (CLI, TCP, HTTP)
└── permission/   # Permission & safety guardrails

AGENTS.md         # Project root navigation index (for AI agents)
```

## Design Document

See `system.md` for the full AI-native OS design rationale (in Chinese).
