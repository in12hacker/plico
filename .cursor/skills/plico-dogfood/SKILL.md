---
name: plico-dogfood
description: PROACTIVE skill — the agent MUST automatically store architectural decisions, progress, experiences, bugs, and code changes into Plico's own CAS + Knowledge Graph during any Plico development work. This skill should be used proactively without waiting for the user to ask. Trigger on ANY code change, design decision, bug fix, feature completion, debugging insight, or reasoning that produced a non-obvious conclusion. Also triggers on "记录", "dogfood", "ADR", "经验", "进度", "保存到plico".
---

# Plico Dogfooding (Self-Sustaining)

This skill captures architectural decisions, progress, and insights into Plico's own CAS + Knowledge Graph during development. Plico manages itself.

## When to Record

Batch at natural pause points (feature complete, session end, milestone). Don't interrupt flow.

| Event | Record as |
|---|---|
| Design choice or trade-off | ADR |
| Feature/refactor completed | Progress |
| Bug fixed | Bug report |
| Non-obvious debugging insight | Experience |

## CLI Setup

Every command uses this wrapper — sets env, suppresses logs, enables JSON for parsing:

```bash
pcli() {
  EMBEDDING_BACKEND=stub RUST_LOG=off AICLI_OUTPUT=json \
    cargo run --quiet --bin aicli -- --root "${HOME}/.plico/dogfood" "$@" 2>/dev/null
}
pcli_human() {
  EMBEDDING_BACKEND=stub RUST_LOG=off \
    cargo run --quiet --bin aicli -- --root "${HOME}/.plico/dogfood" "$@" 2>/dev/null
}
AGENT="plico-dev"
```

### Bootstrap — MANDATORY first step

**MUST run before any storage operation.** Entity nodes are required anchors for KG linking.

```bash
EXISTING=$(pcli nodes --type entity --agent $AGENT 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin).get('nodes',[])))" 2>/dev/null || echo "0")
if [ "$EXISTING" = "0" ] || [ -z "$EXISTING" ]; then
  echo "Bootstrapping dogfood KG..."
  pcli_human agent --register $AGENT || true
  for mod in cas fs kernel api scheduler memory graph temporal cli daemon; do
    pcli_human node --label "$mod" --type entity \
      --props "{\"kind\":\"module\",\"path\":\"src/$mod\"}" --agent $AGENT
  done
  pcli_human node --label "v0.2-dogfooding" --type entity \
    --props '{"kind":"milestone"}' --agent $AGENT
fi
```

After bootstrap, cache module IDs for the session:
```bash
MODULE_JSON=$(pcli nodes --type entity --agent $AGENT)
```

Helper to look up a module entity ID by label:
```bash
mod_id() {
  echo "$MODULE_JSON" | python3 -c "
import sys,json
ns=json.load(sys.stdin).get('nodes',[])
matches=[n['id'] for n in ns if n['label']=='$1']
print(matches[0] if matches else '')" 2>/dev/null
}
```

## Tag Convention

`plico:<dimension>:<value>`:

| Dimension | Values |
|-----------|--------|
| `plico:type:<T>` | adr, progress, experience, test-result, bug, code-change, doc |
| `plico:module:<M>` | cas, fs, kernel, api, scheduler, memory, graph, temporal, cli, daemon |
| `plico:status:<S>` | accepted, active, superseded, resolved, wip |
| `plico:milestone:<V>` | v0.1 .. v0.6 |
| `plico:severity:<L>` | critical, high, medium, low (bugs only) |

## Storage Templates

### ADR — MUST link to KG

Every ADR creates: CAS object + Fact node + edge to module entity.
Do NOT skip the KG linking — unlinked ADRs are invisible to graph queries.

```bash
CID=$(pcli put --content "## <title>
Context: <why>
Decision: <what>
Consequences: <tradeoffs>" \
  --tags "plico:type:adr,plico:module:<mod>,plico:status:accepted" \
  --agent $AGENT | python3 -c "import sys,json; print(json.load(sys.stdin)['cid'])")

FACT_ID=$(pcli node --label "<title>" --type fact \
  --props "{\"content_cid\":\"$CID\",\"kind\":\"adr\"}" --agent $AGENT \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['node_id'])")

MOD_ID=$(mod_id <mod>)
if [ -n "$MOD_ID" ]; then
  pcli edge --src "$FACT_ID" --dst "$MOD_ID" --type related_to --agent $AGENT
fi
```

### Progress

```bash
pcli put --content "Date: $(date +%Y-%m-%d)
Completed: <what>
Files: <changed files>
Tests: <pass/fail>
Next: <upcoming>" \
  --tags "plico:type:progress,plico:milestone:v0.6" --agent $AGENT
```

### Experience

```bash
pcli put --content "What: <what happened>
Why: <root cause>
Takeaway: <lesson>" \
  --tags "plico:type:experience,plico:module:<mod>" --agent $AGENT
```

### Bug

```bash
pcli put --content "Bug: <title>
Module: <mod> | Severity: <level>
Symptom: <what broke>
Root cause: <why>
Fix: <what fixed it>" \
  --tags "plico:type:bug,plico:module:<mod>,plico:severity:<level>" --agent $AGENT
```

## Query

```bash
pcli_human nodes --type entity --agent $AGENT        # module anchors
pcli_human nodes --type fact --agent $AGENT          # decisions/skills
pcli_human search "adr" -t "plico:type:adr" --agent $AGENT
pcli_human search "<module>" -t "plico:type:experience" --agent $AGENT
pcli_human tags
pcli_human explore --cid <node-id> --agent $AGENT
pcli_human paths --src <id1> --dst <id2> --depth 3
```

Note: With `EMBEDDING_BACKEND=stub`, search falls back to tag-substring matching.
Use `--require-tags` / `-t` with `--query` for precise filtering.

## Known Limitations (as of Node 13 audit)

- `recall --tier X` does not filter by tier (B38) — all tiers returned regardless
- `remember --tier procedural` routes to ephemeral storage (F-B) — use `tool call memory.store_procedure` instead
- `--require-tags` without `--query` returns empty (B41) — always combine with `--query`
- `search --require-tags` uses substring matching, not exact tag match
- `tool call memory.recall` returns empty content field (B40) — use CLI `recall` instead
- `delete` may panic on certain CID formats (B35)

## Principles

- Generic KG types only (Entity/Fact) — domain semantics live in `plico:` tags
- All operations via `aicli` — no kernel-layer hacks
- Model-agnostic — any AI agent can use the same API
- Batch over interrupt — record at pause points, not mid-flow
- **ALWAYS bootstrap before first storage** — entity anchors are required for graph structure
- **ALWAYS link ADRs to KG** — unlinked facts are invisible islands (see Node 14 F-J)
