#!/bin/bash
# plico-bootstrap.sh — Seed Plico with its own project structure.
#
# Uses aicli to store module entities, milestone, and an initial ADR
# into Plico's own CAS + KG — the first step of dogfooding.
#
# Usage:
#   PLICO_ROOT=/tmp/plico-dogfood ./scripts/plico-bootstrap.sh
#
# Requires: cargo build (aicli binary must be available)

set -euo pipefail

ROOT="${PLICO_ROOT:-${HOME}/.plico/dogfood}"
AGENT="plico-dev"
export EMBEDDING_BACKEND=stub

CLI="cargo run --quiet --bin aicli -- --root $ROOT"

# Ensure root directory exists
mkdir -p "$ROOT"

echo "=== Plico Bootstrap ==="
echo "Storage root: $ROOT"
echo ""

# 1. Register the dogfooding agent (idempotent: skip if already registered)
echo "--- Registering agent: $AGENT ---"
$CLI agent --register "$AGENT" 2>/dev/null || true

# 2. Create Module Entity nodes (idempotent: skip if already exists)
echo ""
echo "--- Creating module entities ---"
MODULES=(cas fs kernel api scheduler memory graph temporal cli daemon)
declare -A MODULE_IDS

for mod in "${MODULES[@]}"; do
    # F-4: Check if entity with this label already exists (idempotent bootstrap)
    existing=$($CLI nodes --type entity --agent "$AGENT" 2>/dev/null \
        | grep -i "\"$mod\"" | head -1 | grep -o '[a-f0-9-]\{36\}' | head -1 || true)
    if [ -n "$existing" ]; then
        ID="$existing"
        echo "  $mod -> $ID (exists, skipping)"
    else
        ID=$($CLI node --label "$mod" --type entity \
            --props "{\"kind\":\"module\",\"path\":\"src/$mod\"}" \
            --agent "$AGENT" 2>/dev/null | grep "Node ID:" | awk '{print $3}')
        echo "  $mod -> $ID (created)"
    fi
    MODULE_IDS[$mod]="$ID"
done

# 3. Create current milestone (idempotent)
echo ""
echo "--- Creating milestone entity ---"
existing_milestone=$($CLI nodes --type entity --agent "$AGENT" 2>/dev/null \
    | grep -i "v0.2-dogfooding\|milestone" | head -1 | grep -o '[a-f0-9-]\{36\}' | head -1 || true)
if [ -n "$existing_milestone" ]; then
    MILESTONE_ID="$existing_milestone"
    echo "  v0.2-dogfooding -> $MILESTONE_ID (exists, skipping)"
else
    MILESTONE_ID=$($CLI node --label "v0.2-dogfooding" --type entity \
        --props '{"kind":"milestone","target":"2026-05-01","goals":["KG API","self-management","bootstrap"]}' \
        --agent "$AGENT" 2>/dev/null | grep "Node ID:" | awk '{print $3}')
    echo "  v0.2-dogfooding -> $MILESTONE_ID (created)"
fi

# 4. Link modules to milestone
echo ""
echo "--- Linking modules to milestone ---"
for mod in "${MODULES[@]}"; do
    $CLI edge --src "${MODULE_IDS[$mod]}" --dst "$MILESTONE_ID" \
        --type part_of --agent "$AGENT" 2>/dev/null || true
    echo "  $mod --[part_of]--> milestone"
done

# 5. Store the first ADR: "Use generic KG types for project management"
echo ""
echo "--- Storing initial ADR ---"
ADR_CONTENT="---
title: Use generic KG primitives for project management
status: accepted
tags: [plico:type:adr, plico:module:graph, plico:module:kernel]
date: $(date +%Y-%m-%d)
author: $AGENT
---

## Context
Plico needs to manage its own project data (decisions, progress, bugs).
Previous iterations embedded project-specific types (Iteration, Plan, DesignDoc)
directly into the KG kernel, violating the AIOS soul.

