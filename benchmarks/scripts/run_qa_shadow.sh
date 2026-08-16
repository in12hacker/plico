#!/bin/bash
# One-run smoke or five-run QA shadow with a statically bounded paid-attempt budget.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BENCH_DIR="$PROJECT_ROOT/benchmarks"
PYTHON="$BENCH_DIR/.venv/bin/python"
SOURCE_PLICOD="$PROJECT_ROOT/target/release/plicod"
OUTPUT_PARENT="$BENCH_DIR/results"
PREPROCESS_TIMEOUT="${PREPROCESS_TIMEOUT:-1800}"
SAMPLES=50
RUNS=5
DRY_RUN=false
AUTHORIZED_MAX_USD="${PLICO_QA_SHADOW_AUTHORIZED_MAX_USD:-100}"
READER_MAX_USD_PER_RUN="${PLICO_QA_SHADOW_READER_MAX_USD_PER_RUN:-0.10}"
JUDGE_MAX_USD_PER_RUN="${PLICO_QA_SHADOW_JUDGE_MAX_USD_PER_RUN:-0.15}"
OBSERVED_COST_PER_RUN_USD="0.0251223560"
PLICOD_PID=""
ACTIVE_VAULT=""
EXPECTED_HEAD=""

usage() {
    echo "Usage: $0 [--dry-run] [--runs 1|5] [--output-parent DIR] [--preprocess-timeout N]"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --dry-run) DRY_RUN=true; shift ;;
        --runs) RUNS="$2"; shift 2 ;;
        --output-parent) OUTPUT_PARENT="$2"; shift 2 ;;
        --preprocess-timeout) PREPROCESS_TIMEOUT="$2"; shift 2 ;;
        --help) usage; exit 0 ;;
        *) echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

if [[ ! -x "$PYTHON" ]]; then
    echo "ERROR: benchmarks/.venv is unavailable; run uv sync --extra dev"
    exit 1
fi

require_clean_revision() {
    local current_head
    current_head="$(git -C "$PROJECT_ROOT" rev-parse HEAD)"
    if [[ -z "$EXPECTED_HEAD" ]]; then
        EXPECTED_HEAD="$current_head"
    elif [[ "$current_head" != "$EXPECTED_HEAD" ]]; then
        echo "ERROR: repository HEAD changed during the QA shadow campaign"
        exit 1
    fi
    if [[ -n "$(git -C "$PROJECT_ROOT" status --porcelain --untracked-files=all)" ]]; then
        echo "ERROR: QA shadow requires a clean, frozen worktree before paid requests"
        exit 1
    fi
}

if ! "$PYTHON" -c '
from decimal import Decimal, InvalidOperation
import sys
try:
    reader, judge, runs, authorized = map(Decimal, sys.argv[1:])
except (InvalidOperation, ValueError):
    raise SystemExit(2)
if reader <= 0 or judge <= 0 or runs not in {1, 5} or authorized <= 0:
    raise SystemExit(2)
if runs * (reader + judge) > authorized:
    raise SystemExit(3)
' "$READER_MAX_USD_PER_RUN" "$JUDGE_MAX_USD_PER_RUN" "$RUNS" "$AUTHORIZED_MAX_USD"; then
    echo "ERROR: QA shadow per-run budgets are invalid or exceed the authorized campaign cap"
    exit 1
fi

WORST_CASE_USD="$($PYTHON -c '
from decimal import Decimal
import sys
print(format(Decimal(sys.argv[1]) * (Decimal(sys.argv[2]) + Decimal(sys.argv[3])), "f"))
' "$RUNS" "$READER_MAX_USD_PER_RUN" "$JUDGE_MAX_USD_PER_RUN")"
EXPECTED_COST_USD="$($PYTHON -c '
from decimal import Decimal
import sys
print(format(Decimal(sys.argv[1]) * Decimal(sys.argv[2]), "f"))
' "$RUNS" "$OBSERVED_COST_PER_RUN_USD")"

echo "qa_shadow_plan=runs:${RUNS},samples_per_run:${SAMPLES},seed:42"
echo "observed_cost_projection_usd=$EXPECTED_COST_USD"
echo "enforced_worst_case_usd=$WORST_CASE_USD"
echo "authorized_max_usd=$AUTHORIZED_MAX_USD"
if [[ "$RUNS" == 1 ]]; then
    echo "estimated_elapsed_minutes=7-9"
else
    echo "estimated_elapsed_minutes=35-45"
fi

if [[ "$DRY_RUN" == true ]]; then
    echo "dry_run=true; no daemon, provider request, journal, or result directory was created"
    echo "$PYTHON -m plico_benchmarks run conversational-qa --samples 50 --seed 42 --uds <fresh-vault>/plico.sock --output <campaign>/conversational-qa.run-N"
    if [[ "$RUNS" == 5 ]]; then
        echo "$PYTHON -m plico_benchmarks compare-qa-shadow --result <run-1> ... --result <run-5> --output <campaign>/conversational-qa.shadow"
    else
        echo "$PYTHON -c <deep-verify-result> <campaign>/conversational-qa.run-1"
    fi
    exit 0
fi

export PLICO_READER_MAX_USD="$READER_MAX_USD_PER_RUN"
export PLICO_JUDGE_MAX_USD="$JUDGE_MAX_USD_PER_RUN"
export PLICO_BENCH_REQUIRE_REAL_EMBEDDING=1
export PLICO_BENCH_RUN_CLASS=research
export PLICO_COGNITIVE_PIPELINE_MAX_IN_FLIGHT=3
export PLICO_COGNITIVE_PIPELINE_QUEUE_CAPACITY=8192
export PLICO_KG_AUTO_EXTRACT=false

