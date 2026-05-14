#!/bin/bash
# Full benchmark run with preprocessing phase — stable version
# Usage: ./scripts/run_full_benchmark.sh [--dry-run] [--skip-jina-v5] [--preprocess-timeout N]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BENCH_DIR="$PROJECT_ROOT/benchmarks"
PLICOD="$PROJECT_ROOT/target/release/plicod"

HOST="${PLICO_HOST:-127.0.0.1}"
PORT="${PLICO_PORT:-7878}"
ROOT="/tmp/plico-bench-$(date +%Y%m%d-%H%M%S)"
PREPROCESS_TIMEOUT="${PREPROCESS_TIMEOUT:-300}"
DRY_RUN=false
SKIP_JINA_V5=false
FAILED_SUITES=()

function usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --dry-run              Print configuration and exit without running
  --skip-jina-v5         Skip Jina v5 embedding config
  --preprocess-timeout N Seconds to wait for indexing after ingest (default: 180)
  --help                 Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --skip-jina-v5) SKIP_JINA_V5=true; shift ;;
        --preprocess-timeout)
            PREPROCESS_TIMEOUT="$2"
            shift 2
            ;;
        --help) usage; exit 0 ;;
        *) echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

function cleanup() {
    local exit_code=$?
    echo ""
    echo "=== Cleanup ==="
    if [[ -n "${PLICOD_PID:-}" ]] && kill -0 "$PLICOD_PID" 2>/dev/null; then
        echo "Stopping plicod (PID $PLICOD_PID)..."
        kill -TERM "$PLICOD_PID" 2>/dev/null || true
        wait "$PLICOD_PID" 2>/dev/null || true
    fi
    if [[ -d "$ROOT" ]]; then
        rm -rf "$ROOT"
    fi
    if [[ ${#FAILED_SUITES[@]} -gt 0 ]]; then
        echo ""
        echo "FAILED SUITES:"
        for f in "${FAILED_SUITES[@]}"; do
            echo "  - $f"
        done
    fi
    exit $exit_code
}
trap cleanup EXIT

function verify_server() {
    local url=$1
    local name=$2
    local timeout=${3:-30}
    echo -n "Verifying $name at $url ... "
    for ((i = 0; i < timeout; i++)); do
        if curl -sf "$url/models" >/dev/null 2>&1 || curl -sf "$url/health" >/dev/null 2>&1; then
            echo "OK"
            return 0
        fi
        sleep 1
    done
    echo "FAILED (timeout ${timeout}s)"
    return 1
}

function start_plicod() {
    local embed_base=$1
    rm -rf "$ROOT"
    mkdir -p "$ROOT"

    export EMBEDDING_API_BASE="$embed_base"
    export OPENAI_API_BASE="http://127.0.0.1:18920/v1"
    export LLAMA_URL="http://127.0.0.1:18920/v1"
    export LLM_BACKEND=openai
    export LLM_MODEL=gemma-4-26B-A4B-it-Q4_K_M.gguf
    export PLICO_KG_AUTO_EXTRACT=false

    "$PLICOD" --port "$PORT" --root "$ROOT" > /tmp/plicod_bench.log 2>&1 &
    PLICOD_PID=$!
    echo "plicod started (PID $PLICOD_PID), root=$ROOT, embed=$embed_base"

    for ((i = 0; i < 30; i++)); do
        if python3 -c "
import socket, struct, json, sys
try:
    s = socket.create_connection(('${HOST}', ${PORT}), timeout=2)
    payload = json.dumps({'method': 'health_report'}).encode()
    s.sendall(struct.pack('>I', len(payload)) + payload)
    header = s.recv(4)
    length = struct.unpack('>I', header)[0]
    resp = json.loads(s.recv(length))
    s.close()
    sys.exit(0 if resp.get('ok') else 1)
except Exception:
    sys.exit(1)
" 2>/dev/null; then
            echo "plicod ready"
            return 0
        fi
        sleep 1
    done
    echo "plicod failed to start within 30s"
    return 1
}

function kill_plicod() {
    if [[ -n "${PLICOD_PID:-}" ]] && kill -0 "$PLICOD_PID" 2>/dev/null; then
        kill -TERM "$PLICOD_PID" 2>/dev/null || true
        wait "$PLICOD_PID" 2>/dev/null || true
    fi
    PLICOD_PID=""
}

function run_suite() {
    local suite=$1
    local samples=$2
    local embed_name=$3
    local output="results/${suite}_${embed_name}_v44.json"

    echo ""
    echo "========================================"
    echo "Suite: $suite | samples=$samples | embed=$embed_name"
    echo "========================================"

    if [[ "$DRY_RUN" == true ]]; then
        echo "[DRY-RUN] uv run python -m plico_benchmarks run $suite --host $HOST --port $PORT --samples $samples --output $output --preprocess-timeout $PREPROCESS_TIMEOUT"
        return 0
    fi

    if ! uv run python -m plico_benchmarks run "$suite" \
        --host "$HOST" --port "$PORT" \
        --samples "$samples" \
        --output "$output" \
        --preprocess-timeout "$PREPROCESS_TIMEOUT" 2>&1; then
        echo "ERROR: suite $suite failed"
        FAILED_SUITES+=("$suite ($embed_name): runtime error")
        return 1
    fi

    # Validate output
    if [[ ! -s "$output" ]]; then
        echo "WARNING: $output is empty"
        FAILED_SUITES+=("$suite ($embed_name): empty output")
        return 1
    fi

    if ! python3 -c "import json,sys; d=json.load(open('$output')); sys.exit(0 if d.get('metrics') else 1)" 2>/dev/null; then
        echo "WARNING: $output missing metrics"
        FAILED_SUITES+=("$suite ($embed_name): missing metrics")
        return 1
    fi

    echo "Suite $suite completed: $output"
}

# ===== Main =====
echo "=== Plico Full Benchmark ==="
echo "plicod binary: $PLICOD"
echo "host: $HOST:$PORT"
echo "root: $ROOT"
echo "preprocess timeout: ${PREPROCESS_TIMEOUT}s"
echo "dry-run: $DRY_RUN"
echo "skip-jina-v5: $SKIP_JINA_V5"
echo ""

if [[ ! -x "$PLICOD" ]]; then
    echo "ERROR: plicod binary not found or not executable: $PLICOD"
    echo "Run: cargo build --release --bin plicod"
    exit 1
fi

if [[ "$DRY_RUN" == false ]]; then
    verify_server "http://127.0.0.1:18920/v1" "LLM (18920)" 30
    verify_server "http://127.0.0.1:18921/v1" "Embedding Qwen3 (18921)" 30
    if [[ "$SKIP_JINA_V5" == false ]]; then
        verify_server "http://127.0.0.1:18922/v1" "Embedding Jina v5 (18922)" 30
    fi
fi

cd "$BENCH_DIR"
mkdir -p results

# ===== Config A: Qwen3 =====
start_plicod "http://127.0.0.1:18921/v1"
run_suite "performance"       100 "qwen3"
run_suite "memory-crud"       100 "qwen3"
run_suite "conversational-qa"  40 "qwen3"
run_suite "retrieval"          30 "qwen3"
run_suite "kg-reasoning"       50 "qwen3"
run_suite "temporal-reasoning" 30 "qwen3"
kill_plicod

# ===== Config B: Jina v5 =====
if [[ "$SKIP_JINA_V5" == false ]]; then
    start_plicod "http://127.0.0.1:18922/v1"
    run_suite "performance"       100 "jina_v5"
    run_suite "memory-crud"       100 "jina_v5"
    run_suite "conversational-qa"  40 "jina_v5"
    run_suite "retrieval"          30 "jina_v5"
    run_suite "kg-reasoning"       50 "jina_v5"
    run_suite "temporal-reasoning" 30 "jina_v5"
    kill_plicod
fi

# ===== Generate report =====
echo ""
echo "========================================"
echo "Generating comparison report..."
echo "========================================"
if [[ "$DRY_RUN" == false ]]; then
    uv run python -m plico_benchmarks report \
        --input results/ \
        --output docs/benchmark_report_v44_comparison.md 2>&1 || echo "REPORT_FAILED"
fi

echo ""
echo "All done. Results in $BENCH_DIR/results/"