## Decision
Use generic KG primitives (Entity + Fact nodes, standard edge types) with
semantic tags (plico:type:adr, plico:module:cas) to encode project semantics.
All project management happens at the application layer via standard aicli API.

## Consequences
- Positive: No soul violations; any AI agent can use the same API
- Positive: Tag-based semantics are flexible and extensible
- Negative: Slightly more verbose than dedicated types
- Neutral: Requires tag convention discipline (documented in AGENTS.md)"

ADR_CID=$($CLI put --content "$ADR_CONTENT" \
    --tags "plico:type:adr,plico:module:graph,plico:module:kernel,plico:status:accepted" \
    --agent "$AGENT" 2>/dev/null | grep "^CID:" | awk '{print $2}')
echo "  ADR CID: $ADR_CID"

# 6. Create a Fact node for this decision and link to modules
echo ""
echo "--- Creating decision fact + edges ---"
FACT_ID=$($CLI node --label "Use generic KG primitives for project management" --type fact \
    --props "{\"content_cid\":\"$ADR_CID\",\"kind\":\"adr\"}" \
    --agent "$AGENT" 2>/dev/null | grep "Node ID:" | awk '{print $3}')
echo "  Fact: $FACT_ID"

$CLI edge --src "$FACT_ID" --dst "${MODULE_IDS[graph]}" \
    --type related_to --agent "$AGENT" 2>/dev/null || true
echo "  fact --[related_to]--> graph"

$CLI edge --src "$FACT_ID" --dst "${MODULE_IDS[kernel]}" \
    --type related_to --agent "$AGENT" 2>/dev/null || true
echo "  fact --[related_to]--> kernel"

# F-6: Batch record historical ADRs for nodes 1-15
record_adr_bootstrap() {
    local title="$1"
    local tags="$2"
    local content="$3"
    local existing=$($CLI nodes --type fact --agent "$AGENT" 2>/dev/null \
        | grep -i "$(echo "$title" | tr '[:upper:]' '[:lower:]')" | head -1 | grep -o '[a-f0-9-]\{36\}' | head -1 || true)
    if [ -n "$existing" ]; then
        echo "  ADR '$title' exists (skipping)"
        return
    fi
    local CID=$($CLI put --content "$content" --tags "$tags" --agent "$AGENT" 2>/dev/null | grep "^CID:" | awk '{print $2}')
    if [ -z "$CID" ]; then
        echo "  ADR '$title' failed to store"
        return
    fi
    local FACT_ID=$($CLI node --label "$title" --type fact \
        --props "{\"content_cid\":\"$CID\",\"kind\":\"adr\"}" --agent "$AGENT" 2>/dev/null \
        | grep "Node ID:" | awk '{print $3}')
    echo "  ADR: $title -> $FACT_ID"
}

echo ""
echo "--- Recording historical ADRs (F-6) ---"
record_adr_bootstrap "CAS Content Addressing" "plico:type:adr,plico:module:cas,plico:status:accepted" \
  "## CAS Content Addressing\nDecision: Use SHA-256 content hash as primary address. Auto-dedup by content. All operations address by CID not path."

record_adr_bootstrap "Four-Layer Memory" "plico:type:adr,plico:module:memory,plico:status:accepted" \
  "## Four-Layer Memory Architecture\nDecision: Ephemeral -> Working -> Long-term -> Procedural tiers. Tier-based lifecycle management. Each tier has distinct eviction/rehydration policies."

record_adr_bootstrap "Everything is a Tool" "plico:type:adr,plico:module:tool,plico:status:accepted" \
  "## Tool-centric Architecture\nDecision: All external capabilities (filesystem, network, execution) exposed as Tools. Unified ToolHandler trait. No privileged built-ins."

record_adr_bootstrap "Agent Lifecycle Management" "plico:type:adr,plico:module:scheduler,plico:status:accepted" \
  "## Agent Lifecycle Management\nDecision: Create -> Ready -> Running <-> Suspended -> Terminated state machine. Checkpoint-based suspend/resume. Agent identified by UUID."

