#!/bin/bash
export PLICO_CHUNKING=fixed
export PLICO_AUTO_SUMMARIZE=true
export EMBEDDING_API_BASE=http://127.0.0.1:18921/v1
export LLM_URL=http://127.0.0.1:18920
export LLM_MODEL=gemma-4-26B-A4B-it-Q4_K_M.gguf

./target/release/plicod --port 7878 --root /tmp/plico-v40-final > plicod_v40_final.log 2>&1 &
PID=$!

echo "Waiting for plicod to start..."
for i in {1..60}; do
    if netstat -tln | grep -q 7878; then
        echo "plicod is ready."
        break
    fi
    sleep 1
done

source bench/.venv/bin/activate
echo "Running KG Bench..."
python3 bench/kg_bench/kg_bench.py 50
echo "Running Micro Bench..."
python3 bench/perf/micro_bench.py
echo "Running MemoryAgentBench AR..."
python3 bench/memoryagentbench/ar_bench.py
echo "Running LoCoMo Bench..."
python3 bench/locomo/plico_locomo_bench.py --max-conv 20
echo "Running LongMemEval Bench..."
python3 bench/longmemeval/plicod_e2e_bench.py --questions 50
python3 bench/report/generate_report.py

kill $PID
