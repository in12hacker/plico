#!/bin/bash
# Run a single benchmark suite
set -e

SUITE="${1:-performance}"
HOST="${PLICO_HOST:-127.0.0.1}"
PORT="${PLICO_PORT:-7878}"

cd "$(dirname "$0")/.."

echo "Running suite: $SUITE (plicod at $HOST:$PORT)"
uv run python -m plico_benchmarks run "$SUITE" --host "$HOST" --port "$PORT"
