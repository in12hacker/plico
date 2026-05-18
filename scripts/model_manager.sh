#!/bin/bash
# model_manager.sh — Unified model server management with health checks & auto-recovery
#
# Usage:
#   model_manager.sh start   [llm|embedding|reranker|all]
#   model_manager.sh stop    [llm|embedding|reranker|all]
#   model_manager.sh status
#   model_manager.sh health  [--fix]
#   model_manager.sh restart [llm|embedding|reranker]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LLAMA_BIN="${LLAMA_BIN:-/home/leo/llama.cpp/build/bin/llama-server}"
MODEL_DIR="${MODEL_DIR:-/home/leo/models}"
LOG_DIR="${LOG_DIR:-${HOME}/.plico/logs}"

mkdir -p "$LOG_DIR"

# ── Server Definitions ──────────────────────────────────────────────
# Format: name|port|model_file|extra_flags|health_type
#   health_type: models (GET /v1/models), rerank (POST /v1/rerank)
SERVERS=(
    "llm|18920|gemma-4-26B-A4B-it-Q4_K_M.gguf|--reasoning off -c 32768 -ub 1024|models"
    "embedding|18921|Qwen3-Embedding-0.6B-Q8_0.gguf|--embedding --pooling mean -ub 2048|models"
    "reranker|18926|bge-reranker-v2-m3-q4_k_m.gguf|--reranking|rerank"
)

# ── Helpers ─────────────────────────────────────────────────────────

get_server_def() {
    local name=$1
    for def in "${SERVERS[@]}"; do
        IFS='|' read -r sname sport smodel sflags shealth <<< "$def"
        if [[ "$sname" == "$name" ]]; then
            echo "$def"
            return 0
        fi
    done
    return 1
}

is_port_open() {
    local port=$1
    lsof -i:"$port" >/dev/null 2>&1
}

get_pid_on_port() {
    local port=$1
    lsof -ti :"$port" 2>/dev/null | head -1
}

# Returns 0 if healthy, 1 if unhealthy, 2 if 501 (reranker-specific)
check_health() {
    local port=$1
    local health_type=$2
    local url="http://127.0.0.1:${port}/v1"

    if [[ "$health_type" == "rerank" ]]; then
        # Send minimal rerank request — detects missing --reranking flag
        local resp
        resp=$(curl -sf -w "\n%{http_code}" -X POST "$url/rerank" \
            -H "Content-Type: application/json" \
            -d '{"model":"test","query":"test","documents":["test"],"top_n":1}' 2>/dev/null) || true
        local http_code
        http_code=$(echo "$resp" | tail -1)
        if [[ "$http_code" == "501" ]]; then
            return 2  # 501 = missing --reranking
        elif [[ "$http_code" == "200" ]]; then
            return 0
        else
            return 1
        fi
    else
        # Standard /v1/models check
        curl -sf "$url/models" >/dev/null 2>&1
        return $?
    fi
}

start_server() {
    local name=$1
    local def
    def=$(get_server_def "$name") || { echo "Unknown server: $name"; return 1; }

    IFS='|' read -r sname port model flags health_type <<< "$def"

    if is_port_open "$port"; then
        echo "[SKIP] $name already running on port $port"
        return 0
    fi

    echo "[START] $name -> port $port..."
    # shellcheck disable=SC2086
    nohup "$LLAMA_BIN" \
        -m "$MODEL_DIR/$model" \
        --port "$port" \
        -ngl 99 \
        --host 127.0.0.1 \
        $flags \
        > "$LOG_DIR/${name}_${port}.log" 2>&1 &

    local pid=$!
    echo "  PID: $pid, log: $LOG_DIR/${name}_${port}.log"

    # Wait for readiness (up to 60s for model loading)
    for i in $(seq 1 60); do
        sleep 1
        if check_health "$port" "$health_type" 2>/dev/null; then
            echo "[OK] $name ready on port $port (${i}s)"
            return 0
        fi
    done

    echo "[FAIL] $name failed to start within 60s"
    echo "  Check: tail -20 $LOG_DIR/${name}_${port}.log"
    return 1
}

stop_server() {
    local name=$1
    local def
    def=$(get_server_def "$name") || { echo "Unknown server: $name"; return 1; }

    IFS='|' read -r sname port model flags health_type <<< "$def"

    local pid
    pid=$(get_pid_on_port "$port")
    if [[ -z "$pid" ]]; then
        echo "[SKIP] $name not running on port $port"
        return 0
    fi

    echo "[STOP] $name (PID $pid, port $port)..."
    kill -TERM "$pid" 2>/dev/null || true

    # Wait for exit
    for i in $(seq 1 10); do
        if ! is_port_open "$port"; then
            echo "[OK] $name stopped"
            return 0
        fi
        sleep 1
    done

    echo "[WARN] $name did not stop gracefully, sending SIGKILL..."
    kill -9 "$pid" 2>/dev/null || true
    sleep 1
    echo "[OK] $name killed"
}

