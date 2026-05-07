#!/bin/bash
# Plico Model Server Launch Script — Soul v3.0 Benchmark Stack
# Hardware: NVIDIA GB10 (128GB unified memory)
# Models: Gemma4-26B (LLM) + Qwen3-Embedding-0.6B (Embedding) + bge-reranker-v2-m3 (Reranker)

set -e

LLAMA_BIN="/home/leo/llama.cpp/build/bin/llama-server"
MODEL_DIR="/home/leo/models"
LOG_DIR="${HOME}/.plico/logs"
mkdir -p "$LOG_DIR"

# Check if servers already running
if lsof -i:18920 >/dev/null 2>&1; then
    echo "[WARN] LLM server already on port 18920"
else
    echo "[START] LLM (Gemma 4 26B) -> port 18920..."
    nohup "$LLAMA_BIN" \
        -m "$MODEL_DIR/gemma-4-26B-A4B-it-Q4_K_M.gguf" \
        --port 18920 \
        --reasoning off \
        -ngl 99 \
        -c 32768 \
        -ub 1024 \
        --host 127.0.0.1 \
        > "$LOG_DIR/llm_18920.log" 2>&1 &
    sleep 3
    if lsof -i:18920 >/dev/null 2>&1; then
        echo "[OK] LLM server ready on 18920"
    else
        echo "[FAIL] LLM server failed to start, check $LOG_DIR/llm_18920.log"
        exit 1
    fi
fi

if lsof -i:18921 >/dev/null 2>&1; then
    echo "[WARN] Embedding server already on port 18921"
else
    echo "[START] Embedding (Qwen3-0.6B) -> port 18921..."
    nohup "$LLAMA_BIN" \
        -m "$MODEL_DIR/Qwen3-Embedding-0.6B-Q8_0.gguf" \
        --port 18921 \
        --embedding \
        -ngl 99 \
        --pooling mean \
        -ub 2048 \
        --host 127.0.0.1 \
        > "$LOG_DIR/embedding_18921.log" 2>&1 &
    sleep 3
    if lsof -i:18921 >/dev/null 2>&1; then
        echo "[OK] Embedding server ready on 18921"
    else
        echo "[FAIL] Embedding server failed to start, check $LOG_DIR/embedding_18921.log"
        exit 1
    fi
fi

if lsof -i:18926 >/dev/null 2>&1; then
    echo "[WARN] Reranker server already on port 18926"
else
    echo "[START] Reranker (bge-reranker-v2-m3) -> port 18926..."
    nohup "$LLAMA_BIN" \
        -m "$MODEL_DIR/bge-reranker-v2-m3-q4_k_m.gguf" \
        --port 18926 \
        -ngl 99 \
        --host 127.0.0.1 \
        > "$LOG_DIR/reranker_18926.log" 2>&1 &
    sleep 3
    if lsof -i:18926 >/dev/null 2>&1; then
        echo "[OK] Reranker server ready on 18926"
    else
        echo "[FAIL] Reranker server failed to start, check $LOG_DIR/reranker_18926.log"
        exit 1
    fi
fi

echo ""
echo "=== All model servers running ==="
echo "  LLM:       http://127.0.0.1:18920  (Gemma 4 26B)"
echo "  Embedding: http://127.0.0.1:18921  (Qwen3-0.6B)"
echo "  Reranker:  http://127.0.0.1:18926  (bge-reranker-v2-m3)"
echo "  Logs:      $LOG_DIR"
