#!/bin/bash
# Run all benchmark suites
set -e

HOST="${PLICO_HOST:-127.0.0.1}"
PORT="${PLICO_PORT:-7878}"

cd "$(dirname "$0")/.."

echo "Running all benchmark suites (plicod at $HOST:$PORT)"
uv run python -m plico_benchmarks run-all --host "$HOST" --port "$PORT"
