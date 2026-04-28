# Plico v33 Audit Report — Tech Debt Cleanup & Feature Additions

**Date**: 2026-04-29  
**Scope**: Full codebase audit, tech debt elimination, feature additions  
**Files Changed**: 36 (34 modified, 2 new)  
**Net**: +546 / −213 lines  
**Tests**: 1,038 unit + 38 integration — all green  
**Clippy**: zero warnings

---

## 1. Bug Fixes

| ID | Description | Files |
|----|-------------|-------|
| B-1 | `bench_b18_agent_profile_learning` assertion omitted `bm25_keyword` weight (7th signal), causing false "sum ≠ 1.0" panic. Changed to `final_weights.total()`. | `tests/real_llm_benchmark.rs` |
| B-2 | `oldest_entry_age_ms` tracked minimum age (newest) instead of maximum age (oldest). Init changed from `u64::MAX` to `0`, comparison flipped to `>`. | `src/kernel/ops/memory.rs`, `src/memory/layered/mod.rs` |

## 2. Fake / Stub Implementations Replaced

| ID | Before | After | Files |
|----|--------|-------|-------|
| S-1 | `llm_available()` always returned `true` | Sends minimal probe (`ChatMessage::user("ping")`, `max_tokens=1`) and returns `is_ok()` | `src/kernel/ops/dashboard.rs` |
| S-2 | `is_cid_referenced()` O(n) scan over all nodes | CID reverse index (`HashMap<String, HashSet<String>>`) on `PetgraphBackend`, maintained on `add_node`/`remove_node`, O(1) lookup | `src/fs/graph/backend.rs`, `src/fs/graph/mod.rs`, `src/kernel/ops/graph.rs` |

## 3. New Features

### 3.1 Markdown-Aware Chunking (`ChunkingMode::Markdown`)

- Splits on ATX headings (`#`…`######`) and thematic breaks (`---`)
- Preserves fenced code blocks (```) intact within their parent section
- Falls back to `fixed_chunk` for oversized sections
- Env var: `PLICO_CHUNKING=markdown`
- Files: `src/fs/chunking/mod.rs` (+3 tests)

### 3.2 File Import — CLI

```
aicli put --file document.md --tags project:docs
aicli put --dir ./notes/ --glob "*.md" --tags imported --chunking markdown
```

- `--file <path>`: single file import
- `--dir <path> --glob <pattern>`: recursive directory scan (default `*.md`)
- `--chunking <mode>`: override chunking (`markdown`|`fixed`|`semantic`|`none`)
- Auto-tags: `source:<filename>`, `type:<ext>`
- Files: `src/bin/aicli/commands/handlers/crud.rs`, `src/bin/aicli/main.rs`

### 3.3 File Import — API

```json
{
  "import_files": {
    "paths": ["/abs/path/to/doc.md"],
    "agent_id": "agent-1",
    "tags": ["project:docs"],
    "chunking": "markdown"
  }
}
```

- Response includes `import_results` array with per-file `cid`, `chunks`, `ok`, `error`
- Files: `src/api/semantic.rs`, `src/kernel/api_dispatch.rs`, `src/kernel/handlers/import.rs` (new), `src/kernel/handlers/mod.rs`

### 3.4 Unified Configuration (`PlicoConfig`)

- 3-layer cascade: defaults → `config.json` → environment variables
- Covers: network (host/port/UDS), inference (llama URL), tuning (persist intervals)
- Cross-platform llama server detection (`ps aux` parsing via Rust `Command`)
- Files: `src/config.rs` (new), `src/lib.rs`

## 4. Portability Fixes

| Area | Before | After |
|------|--------|-------|
| TCP bind | `0.0.0.0:7878` | `127.0.0.1:7878` (configurable via `--host`) |
| UDS client | No platform guard | `#[cfg(unix)]` + `Unsupported` error on non-Unix |
| `/tmp` fallback | Hardcoded `/tmp` | `std::env::temp_dir()` |
| Memory stats | Linux-only `/proc/meminfo` | Linux + macOS (`sysctl`/`vm_stat`) |
| Llama detection | `grep -oP` (GNU-only) | Rust `Command::new("ps")` with manual parsing |
| Home dir | `$HOME` env var | `dirs::home_dir()` (works on Windows) |

## 5. Clippy Cleanup (15 warnings → 0)

- `#[derive(Default)]` for 6 structs (replaced manual `new()`)
- `map_or(true, …)` → `is_none_or(…)` (2 sites)
- `into_iter().map(|(k,_)|)` → `into_keys()` (1 site)
- `importance = importance / 2` → `importance /= 2`
- `display().to_string() == new_content` → `display() == new_content`
- Elided explicit lifetimes in `find_cross_agent_merge_candidates`
- `(x as u32) * 2` → redundant cast removed
- Type alias for complex `Vec<…>` in distillation
- `#[allow(clippy::too_many_arguments)]` for `run_prefetch` (15-param internal function)
- `or_insert_with(Foo::new)` → `or_default()` where `Foo: Default`

## 6. ApiResponse Simplification

- Implemented `Default` for `ApiResponse` (60+ fields → single source of truth)
- `ok()` and `error()` now use `..Self::default()`

## 7. Documentation Updates

- `CLAUDE.md`: corrected test count to "1,038 unit + 38 integration"
- `README.md` / `README_zh.md`: updated stats, added Configuration section, updated quick-start commands
- `tenant.rs`: 3 `TODO` → `ROADMAP` (deferred to multi-tenant milestone)

## 8. Test Summary

```
cargo test --lib:  1038 passed, 0 failed
cargo test --test api_version_test: 14 passed
cargo test --test real_llm_benchmark: 24 passed
cargo clippy: 0 warnings
```
