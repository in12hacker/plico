#!/usr/bin/env python3
"""Plico local Hugging Face embedding worker (JSON-RPC over stdio)."""

import json
import logging
import os
import sys
import warnings

warnings.filterwarnings("ignore")
logging.basicConfig(
    level=getattr(logging, os.environ.get("LOG_LEVEL", "WARNING")),
    format="%(asctime)s plico-local-embedding %(levelname)s %(message)s",
)
log = logging.getLogger("plico-local-embedding")

MODEL_ID = os.environ.get("EMBEDDING_MODEL_ID", "BAAI/bge-small-en-v1.5")
MAX_LENGTH = 512
_pipeline = None
_dimension = None


def _mean_pool(last_hidden_state, attention_mask):
    import numpy as np

    mask_expanded = np.broadcast_to(
        np.expand_dims(attention_mask, axis=-1),
        last_hidden_state.shape,
    ).astype(float)
    sum_emb = np.sum(last_hidden_state * mask_expanded, axis=1)
    sum_mask = np.clip(mask_expanded.sum(axis=1), a_min=1e-9, a_max=None)
    return sum_emb / sum_mask


def get_pipeline():
    global _pipeline, _dimension
    if _pipeline is not None:
        return _pipeline

    try:
        import torch
        from transformers import AutoTokenizer
        from optimum.onnxruntime import ORTModelForFeatureExtraction

        tokenizer = AutoTokenizer.from_pretrained(MODEL_ID)
        model = ORTModelForFeatureExtraction.from_pretrained(
            MODEL_ID,
            export=False,
            provider="CPUExecutionProvider",
        )
        _dimension = int(model.config.hidden_size)

        def pipeline(texts):
            inputs = tokenizer(texts, return_tensors="pt", padding=True, truncation=True, max_length=MAX_LENGTH)
            with torch.no_grad():
                outputs = model(**inputs)
            return _mean_pool(outputs.last_hidden_state.numpy(), inputs["attention_mask"].numpy())

        _pipeline = pipeline
        return _pipeline
    except Exception:
        log.warning("local optimum embedding backend unavailable; trying transformers cpu")

    try:
        import torch
        from transformers import AutoModel, AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(MODEL_ID)
        model = AutoModel.from_pretrained(MODEL_ID)
        model.eval()
        _dimension = int(model.config.hidden_size)

        def pipeline(texts):
            inputs = tokenizer(texts, return_tensors="pt", padding=True, truncation=True, max_length=MAX_LENGTH)
            with torch.no_grad():
                outputs = model(**inputs)
            last_hidden = outputs.last_hidden_state
            mask = inputs["attention_mask"].unsqueeze(-1).expand(last_hidden.size()).float()
            summed = (last_hidden * mask).sum(dim=1)
            counts = mask.sum(dim=1).clamp(min=1e-9)
            return (summed / counts).numpy()

        _pipeline = pipeline
        return _pipeline
    except Exception as error:
        raise RuntimeError("local embedding model unavailable") from error


def handle_embed(params):
    text = params.get("text", "")
    if not isinstance(text, str) or not text:
        raise ValueError("text is required")
    embedding = get_pipeline()([text])[0].tolist()
    return {"embedding": embedding}


def handle_info(_params):
    get_pipeline()
    return {
        "schema": "plico.embedding.local-worker-info/v1",
        "model_id": MODEL_ID,
        "raw_dimension": _dimension,
    }


def handle_request(line):
    try:
        request = json.loads(line.strip())
    except json.JSONDecodeError:
        return json.dumps({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse error"}})
    request_id = request.get("id")
    try:
        if request.get("method") == "embed":
            result = handle_embed(request.get("params", {}))
        elif request.get("method") == "info":
            result = handle_info(request.get("params", {}))
        else:
            return json.dumps({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "method not found"},
            })
        return json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result})
    except Exception:
        log.error("local embedding worker request failed")
        return json.dumps({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32603, "message": "local embedding worker failed"},
        })


def main():
    try:
        get_pipeline()
    except Exception:
        log.error("local embedding worker initialization failed")
    for line in sys.stdin:
        if not line.strip():
            continue
        response = handle_request(line)
        print(response, flush=True)


if __name__ == "__main__":
    main()
