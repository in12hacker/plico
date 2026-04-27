#!/usr/bin/env python3
"""
HNSW Parameter Sweep — find optimal (M, ef_construction, ef_search) for recall vs latency.

Tests multiple parameter combinations against LongMemEval-S retrieval.
Uses a random sample of 100 questions for speed, then validates the winner on all 500.
"""

import json
import math
import os
import sys
import time
from pathlib import Path

import numpy as np

BENCH_ROOT = Path(__file__).resolve().parent.parent
DATA_DIR = BENCH_ROOT / "data"
RESULTS_DIR = BENCH_ROOT / "results"
DATASET_FILE = DATA_DIR / "longmemeval_s_cleaned.json"

EMBED_MODEL = "all-MiniLM-L6-v2"
EMBED_DIM = 384

# Parameter grid to test
CONFIGS = [
    # (name, M, ef_construction, ef_search)
    ("baseline",      16, 128,  64),   # current Plico defaults
    ("ef_search_128", 16, 128, 128),
    ("ef_search_256", 16, 128, 256),
    ("ef_search_512", 16, 128, 512),
    ("M32_ef128",     32, 256, 128),
    ("M32_ef256",     32, 256, 256),
    ("M32_ef512",     32, 256, 512),
    ("M24_ef200",     24, 200, 200),
    ("M48_ef256",     48, 400, 256),
]


def flatten_session(session: list[dict]) -> str:
    parts = []
    for turn in session:
        role = turn.get("role", "unknown")
        content = turn.get("content", "")
        parts.append(f"{role}: {content}")
    return "\n".join(parts)


def compute_recall_at_k(gold_ids: list, retrieved_ids: list, k: int) -> float:
    top_k = set(retrieved_ids[:k])
    return 1.0 if any(g in top_k for g in gold_ids) else 0.0


def compute_ndcg_at_k(gold_ids: list, retrieved_ids: list, k: int) -> float:
    gold_set = set(gold_ids)
    dcg = sum(1.0 / math.log2(i + 2) for i, rid in enumerate(retrieved_ids[:k]) if rid in gold_set)
    idcg = sum(1.0 / math.log2(i + 2) for i in range(min(len(gold_ids), k)))
    return dcg / idcg if idcg > 0 else 0.0


def compute_mrr(gold_ids: list, retrieved_ids: list) -> float:
    gold_set = set(gold_ids)
    for i, rid in enumerate(retrieved_ids):
        if rid in gold_set:
            return 1.0 / (i + 1)
    return 0.0


def run_config(config_name, M, ef_c, ef_s, questions, model, precomputed_embeddings):
    """Run a single HNSW config against the question set."""
    from usearch.index import Index

    results = []
    total_search_ms = 0.0
    total_build_ms = 0.0

    for item in questions:
        qid = item["question_id"]
        question = item["question"]
        gold_session_ids = item.get("answer_session_ids", [])
        haystack_ids = item.get("haystack_session_ids", [])
        sessions = item.get("haystack_sessions", [])

        if not sessions or not gold_session_ids or not haystack_ids:
            continue

        cache_key = qid
        if cache_key not in precomputed_embeddings:
            session_texts = [flatten_session(s) for s in sessions]
            session_embs = model.encode(session_texts, batch_size=64, show_progress_bar=False)
            query_emb = model.encode(question)
            precomputed_embeddings[cache_key] = (session_embs, query_emb)

        session_embs, query_emb = precomputed_embeddings[cache_key]

        # Build index with specific params
        t0 = time.perf_counter()
        idx = Index(
            ndim=EMBED_DIM,
            metric="cos",
            dtype="f16",
            connectivity=M,
            expansion_add=ef_c,
            expansion_search=ef_s,
        )
        for sid, emb in enumerate(session_embs):
            idx.add(sid, emb.astype(np.float32))
        build_ms = (time.perf_counter() - t0) * 1000
        total_build_ms += build_ms

        # Search
        t1 = time.perf_counter()
        matches = idx.search(query_emb.astype(np.float32), 20)
        search_ms = (time.perf_counter() - t1) * 1000
        total_search_ms += search_ms

        retrieved_ids = [haystack_ids[int(k)] for k in matches.keys if int(k) < len(haystack_ids)]

        r5 = compute_recall_at_k(gold_session_ids, retrieved_ids, 5)
        r10 = compute_recall_at_k(gold_session_ids, retrieved_ids, 10)
        ndcg10 = compute_ndcg_at_k(gold_session_ids, retrieved_ids, 10)
        mrr = compute_mrr(gold_session_ids, retrieved_ids)

        results.append({"r5": r5, "r10": r10, "ndcg10": ndcg10, "mrr": mrr})

    n = len(results)
    if n == 0:
        return None

    return {
        "config": config_name,
        "M": M,
        "ef_construction": ef_c,
        "ef_search": ef_s,
        "n_questions": n,
        "recall@5": round(np.mean([r["r5"] for r in results]) * 100, 1),
        "recall@10": round(np.mean([r["r10"] for r in results]) * 100, 1),
        "ndcg@10": round(np.mean([r["ndcg10"] for r in results]) * 100, 1),
        "mrr": round(np.mean([r["mrr"] for r in results]) * 100, 1),
        "avg_build_ms": round(total_build_ms / n, 2),
        "avg_search_ms": round(total_search_ms / n, 3),
    }


