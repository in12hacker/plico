"""Standard metrics for benchmark evaluation."""

from __future__ import annotations

import math
import re
import statistics
from typing import Any

import numpy as np


def token_level_f1(pred: str, ref: str) -> float:
    """Compute token-level F1 after normalization."""
    pred_tokens = set(_normalize(pred).split())
    ref_tokens = set(_normalize(ref).split())
    if not pred_tokens and not ref_tokens:
        return 1.0
    if not pred_tokens or not ref_tokens:
        return 0.0
    common = pred_tokens & ref_tokens
    prec = len(common) / len(pred_tokens)
    rec = len(common) / len(ref_tokens)
    if prec + rec == 0:
        return 0.0
    return 2 * prec * rec / (prec + rec)


def _normalize(text: str) -> str:
    text = text.lower().strip()
    text = re.sub(r"[^\w\s]", " ", text)
    text = re.sub(r"\s+", " ", text)
    return text


def exact_match(pred: str, ref: str) -> bool:
    return _normalize(pred) == _normalize(ref)


def bleu1(pred: str, ref: str) -> float:
    """Simplified BLEU-1 (unigram precision with brevity penalty)."""
    pred_tokens = _normalize(pred).split()
    ref_tokens = _normalize(ref).split()
    if not pred_tokens or not ref_tokens:
        return 0.0
    pred_counts: dict[str, int] = {}
    for t in pred_tokens:
        pred_counts[t] = pred_counts.get(t, 0) + 1
    ref_counts: dict[str, int] = {}
    for t in ref_tokens:
        ref_counts[t] = ref_counts.get(t, 0) + 1
    clipped = 0
    for t, c in pred_counts.items():
        clipped += min(c, ref_counts.get(t, 0))
    precision = clipped / len(pred_tokens)
    bp = math.exp(1 - len(ref_tokens) / len(pred_tokens)) if len(pred_tokens) < len(ref_tokens) else 1.0
    return precision * bp


def recall_at_k(relevants: set[Any], retrieved: list[Any], k: int) -> float:
    if not relevants:
        return 0.0
    retrieved_k = set(retrieved[:k])
    return len(relevants & retrieved_k) / len(relevants)


def ndcg_at_k(relevances: dict[Any, float], retrieved: list[Any], k: int) -> float:
    """Compute NDCG@k given relevance scores for items."""
    dcg = 0.0
    for i, item in enumerate(retrieved[:k], start=1):
        rel = relevances.get(item, 0.0)
        dcg += rel / math.log2(i + 1)
    # ideal DCG
    ideal_rels = sorted(relevances.values(), reverse=True)[:k]
    idcg = sum(rel / math.log2(i + 2) for i, rel in enumerate(ideal_rels))
    return dcg / idcg if idcg > 0 else 0.0


def mrr(relevants: set[Any], retrieved: list[Any]) -> float:
    for i, item in enumerate(retrieved, start=1):
        if item in relevants:
            return 1.0 / i
    return 0.0


def compute_statistics(values: list[float]) -> dict[str, float]:
    """Compute mean, std, and 95% CI."""
    if not values:
        return {"mean": 0.0, "std": 0.0, "ci95_low": 0.0, "ci95_high": 0.0}
    arr = np.array(values)
    mean = float(np.mean(arr))
    std = float(np.std(arr, ddof=1)) if len(arr) > 1 else 0.0
    sem = std / math.sqrt(len(arr))
    # approximate 95% CI using t-distribution (large n ~ 1.96)
    z = 1.96 if len(arr) >= 30 else 2.228  # t-value for df=10 approx
    return {
        "mean": mean,
        "std": std,
        "ci95_low": mean - z * sem,
        "ci95_high": mean + z * sem,
    }


def latency_percentiles(latencies_ms: list[float]) -> dict[str, float]:
    if not latencies_ms:
        return {"p50": 0.0, "p95": 0.0, "p99": 0.0}
    arr = np.array(latencies_ms)
    return {
        "p50": float(np.percentile(arr, 50)),
        "p95": float(np.percentile(arr, 95)),
        "p99": float(np.percentile(arr, 99)),
    }


def aggregate_category(
    results: list[dict[str, Any]], category_key: str
) -> dict[str, dict[str, Any]]:
    """Aggregate results by category key."""
    from collections import defaultdict

    by_cat: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for r in results:
        cat = r.get(category_key, "unknown")
        by_cat[cat].append(r)
    summary: dict[str, dict[str, Any]] = {}
    for cat, items in by_cat.items():
        summary[cat] = {
            "count": len(items),
            "f1": statistics.mean([r["f1"] for r in items]) if any("f1" in r for r in items) else None,
            "em": statistics.mean([r["em"] for r in items]) if any("em" in r for r in items) else None,
            "llm_score": statistics.mean([r["llm_score"] for r in items]) if any("llm_score" in r for r in items) else None,
        }
    return summary
