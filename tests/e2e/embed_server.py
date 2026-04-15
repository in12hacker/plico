#!/usr/bin/env python3
"""
Local Embedding Server — Plico Native

Standalone Python server for text embeddings using HuggingFace ONNX Runtime or PyTorch.
Integrated into Plico as a subprocess via stdio JSON-RPC.

Protocol:
    Each request:  JSON-RPC 2.0 request on a single line (stdin)
    Each response: JSON-RPC 2.0 response on a single line (stdout)

Setup:
    pip install transformers huggingface_hub onnxruntime torch
    # Model downloads automatically on first run (~24MB for bge-small-en-v1.5)

Environment:
    EMBEDDING_MODEL_ID  — HuggingFace model (default: BAAI/bge-small-en-v1.5)
    HF_HOME             — HuggingFace cache dir (default: ~/.cache/huggingface)
    LOG_LEVEL           — Logging level (default: WARNING)

Example request:
    {"jsonrpc": "2.0", "id": 1, "method": "embed", "params": {"text": "hello world"}}

Example response:
    {"jsonrpc": "2.0", "id": 1, "result": {"embedding": [0.1, -0.2, ...]}}
"""

import os
import sys
import json
import warnings
import logging
from typing import Optional

warnings.filterwarnings("ignore")

# Configure logging
logging.basicConfig(
    level=getattr(logging, os.environ.get("LOG_LEVEL", "WARNING")),
    format="%(asctime)s embed-server %(levelname)s %(message)s",
)
log = logging.getLogger("embed-server")

MODEL_ID = os.environ.get("EMBEDDING_MODEL_ID", "BAAI/bge-small-en-v1.5")

# ─── Lazy model loading ───────────────────────────────────────────────────────

_pipeline = None
_dimension = None


def _mean_pool(last_hidden_state, attention_mask) -> "numpy.ndarray":
    """Mean pool token embeddings into a single sentence vector. Works with torch or numpy."""
    import numpy as np
    mask_expanded = attention_mask[..., np.newaxis].expand(last_hidden_state.shape).astype(float)
    sum_emb = np.sum(last_hidden_state * mask_expanded, axis=1)
    sum_mask = np.clip(mask_expanded.sum(axis=1), a_min=1e-9, a_max=None)
    return sum_emb / sum_mask


def get_pipeline():
    """Lazy-load the embedding model on first use.

    Tries ONNX Runtime first (CPU-efficient, no GPU needed), then falls back to
    PyTorch. Both require `torch` as a dependency of the optimum.onnxruntime base class.
    """
    global _pipeline, _dimension
    if _pipeline is not None:
        return _pipeline

    log.info(f"Loading model {MODEL_ID} (first inference may take 10-30s for download + init)...")

    # Try ONNX Runtime path (optimum.onnxruntime requires torch as a base class dep)
    try:
        import torch
        from transformers import AutoTokenizer, AutoModel
        from optimum.onnxruntime import ORTModelForFeatureExtraction

        tokenizer = AutoTokenizer.from_pretrained(MODEL_ID)
        model = ORTModelForFeatureExtraction.from_pretrained(
            MODEL_ID,
            export=False,
            provider="CPUExecutionProvider",
        )
        _dimension = model.config.hidden_size
        log.info(f"Model loaded (ONNX Runtime, dim={_dimension})")

        def pipeline(texts):
            nonlocal model, tokenizer
            inputs = tokenizer(texts, return_tensors="pt", padding=True, truncation=True, max_length=512)
            with torch.no_grad():
                outputs = model(**inputs)
            last_hidden = outputs.last_hidden_state.numpy()
            attn_mask = inputs["attention_mask"].numpy()
            return _mean_pool(last_hidden, attn_mask)

        _pipeline = pipeline
        return _pipeline

    except ImportError as e:
        log.warning(f"onnxruntime import failed ({e}), falling back to PyTorch...")
    except Exception as e:
        log.warning(f"ONNX Runtime load failed ({e}), falling back to PyTorch...")

    # PyTorch fallback
    try:
        import torch
        from transformers import AutoTokenizer, AutoModel

        tokenizer = AutoTokenizer.from_pretrained(MODEL_ID)
        model = AutoModel.from_pretrained(MODEL_ID)
        _dimension = model.config.hidden_size
        model.eval()
        log.info(f"Model loaded (PyTorch, dim={_dimension})")

        def pipeline(texts):
            nonlocal model, tokenizer
            inputs = tokenizer(texts, return_tensors="pt", padding=True, truncation=True, max_length=512)
            with torch.no_grad():
                outputs = model(**inputs)
            last_hidden = outputs.last_hidden_state
            attn_mask = inputs["attention_mask"]
            mask_expanded = attn_mask.unsqueeze(-1).expand(last_hidden.size()).float()
            sum_emb = torch.sum(last_hidden * mask_expanded, dim=1)
            sum_mask = mask_expanded.sum(dim=1).clamp(min=1e-9)
            embeddings = (sum_emb / sum_mask).numpy()
            return embeddings

        _pipeline = pipeline
        return _pipeline

    except ImportError as e:
        raise ImportError(
            f"Neither ONNX Runtime nor PyTorch available. "
            f"Install one of:\n"
            f"  pip install optimum onnxruntime torch  # ONNX (recommended, ~50ms/sentence)\n"
            f"  pip install torch transformers        # PyTorch only"
        ) from e
    except Exception as e:
        raise RuntimeError(f"Failed to load model {MODEL_ID}: {e}") from e