def main():
    from sentence_transformers import SentenceTransformer

    if not DATASET_FILE.exists():
        print(f"ERROR: {DATASET_FILE} not found")
        sys.exit(1)

    print(f"Loading dataset ...")
    with open(DATASET_FILE) as f:
        dataset = json.load(f)

    sample_size = int(os.environ.get("SWEEP_SAMPLES", "100"))
    if sample_size < len(dataset):
        rng = np.random.default_rng(42)
        indices = rng.choice(len(dataset), sample_size, replace=False)
        questions = [dataset[i] for i in indices]
    else:
        questions = dataset
    print(f"  {len(questions)} questions (sample={sample_size})")

    print(f"Loading embedding model: {EMBED_MODEL} ...")
    model = SentenceTransformer(EMBED_MODEL)

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    precomputed = {}
    all_results = []

    for cfg_name, M, ef_c, ef_s in CONFIGS:
        print(f"\n  Testing {cfg_name}: M={M}, ef_c={ef_c}, ef_s={ef_s} ...")
        result = run_config(cfg_name, M, ef_c, ef_s, questions, model, precomputed)
        if result:
            all_results.append(result)
            print(f"    R@5={result['recall@5']}%  R@10={result['recall@10']}%  "
                  f"NDCG@10={result['ndcg@10']}%  MRR={result['mrr']}%  "
                  f"build={result['avg_build_ms']}ms  search={result['avg_search_ms']}ms")

    # Sort by recall@5 descending
    all_results.sort(key=lambda r: (-r["recall@5"], r["avg_search_ms"]))

    print("\n" + "=" * 90)
    print("HNSW Parameter Sweep Results")
    print("=" * 90)
    print(f"{'Config':<18} {'M':>3} {'ef_c':>5} {'ef_s':>5} {'R@5':>6} {'R@10':>6} {'NDCG':>6} {'MRR':>6} {'build':>8} {'search':>8}")
    print("-" * 90)
    for r in all_results:
        print(f"{r['config']:<18} {r['M']:>3} {r['ef_construction']:>5} {r['ef_search']:>5} "
              f"{r['recall@5']:>5.1f}% {r['recall@10']:>5.1f}% {r['ndcg@10']:>5.1f}% {r['mrr']:>5.1f}% "
              f"{r['avg_build_ms']:>7.1f}ms {r['avg_search_ms']:>7.3f}ms")

    out_file = RESULTS_DIR / "hnsw_param_sweep.json"
    with open(out_file, "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\nResults saved to: {out_file}")

    # Recommend best config
    best = all_results[0]
    print(f"\nRecommended: {best['config']} (R@5={best['recall@5']}%, search={best['avg_search_ms']}ms)")


if __name__ == "__main__":
    main()
