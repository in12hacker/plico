#!/bin/bash
# One fresh-vault smoke run by default; five runs are an explicit shadow campaign.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BENCH_DIR="$PROJECT_ROOT/benchmarks"
PYTHON="$BENCH_DIR/.venv/bin/python"
SOURCE_PLICOD="$PROJECT_ROOT/target/release/plicod"
OUTPUT_PARENT="$BENCH_DIR/results"
PREPROCESS_TIMEOUT="${PREPROCESS_TIMEOUT:-300}"
RUN_CLASS="${PLICO_BENCH_RUN_CLASS:-research}"
DRY_RUN=false
RUNS=1
PLICOD_PID=""
ACTIVE_VAULT=""
export PLICO_COGNITIVE_PIPELINE_MAX_IN_FLIGHT=3
export PLICO_COGNITIVE_PIPELINE_QUEUE_CAPACITY=8192
export PLICO_KG_AUTO_EXTRACT=false

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

if [[ "$RUNS" != "1" && "$RUNS" != "5" ]]; then
    echo "ERROR: --runs must be exactly 1 (smoke) or 5 (shadow variance)"
    exit 1
fi

if [[ "$RUN_CLASS" != "regression" && "$RUN_CLASS" != "research" ]]; then
    echo "ERROR: full benchmark run class must be regression or research"
    exit 1
fi
if [[ ! -x "$PYTHON" ]]; then
    echo "ERROR: benchmarks/.venv is unavailable; run uv sync --extra dev"
    exit 1
fi

if [[ "$DRY_RUN" == false ]]; then
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
fi

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

if [[ "$DRY_RUN" == false ]]; then
    cargo build --manifest-path "$PROJECT_ROOT/Cargo.toml" --release --bin plicod
    if [[ ! -x "$SOURCE_PLICOD" ]]; then
        echo "ERROR: release plicod was not produced"
        exit 1
    fi
fi

mkdir -p "$OUTPUT_PARENT"
OUTPUT_PARENT="$(cd "$OUTPUT_PARENT" && pwd -P)"
CAMPAIGN_ID="$($PYTHON -c 'import uuid; print(uuid.uuid4())')"
CAMPAIGN_DIR="$OUTPUT_PARENT/campaign-$CAMPAIGN_ID"
if [[ -e "$CAMPAIGN_DIR" ]]; then
    echo "ERROR: campaign output collision"
    exit 1
fi
mkdir -m 700 "$CAMPAIGN_DIR"
SEALED_PLICOD="$CAMPAIGN_DIR/plicod"
if [[ "$DRY_RUN" == false ]]; then
    install -m 700 "$SOURCE_PLICOD" "$SEALED_PLICOD"
fi

start_fresh_daemon() {
    local suite=$1
    local ordinal=$2
    ACTIVE_VAULT="$(mktemp -d "/tmp/plico-benchmark-${suite}-${ordinal}.XXXXXX")"
    chmod 700 "$ACTIVE_VAULT"
    if find "$ACTIVE_VAULT" -mindepth 1 -print -quit | grep -q .; then
        echo "ERROR: fresh vault was not empty before daemon startup"
        exit 1
    fi
    local daemon_log="$CAMPAIGN_DIR/${suite}.run-${ordinal}.plicod.log"
    if [[ "$DRY_RUN" == false ]]; then
        : > "$daemon_log"
        chmod 600 "$daemon_log"
        env -u PLICO_READER_API_KEY -u PLICO_JUDGE_API_KEY \
            -u PLICO_COMPILER_API_KEY -u OPENAI_API_KEY \
            -u DEEPSEEK_API_KEY \
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
    fi
}

stop_fresh_daemon() {
    cleanup_runtime
}

run_one() {
    local suite=$1
    local ordinal=$2
    local run_id
    run_id="$($PYTHON -c 'import uuid; print(uuid.uuid4())')"
    local output="$CAMPAIGN_DIR/${suite}.run-${ordinal}"
    local journal="$CAMPAIGN_DIR/llm-journal-$run_id"
    mkdir -m 700 "$journal"
    start_fresh_daemon "$suite" "$ordinal"
    echo "suite=$suite independent_run=$ordinal/$RUNS run_id=$run_id"
    if [[ "$DRY_RUN" == true ]]; then
        if [[ "$suite" == "conversational-qa" ]]; then
            echo "PLICO_BENCH_RUN_ID=$run_id PLICO_LLM_RUN_ID=$run_id PLICO_LLM_ATTEMPT_JOURNAL_DIR=$journal $PYTHON -m plico_benchmarks run $suite --samples 50 --seed 42 --uds <fresh-vault>/plico.sock --output $output"
        else
            echo "PLICO_BENCH_RUN_ID=$run_id PLICO_LLM_RUN_ID=$run_id PLICO_LLM_ATTEMPT_JOURNAL_DIR=$journal $PYTHON -m plico_benchmarks run $suite --uds <fresh-vault>/plico.sock --output $output"
        fi
    elif [[ "$suite" == "conversational-qa" ]]; then
        PLICO_BENCH_RUN_CLASS="$RUN_CLASS" \
            PLICO_BENCH_RUN_ID="$run_id" \
            PLICO_LLM_RUN_ID="$run_id" \
            PLICO_LLM_ATTEMPT_JOURNAL_DIR="$journal" \
            "$PYTHON" -m plico_benchmarks run "$suite" \
            --samples 50 \
            --seed 42 \
            --uds "$ACTIVE_VAULT/plico.sock" \
            --output "$output" \
            --preprocess-timeout "$PREPROCESS_TIMEOUT"
    else
        env -u PLICO_READER_API_KEY -u PLICO_JUDGE_API_KEY \
            -u PLICO_COMPILER_API_KEY -u OPENAI_API_KEY \
            -u DEEPSEEK_API_KEY \
            PLICO_BENCH_RUN_CLASS="$RUN_CLASS" \
            PLICO_BENCH_RUN_ID="$run_id" \
            PLICO_LLM_RUN_ID="$run_id" \
            PLICO_LLM_ATTEMPT_JOURNAL_DIR="$journal" \
            "$PYTHON" -m plico_benchmarks run "$suite" \
            --uds "$ACTIVE_VAULT/plico.sock" \
            --output "$output" \
            --preprocess-timeout "$PREPROCESS_TIMEOUT"
    fi
    stop_fresh_daemon
}

cd "$BENCH_DIR"
SUITES=(performance retrieval memory-recall-lexical conversational-qa)
for suite in "${SUITES[@]}"; do
    for ordinal in $(seq 1 "$RUNS"); do
        run_one "$suite" "$ordinal"
    done
done

if [[ "$DRY_RUN" == false && "$RUNS" == "5" ]]; then
    comparison_args=()
    for ordinal in 1 2 3 4 5; do
        comparison_args+=(--result "$CAMPAIGN_DIR/retrieval.run-${ordinal}")
    done
    "$PYTHON" -m plico_benchmarks compare-shadow \
        "${comparison_args[@]}" \
        --candidate plico_object_search \
        --reference bm25_only \
        --metric recall@10 \
        --output "$CAMPAIGN_DIR/retrieval.shadow"
fi

echo "campaign_id=$CAMPAIGN_ID"
echo "results=$CAMPAIGN_DIR"