require_clean_revision

for role in READER JUDGE; do
    for suffix in PROVIDER API_BASE MODEL API_KEY TIMEOUT_SECONDS MAX_TOKENS MAX_ATTEMPTS \
        THINKING REASONING_EFFORT TEMPERATURE TOP_P MAX_REQUESTS MAX_INPUT_TOKENS \
        MAX_OUTPUT_TOKENS MAX_USD; do
        variable="PLICO_${role}_${suffix}"
        if [[ -z "${!variable:-}" ]]; then
            echo "ERROR: missing exact DeepSeek role setting $variable"
            exit 1
        fi
    done
done

cargo build --manifest-path "$PROJECT_ROOT/Cargo.toml" --release --bin plicod
if [[ ! -x "$SOURCE_PLICOD" ]]; then
    echo "ERROR: release plicod was not produced"
    exit 1
fi
require_clean_revision

mkdir -p "$OUTPUT_PARENT"
OUTPUT_PARENT="$(cd "$OUTPUT_PARENT" && pwd -P)"
CAMPAIGN_ID="$($PYTHON -c 'import uuid; print(uuid.uuid4())')"
CAMPAIGN_DIR="$OUTPUT_PARENT/qa-shadow-$CAMPAIGN_ID"
mkdir -m 700 "$CAMPAIGN_DIR"
SEALED_PLICOD="$CAMPAIGN_DIR/plicod"
install -m 700 "$SOURCE_PLICOD" "$SEALED_PLICOD"

cleanup_runtime() {
    if [[ -n "$PLICOD_PID" ]] && kill -0 "$PLICOD_PID" 2>/dev/null; then
        kill -TERM "$PLICOD_PID" 2>/dev/null || true
        wait "$PLICOD_PID" 2>/dev/null || true
    fi
    PLICOD_PID=""
    if [[ -n "$ACTIVE_VAULT" && -d "$ACTIVE_VAULT" ]]; then
        rm -rf "$ACTIVE_VAULT"
    fi
    ACTIVE_VAULT=""
}
trap cleanup_runtime EXIT

start_fresh_daemon() {
    local ordinal=$1
    ACTIVE_VAULT="$(mktemp -d "/tmp/plico-qa-shadow-${ordinal}.XXXXXX")"
    chmod 700 "$ACTIVE_VAULT"
    if find "$ACTIVE_VAULT" -mindepth 1 -print -quit | grep -q .; then
        echo "ERROR: fresh vault was not empty before daemon startup"
        exit 1
    fi
    local daemon_log="$CAMPAIGN_DIR/conversational-qa.run-${ordinal}.plicod.log"
    : > "$daemon_log"
    chmod 600 "$daemon_log"
    env -u PLICO_READER_API_KEY -u PLICO_JUDGE_API_KEY \
        -u PLICO_COMPILER_API_KEY -u OPENAI_API_KEY -u DEEPSEEK_API_KEY \
        "$SEALED_PLICOD" --port 0 --root "$ACTIVE_VAULT" >"$daemon_log" 2>&1 &
    PLICOD_PID=$!
    for _ in $(seq 1 100); do
        if [[ -S "$ACTIVE_VAULT/plico.sock" ]]; then
            return 0
        fi
        if ! kill -0 "$PLICOD_PID" 2>/dev/null; then
            echo "ERROR: plicod exited before creating its UDS"
            exit 1
        fi
        sleep 0.1
    done
    echo "ERROR: plicod did not create its UDS"
    exit 1
}

cd "$BENCH_DIR"
for ordinal in $(seq 1 "$RUNS"); do
    require_clean_revision
    run_id="$($PYTHON -c 'import uuid; print(uuid.uuid4())')"
    output="$CAMPAIGN_DIR/conversational-qa.run-${ordinal}"
    journal="$CAMPAIGN_DIR/llm-journal-$run_id"
    mkdir -m 700 "$journal"
    start_fresh_daemon "$ordinal"
    echo "suite=conversational-qa independent_run=$ordinal/$RUNS run_id=$run_id"
    PLICO_BENCH_RUN_ID="$run_id" \
        PLICO_LLM_RUN_ID="$run_id" \
        PLICO_LLM_ATTEMPT_JOURNAL_DIR="$journal" \
        "$PYTHON" -m plico_benchmarks run conversational-qa \
        --samples "$SAMPLES" \
        --seed 42 \
        --uds "$ACTIVE_VAULT/plico.sock" \
        --output "$output" \
        --preprocess-timeout "$PREPROCESS_TIMEOUT"
    cleanup_runtime
done

require_clean_revision

if [[ "$RUNS" == 5 ]]; then
    comparison_args=()
    for ordinal in $(seq 1 "$RUNS"); do
        comparison_args+=(--result "$CAMPAIGN_DIR/conversational-qa.run-${ordinal}")
    done
    "$PYTHON" -m plico_benchmarks compare-qa-shadow \
        "${comparison_args[@]}" \
        --seed 42 \
        --output "$CAMPAIGN_DIR/conversational-qa.shadow"
else
    "$PYTHON" -c \
        'import sys; from pathlib import Path; from plico_benchmarks.core.result_artifact import verify_result_directory; verify_result_directory(Path(sys.argv[1]))' \
        "$CAMPAIGN_DIR/conversational-qa.run-1"
fi

echo "campaign_id=$CAMPAIGN_ID"
echo "results=$CAMPAIGN_DIR"
