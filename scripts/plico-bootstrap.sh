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

ROOT="${PLICO_ROOT:-/tmp/plico-dogfood}"
AGENT="plico-dev"
export EMBEDDING_BACKEND=stub

CLI="cargo run --quiet --bin aicli -- --root $ROOT"

echo "=== Plico Bootstrap ==="
echo "Storage root: $ROOT"
echo ""

# 1. Register the dogfooding agent
echo "--- Registering agent: $AGENT ---"
$CLI agent --register "$AGENT" 2>/dev/null || true

# 2. Create Module Entity nodes
echo ""
echo "--- Creating module entities ---"
MODULES=(cas fs kernel api scheduler memory graph temporal cli daemon)
declare -A MODULE_IDS

for mod in "${MODULES[@]}"; do
    ID=$($CLI node --label "$mod" --type entity \
        --props "{\"kind\":\"module\",\"path\":\"src/$mod\"}" \
        --agent "$AGENT" 2>/dev/null | grep "Node created:" | awk '{print $3}')
    MODULE_IDS[$mod]="$ID"
    echo "  $mod -> $ID"
done

# 3. Create current milestone
echo ""
echo "--- Creating milestone entity ---"
MILESTONE_ID=$($CLI node --label "v0.2-dogfooding" --type entity \
    --props '{"kind":"milestone","target":"2026-05-01","goals":["KG API","self-management","bootstrap"]}' \
    --agent "$AGENT" 2>/dev/null | grep "Node created:" | awk '{print $3}')
echo "  v0.2-dogfooding -> $MILESTONE_ID"

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
    --agent "$AGENT" 2>/dev/null | grep "CID:" | awk '{print $2}')
echo "  ADR CID: $ADR_CID"

# 6. Create a Fact node for this decision and link to modules
echo ""
echo "--- Creating decision fact + edges ---"
FACT_ID=$($CLI node --label "Use generic KG primitives for project management" --type fact \
    --props "{\"content_cid\":\"$ADR_CID\",\"kind\":\"adr\"}" \
    --agent "$AGENT" 2>/dev/null | grep "Node created:" | awk '{print $3}')
echo "  Fact: $FACT_ID"

$CLI edge --src "$FACT_ID" --dst "${MODULE_IDS[graph]}" \
    --type related_to --agent "$AGENT" 2>/dev/null || true
echo "  fact --[related_to]--> graph"

$CLI edge --src "$FACT_ID" --dst "${MODULE_IDS[kernel]}" \
    --type related_to --agent "$AGENT" 2>/dev/null || true
echo "  fact --[related_to]--> kernel"

# 7. Summary
echo ""
echo "=== Bootstrap Complete ==="
echo "Modules: ${#MODULES[@]}"
echo "Milestone: v0.2-dogfooding"
echo "ADRs: 1"
echo ""
echo "Verify with:"
echo "  EMBEDDING_BACKEND=stub cargo run --bin aicli -- --root $ROOT nodes --type entity --agent $AGENT"
echo "  EMBEDDING_BACKEND=stub cargo run --bin aicli -- --root $ROOT explore --cid $MILESTONE_ID --agent $AGENT"
