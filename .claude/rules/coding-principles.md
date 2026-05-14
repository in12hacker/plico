# Coding Principles

Behavioral guidelines to reduce common LLM coding mistakes.

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
