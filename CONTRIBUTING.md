# Contributing to Plico

Thank you for your interest in contributing to Plico (太初)!

## Development Setup

```bash
# Clone
git clone https://github.com/in12hacker/plico.git
cd plico

# Build
cargo build

# Run tests (no external dependencies)
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# Task runner (optional)
cargo install just
just test       # lib tests
just gate       # test + clippy
just coverage   # coverage report
```

## Coding Standards

- **Language**: Rust edition 2021
- **Files**: `snake_case.rs`, one concept per file, target < 300 lines
- **Naming**: `snake_case` functions, `PascalCase` types, `SCREAMING_SNAKE` constants
- **Modules**: `pub mod` in `mod.rs`; large modules split into `dir/mod.rs` + sub-files
- **Visibility**: `pub fn` only for public API, private by default
- **No unsafe**: unless documented with `# Safety` comment
- **No `#[allow(clippy::...)]`**: structural lint suppressions are not permitted

## Testing

```bash
# Unit tests (inline in source files)
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# Integration tests (tests/ directory)
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# Specific module
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib kernel::ops::fs::tests
```

- **Unit tests**: `#[cfg(test)] mod tests` inline in source files
- **Integration tests**: `tests/` directory, named `{module}_test.rs`
- **Stub backends**: Always use `EMBEDDING_BACKEND=stub LLM_BACKEND=stub` for tests

## Quality Gates

All PRs must pass:

1. `cargo test` — zero failures
2. `cargo clippy -- -D warnings` — zero warnings
3. `cargo fmt --check` — properly formatted
4. `cargo llvm-cov --lib` — coverage ≥ 90%

## PR Process

1. Fork and create a feature branch
2. Write code following the coding standards above
3. Add tests for new functionality
4. Run `just gate` (or `cargo test && cargo clippy -- -D warnings`)
5. Submit PR with a clear description of changes
6. CI must pass before merge

## Architecture

See `AGENTS.md` for the full directory map and module descriptions. Key constraints:

- Dependency direction: `api/bin → kernel → tool/fs/intent → cas/memory/scheduler/temporal/llm`
- `kernel/` is the only module that imports all others
- CAS is the only module that touches the host filesystem
- All errors are typed via `thiserror`

## Reporting Issues

- Use GitHub Issues for bug reports and feature requests
- Include steps to reproduce for bugs
- Reference relevant modules from `AGENTS.md`
