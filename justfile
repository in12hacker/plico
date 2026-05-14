# justfile — Plico development task runner
# Usage: just <command>
# Install: cargo install just

# Default: show available commands
default:
    @just --list

# --- Build ---

# Build all targets
build:
    cargo build

# Build release
build-release:
    cargo build --release

# Build specific binary
build-bin name:
    cargo build --bin {{name}}

# --- Test ---

# Run lib tests (fastest, ~2s)
test:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# Run all tests
test-all:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# Run specific module test
test-mod module:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib {{module}}

# Run integration tests only
test-integration:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test '*'

# --- Quality ---

# Clippy lint (zero warnings)
lint:
    cargo clippy -- -D warnings

# Coverage measurement (requires cargo-llvm-cov)
coverage:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# Coverage with HTML report
coverage-html:
    EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib --html

# Format check
fmt-check:
    cargo fmt --check

# Format
fmt:
    cargo fmt

# Full quality gate: test + clippy
gate: test lint

# --- Daemon ---

# Start daemon
daemon-start port="7878":
    cargo run --bin plicod -- start --port {{port}}

# Stop daemon
daemon-stop:
    cargo run --bin plicod -- stop

# Daemon status
daemon-status:
    cargo run --bin plicod -- status

# --- CLI ---

# Run CLI in embedded mode
cli *args:
    cargo run --bin aicli -- --embedded {{args}}

# Run CLI against daemon
cli-remote *args:
    cargo run --bin aicli -- {{args}}

# --- Benchmark ---

# Run full benchmark suite
bench:
    cd benchmarks && ./scripts/run_full_benchmark.sh

# Run single benchmark suite
bench-suite name:
    cd benchmarks && ./scripts/run_suite.sh {{name}}

# --- Utilities ---

# Count source lines
loc:
    @find src/ -name '*.rs' | xargs wc -l | tail -1

# Count tests
test-count:
    @EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib 2>&1 | grep "test result" | head -1

# Clean build artifacts
clean:
    cargo clean

# Pre-commit check: format + lint + test
pre-commit: fmt lint test
