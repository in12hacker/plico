#!/bin/bash
# Download all-MiniLM-L6-v2 ONNX model from HuggingFace for OrtEmbeddingBackend.
#
# Usage:
#   PLICO_MODEL_DIR=./models/all-MiniLM-L6-v2 ./scripts/download_model.sh
#
# Required environment:
#   PLICO_MODEL_DIR  — model directory (default: ./models/all-MiniLM-L6-v2)
#
# The script downloads:
#   - model.onnx          — the ONNX model (~90MB)
#   - tokenizer.json      — HuggingFace tokenizer config
#   - tokenizer_config.json — tokenizer configuration
#
# Note: This script requires `pip install huggingface_hub` to download models.
#   Or you can use `huggingface-cli download` directly.

set -euo pipefail

MODEL_DIR="${PLICO_MODEL_DIR:-./models/all-MiniLM-L6-v2}"
MODEL_ID="sentence-transformers/all-MiniLM-L6-v2"

echo "=== Downloading all-MiniLM-L6-v2 ONNX model ==="
echo "Model ID: ${MODEL_ID}"
echo "Target dir: ${MODEL_DIR}"
echo ""

mkdir -p "${MODEL_DIR}"

# Check for huggingface_hub
if ! command -v huggingface-cli &> /dev/null && ! python3 -c "from huggingface_hub import hf_hub_download" 2> /dev/null; then
    echo "Installing huggingface_hub..."
    pip install -q huggingface_hub
fi

# Download tokenizer files using Python
echo "Downloading tokenizer.json..."
python3 - <<'PYEOF'
import os
import sys
from huggingface_hub import hf_hub_download

model_id = "sentence-transformers/all-MiniLM-L6-v2"
model_dir = os.environ.get("PLICO_MODEL_DIR", "./models/all-MiniLM-L6-v2")

files = ["tokenizer.json", "tokenizer_config.json", "special_tokens_map.json", "config.json"]
for f in files:
    try:
        path = hf_hub_download(repo_id=model_id, filename=f, local_dir=model_dir)
        print(f"  downloaded: {f}")
    except Exception as e:
        print(f"  warning: could not download {f}: {e}", file=sys.stderr)
PYEOF

# The ONNX model is not directly available in the sentence-transformers repo.
# We need to convert it using optimum-cli or download from a provider that hosts the ONNX.
echo ""
echo "Downloading model.onnx (ONNX export from sentence-transformers)..."

# Try to get the ONNX model from the optimum export or a mirror
# The model can be exported using: optimum-cli export onnx --model sentence-transformers/all-MiniLM-L6-v2 ./model_dir
if command -v optimum-cli &> /dev/null; then
    echo "Using optimum-cli to export ONNX model..."
    optimum-cli export onnx --model "${MODEL_ID}" "${MODEL_DIR}"
else
    echo "optimum-cli not found. To get model.onnx, either:"
    echo "  1. pip install optimum && optimum-cli export onnx --model ${MODEL_ID} ${MODEL_DIR}"
    echo "  2. Or download pre-exported ONNX from:"
    echo "     https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/tree/main"
    echo ""
    echo "Downloading config.json and other metadata..."
    python3 - <<'PYEOF'
import os
import sys
from huggingface_hub import hf_hub_download

model_id = "sentence-transformers/all-MiniLM-L6-v2"
model_dir = os.environ.get("PLICO_MODEL_DIR", "./models/all-MiniLM-L6-v2")

# Try to download the onnx model file (some repos have it pre-exported)
try:
    path = hf_hub_download(repo_id=model_id, filename="model.onnx", local_dir=model_dir)
    print(f"  downloaded: model.onnx")
except Exception as e:
    print(f"  note: model.onnx not pre-exported in repo. Run:")
    print(f"    pip install optimum")
    print(f"    optimum-cli export onnx --model {model_id} {model_dir}", file=sys.stderr)
PYEOF
fi

echo ""
echo "=== Done ==="
echo "Model files in: ${MODEL_DIR}"
ls -lh "${MODEL_DIR}" || true
