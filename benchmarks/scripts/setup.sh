#!/bin/bash
# Setup script for Plico Benchmark Framework
set -e

cd "$(dirname "$0")/.."

echo "=== Plico Benchmark Setup ==="

# Check uv
if ! command -v uv &> /dev/null; then
    echo "ERROR: uv not found. Please install uv: https://docs.astral.sh/uv/getting-started/installation/"
    exit 1
fi

echo "Installing Python dependencies..."
uv sync

echo "Checking legacy dataset fallback..."
LEGACY_DIRS=(
    "../bench/data"
    "../.runtime/bench_legacy/data"
)
for dir in "${LEGACY_DIRS[@]}"; do
    if [ -d "$dir" ]; then
        echo "  Found legacy data at $dir"
    fi
done

echo ""
echo "Setup complete. Run a suite:"
echo "  uv run python -m plico_benchmarks list"
echo "  uv run python -m plico_benchmarks run performance"