def get_dimension() -> int:
    if _dimension is not None:
        return _dimension
    get_pipeline()  # triggers load
    return _dimension or 384


def handle_embed(params: dict) -> dict:
    text = params.get("text", "")
    if not text:
        raise ValueError("text is required")

    pipeline_fn = get_pipeline()
    embeddings = pipeline_fn([text])
    embedding = embeddings[0].tolist()
    return {"embedding": embedding}


def handle_info(_params) -> dict:
    return {
        "model": MODEL_ID,
        "dimension": get_dimension(),
    }


# ─── JSON-RPC dispatch ───────────────────────────────────────────────────────

def handle_request(line: str) -> Optional[str]:
    """Handle a single JSON-RPC request line. Returns response line or None."""
    try:
        req = json.loads(line.strip())
    except json.JSONDecodeError as e:
        return json.dumps({
            "jsonrpc": "2.0",
            "id": None,
            "error": {"code": -32700, "message": f"Parse error: {e}"},
        })

    req_id = req.get("id")
    method = req.get("method")
    params = req.get("params", {})

    if method == "embed":
        try:
            result = handle_embed(params)
            return json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result})
        except Exception as e:
            log.error(f"embed error: {e}")
            return json.dumps({
                "jsonrpc": "2.0", "id": req_id,
                "error": {"code": -32603, "message": str(e)},
            })
    elif method == "info":
        try:
            result = handle_info(params)
            return json.dumps({"jsonrpc": "2.0", "id": req_id, "result": result})
        except Exception as e:
            log.error(f"info error: {e}")
            return json.dumps({
                "jsonrpc": "2.0", "id": req_id,
                "error": {"code": -32603, "message": str(e)},
            })
    else:
        return json.dumps({
            "jsonrpc": "2.0", "id": req_id,
            "error": {"code": -32601, "message": f"Method not found: {method}"},
        })


# ─── Main loop ───────────────────────────────────────────────────────────────

def main():
    # Warm up — load model eagerly so startup time is known upfront
    try:
        get_pipeline()
        log.info(f"Ready: model={MODEL_ID} dim={get_dimension()}")
    except Exception as e:
        log.error(f"Failed to load model: {e}")
        # Still ready to report error on first request
        print(json.dumps({
            "jsonrpc": "2.0", "id": -1,
            "error": {"code": -32603, "message": f"Model load failed: {e}"},
        }), flush=True)

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        resp = handle_request(line)
        if resp is not None:
            print(resp, flush=True)


if __name__ == "__main__":
    main()