restart_server() {
    local name=$1
    stop_server "$name"
    sleep 2
    start_server "$name"
}

# ── Commands ────────────────────────────────────────────────────────

cmd_status() {
    echo "=== Model Server Status ==="
    printf "%-12s %-6s %-8s %-10s %s\n" "SERVER" "PORT" "RUNNING" "HEALTH" "PID"
    printf "%-12s %-6s %-8s %-10s %s\n" "------" "----" "-------" "------" "---"

    for def in "${SERVERS[@]}"; do
        IFS='|' read -r name port model flags health_type <<< "$def"
        local running="no" health="—" pid="—"

        if is_port_open "$port"; then
            running="yes"
            pid=$(get_pid_on_port "$port")

            local hr
            if check_health "$port" "$health_type"; then
                health="ok"
            elif [[ $? -eq 2 ]]; then
                health="501!"
            else
                health="fail"
            fi
        fi

        printf "%-12s %-6s %-8s %-10s %s\n" "$name" "$port" "$running" "$health" "$pid"
    done
}

cmd_health() {
    local fix_mode=false
    [[ "${1:-}" == "--fix" ]] && fix_mode=true

    echo "=== Health Check ==="
    local issues=0

    for def in "${SERVERS[@]}"; do
        IFS='|' read -r name port model flags health_type <<< "$def"
        printf "%-12s (port %s): " "$name" "$port"

        if ! is_port_open "$port"; then
            echo "NOT RUNNING"
            if $fix_mode; then
                echo "  → Starting $name..."
                start_server "$name" || true
            fi
            ((issues++))
            continue
        fi

        local hr=0
        check_health "$port" "$health_type" || hr=$?

        if [[ $hr -eq 0 ]]; then
            echo "OK"
        elif [[ $hr -eq 2 && "$name" == "reranker" ]]; then
            echo "501 — missing --reranking flag!"
            ((issues++))
            if $fix_mode; then
                echo "  → Restarting reranker with --reranking..."
                restart_server "reranker"
                # Verify fix
                local verify_hr=0
                check_health "$port" "$health_type" || verify_hr=$?
                if [[ $verify_hr -eq 0 ]]; then
                    echo "  → FIXED: reranker now healthy"
                else
                    echo "  → STILL BROKEN after restart"
                fi
            fi
        else
            echo "UNHEALTHY (health check failed)"
            ((issues++))
            if $fix_mode; then
                echo "  → Restarting $name..."
                restart_server "$name" || true
            fi
        fi
    done

    echo ""
    if [[ $issues -eq 0 ]]; then
        echo "All servers healthy."
        return 0
    else
        echo "$issues issue(s) found."
        return 1
    fi
}

cmd_start() {
    local target="${1:-all}"
    case "$target" in
        all)    for def in "${SERVERS[@]}"; do IFS='|' read -r n _ _ _ _ <<< "$def"; start_server "$n"; done ;;
        llm|embedding|reranker) start_server "$target" ;;
        *)      echo "Usage: $0 start [llm|embedding|reranker|all]"; exit 1 ;;
    esac
}

cmd_stop() {
    local target="${1:-all}"
    case "$target" in
        all)    for def in "${SERVERS[@]}"; do IFS='|' read -r n _ _ _ _ <<< "$def"; stop_server "$n"; done ;;
        llm|embedding|reranker) stop_server "$target" ;;
        *)      echo "Usage: $0 stop [llm|embedding|reranker|all]"; exit 1 ;;
    esac
}

cmd_restart() {
    local target="${1:-}"
    case "$target" in
        llm|embedding|reranker) restart_server "$target" ;;
        *)      echo "Usage: $0 restart [llm|embedding|reranker]"; exit 1 ;;
    esac
}

# ── Main ────────────────────────────────────────────────────────────

case "${1:-}" in
    start)   cmd_start "${2:-all}" ;;
    stop)    cmd_stop "${2:-all}" ;;
    status)  cmd_status ;;
    health)  cmd_health "${2:-}" ;;
    restart) cmd_restart "${2:-}" ;;
    *)       echo "Usage: $0 {start|stop|status|health|restart} [server|--fix]"
             echo ""
             echo "Commands:"
             echo "  start   [llm|embedding|reranker|all]  Start server(s)"
             echo "  stop    [llm|embedding|reranker|all]  Stop server(s)"
             echo "  status                                Show all server status"
             echo "  health  [--fix]                       Health check (with optional auto-fix)"
             echo "  restart [llm|embedding|reranker]      Restart a server"
             exit 1 ;;
esac