record_adr_bootstrap "Event Bus Architecture" "plico:type:adr,plico:module:kernel,plico:status:accepted" \
  "## Event Bus Architecture\nDecision: Asynchronous event bus for inter-component communication. Event sourcing for audit trail. Topics: agent.*, memory.*, tool.*, kg.*"

record_adr_bootstrap "Session Persistence" "plico:type:adr,plico:module:persistence,plico:status:accepted" \
  "## Session Persistence\nDecision: Sessions persisted to CAS as checkpoints. Resume loads last checkpoint + replay log. Session ID is CID of checkpoint."

record_adr_bootstrap "KG Generic Types" "plico:type:adr,plico:module:graph,plico:status:accepted" \
  "## Knowledge Graph Generic Types\nDecision: Entity/Fact/Document/Agent/Memory node types. RelatedTo/PartOf/Mentions/Causes edge types. ID is CID of content. No schema enforcement."

record_adr_bootstrap "Tool Handler Trait" "plico:type:adr,plico:module:tool,plico:status:accepted" \
  "## Tool Handler Trait\nDecision: ToolHandler trait with execute(context, params) -> Result<Value>. Registry maps name -> handler. Handlers are stateless."

record_adr_bootstrap "ExternalToolProvider Protocol" "plico:type:adr,plico:module:tool,plico:status:accepted" \
  "## External Tool Provider Protocol\nDecision: External tools via ExternalToolProvider trait: list_tools() -> Vec<ToolDef>, call_tool(name, params) -> Result<Value>. MCP as reference implementation."

record_adr_bootstrap "Semantic Search Fallback" "plico:type:adr,plico:module:fs,plico:status:accepted" \
  "## Semantic Search Fallback Chain\nDecision: Vector search -> BM25 keyword search -> CAS full-scan. Embedding model configurable (ollama/local/stub). Fallback ensures availability."

record_adr_bootstrap "Agent Checkpoint via CAS" "plico:type:adr,plico:module:kernel,plico:status:accepted" \
  "## Agent Checkpoint via CAS\nDecision: Agent state serialized to JSON, stored as CAS object. Checkpoint CID = agent state CID. Enables perfect resume."

record_adr_bootstrap "Memory Link Engine" "plico:type:adr,plico:module:memory,plico:status:accepted" \
  "## Memory Link Engine\nDecision: On remember_long_term, auto-create KG Memory node + SimilarTo edges to related memories. Tag-based similarity. Bidirectional edges."

record_adr_bootstrap "Tier Maintenance Cycle" "plico:type:adr,plico:module:memory,plico:status:accepted" \
  "## Tier Maintenance Cycle\nDecision: Session-end runs tier maintenance: promote recent ephemeral to working, archive old working to long-term. Never demote working or long-term."

record_adr_bootstrap "Concurrent Agent Dispatch" "plico:type:adr,plico:module:scheduler,plico:status:accepted" \
  "## Concurrent Agent Dispatch\nDecision: Scheduler uses work-stealing queue for agent dispatch. Max concurrent agents configurable. Agent yield on I/O wait."

record_adr_bootstrap "Context Budget Engine" "plico:type:adr,plico:module:kernel,plico:status:accepted" \
  "## Context Budget Engine\nDecision: Context loader tracks token budget per request. L0/L1/L2 layers with progressive loading. Budget exceeded = partial context + warning."
# 8. Summary
echo ""
echo "=== Bootstrap Complete ==="
echo "Modules: ${#MODULES[@]}"
echo "Milestone: v0.2-dogfooding"
echo "ADRs: 16 (1 initial + 15 historical)"
echo ""
echo "Verify with:"
echo "  EMBEDDING_BACKEND=stub cargo run --bin aicli -- --root $ROOT nodes --type entity --agent $AGENT"
echo "  EMBEDDING_BACKEND=stub cargo run --bin aicli -- --root $ROOT explore --cid $MILESTONE_ID --agent $AGENT"
