---
name: plico-dogfood
description: PROACTIVE skill — the agent MUST automatically store architectural decisions, progress, experiences, bugs, and code changes into Plico's own CAS + Knowledge Graph during any Plico development work. This skill should be used proactively without waiting for the user to ask. Trigger on ANY code change, design decision, bug fix, feature completion, debugging insight, or reasoning that produced a non-obvious conclusion. Also triggers on "记录", "dogfood", "ADR", "经验", "进度", "保存到plico".
---

# Plico Dogfooding (Self-Sustaining)

**CRITICAL BEHAVIOR**: This skill is PROACTIVE. During ANY Plico development session, the agent MUST automatically capture valuable information into Plico without being asked. The agent acts as both developer AND project historian simultaneously.

## Proactive Trigger Rules

After EVERY significant action during development, check this list and act:

| After doing this... | Automatically do this |
|---|---|
| Made a design choice (even implicit, e.g. "use builder pattern for ApiResponse") | → Store ADR |
| Completed a TODO / feature / refactor | → Store progress snapshot |
| Fixed a bug or test failure | → Store bug report + resolution |
| Discovered something non-obvious while debugging | → Store experience |
| Finished a multi-step task | → Store progress summary |
| Encountered a pattern worth remembering | → Store experience |
| Made a trade-off (e.g. "chose X over Y because...") | → Store ADR |
| Reasoning chain produced a reusable insight | → Store experience |

**Do NOT wait for the user to say "记录".** Act immediately after the development action completes.

**Batching**: If multiple events occur in rapid succession (e.g. 3 bug fixes in a row), batch them into a single progress snapshot rather than storing individually.

## Environment Setup

At the start of any Plico development session, ensure dogfood storage is ready:

```bash
export EMBEDDING_BACKEND=stub
export PLICO_ROOT=/tmp/plico-dogfood
AGENT="plico-dev"
```

Check if bootstrapped — if `nodes --type entity` returns empty, run:
```bash
PLICO_ROOT=/tmp/plico-dogfood ./scripts/plico-bootstrap.sh
```

Retrieve module entity IDs once per session and cache them:
```bash
EMBEDDING_BACKEND=stub cargo run --quiet --bin aicli -- --root $PLICO_ROOT nodes --type entity --agent plico-dev
```

## Tag Convention

`plico:<dimension>:<value>` — dimensions:

| Dimension | Values |
|-----------|--------|
| `plico:type:<T>` | adr, progress, experience, test-result, bug, code-change, doc |
| `plico:module:<M>` | cas, fs, kernel, api, scheduler, memory, graph, temporal, cli, daemon |
| `plico:status:<S>` | active, superseded, resolved, wip |
| `plico:milestone:<V>` | v0.1, v0.2, v0.3, v0.4, v0.5 |
| `plico:severity:<L>` | critical, high, medium, low (bugs only) |

## Storage Commands

All commands use this base:
```bash
CLI="EMBEDDING_BACKEND=stub cargo run --quiet --bin aicli -- --root /tmp/plico-dogfood"
```

### ADR (Architectural Decision)

Store whenever a design choice is made — even small ones like "use `Option<Arc<dyn Trait>>` for optional subsystems".

```bash
$CLI put --content "---
title: \"<title>\"
status: accepted
tags: [plico:type:adr, plico:module:<mod>]
date: $(date +%Y-%m-%d)
author: plico-dev
---
## Context
<why>
## Decision
<what>
## Consequences
- Positive: <benefit>
- Negative: <cost>" \
  --tags "plico:type:adr,plico:module:<mod>,plico:status:accepted" \
  --agent plico-dev
```

Then create Fact node + edge to module:
```bash
$CLI node --label "<title>" --type fact \
  --props "{\"content_cid\":\"<CID>\",\"kind\":\"adr\"}" --agent plico-dev
$CLI edge --src <fact-id> --dst <module-id> --type related_to --agent plico-dev
```

### Progress Snapshot

Store after completing a feature, a batch of TODOs, or at natural pause points.

```bash
$CLI put --content "Date: $(date +%Y-%m-%d)
Completed: <what was done>
Files changed: <list>
Tests: <pass/fail count>
Next: <what comes next>" \
  --tags "plico:type:progress,plico:milestone:<ver>" --agent plico-dev
```

### Experience / Lesson Learned

Store whenever debugging reveals a non-obvious cause, or when a pattern emerges that future agents should know.

```bash
$CLI put --content "## What
<what happened>
## Why
<root cause / insight>
## Takeaway
<reusable lesson>" \
  --tags "plico:type:experience,plico:module:<mod>" --agent plico-dev
```

### Bug Report

Store when a test fails, compilation breaks unexpectedly, or runtime behavior is wrong.

```bash
$CLI put --content "Bug: <title>
Module: <mod>
Severity: <level>
Steps: <reproduction>
Fix: <what fixed it>" \
  --tags "plico:type:bug,plico:module:<mod>,plico:severity:<level>" --agent plico-dev
```

### Test Result

Store after running `cargo test`, especially when results change.

```bash
$CLI put --content "$(cargo test 2>&1 | tail -5)" \
  --tags "plico:type:test-result,plico:milestone:<ver>" --agent plico-dev
```

## Query Commands

```bash
$CLI nodes --type entity --agent plico-dev          # list modules
$CLI nodes --type fact --agent plico-dev             # list decisions/facts
$CLI paths --src <id1> --dst <id2> --depth 3         # find relationships
$CLI explore --cid <node-id> --agent plico-dev       # neighborhood
$CLI tags                                             # all tags
$CLI search "<query>" --require-tags "plico:type:experience" --agent plico-dev
```

## Self-Check Protocol

At the END of every development turn (before responding to user), the agent asks itself:

1. Did I make any design decisions? → ADR
2. Did I complete something? → Progress
3. Did I learn something non-obvious? → Experience
4. Did I fix a bug? → Bug report
5. Did test results change? → Test result

If ANY answer is yes, store it **now** before finishing the response. This is not optional.

## Soul Alignment

- Generic types only (Entity/Fact + `plico:` tags) — never add project-specific KG types
- All operations via standard `aicli` — no kernel-layer project logic
- Any AI agent can use the same API — nothing binds to a specific model or agent
