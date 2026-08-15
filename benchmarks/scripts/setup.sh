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

echo ""
echo "Setup complete. Run a suite:"
echo "  uv run python -m plico_benchmarks list"
echo "  uv run python -m plico_benchmarks run performance"
