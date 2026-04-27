#!/usr/bin/env python3
"""
Dimension 1: LongMemEval-S Retrieval Benchmark

Measures Plico-equivalent retrieval quality (usearch HNSW + all-MiniLM-L6-v2)
against the LongMemEval-S dataset (500 questions, ~40 sessions each).

Metrics: recall@5, recall@10, recall@20, NDCG@10, MRR

Comparison baselines:
  agentmemory BM25+Vector: 95.2% R@5, 98.6% R@10
  MemPalace raw vector:    96.6% R@5
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
RESULTS_FILE = RESULTS_DIR / "longmemeval_retrieval.json"

EMBED_MODEL = "all-MiniLM-L6-v2"
EMBED_DIM = 384


def load_model():
    from sentence_transformers import SentenceTransformer
    return SentenceTransformer(EMBED_MODEL)


def flatten_session(session: list[dict]) -> str:
    """Convert a list of turns into a single text string."""
    parts = []
    for turn in session:
        role = turn.get("role", "unknown")
        content = turn.get("content", "")
        parts.append(f"{role}: {content}")
    return "\n".join(parts)


def compute_recall_at_k(gold_ids: list, retrieved_ids: list, k: int) -> float:
    """Does ANY gold session appear in top-K retrieved results?"""
    top_k = set(retrieved_ids[:k])
    return 1.0 if any(g in top_k for g in gold_ids) else 0.0


def compute_ndcg_at_k(gold_ids: list, retrieved_ids: list, k: int) -> float:
    """NDCG@K where gold sessions have relevance 1, others 0."""
    gold_set = set(gold_ids)
    dcg = 0.0
    for i, rid in enumerate(retrieved_ids[:k]):
        if rid in gold_set:
            dcg += 1.0 / math.log2(i + 2)
    n_rel = min(len(gold_ids), k)
    idcg = sum(1.0 / math.log2(i + 2) for i in range(n_rel))
    return dcg / idcg if idcg > 0 else 0.0


def compute_mrr(gold_ids: list, retrieved_ids: list) -> float:
    """Mean Reciprocal Rank — 1/(rank of first gold hit)."""
    gold_set = set(gold_ids)
    for i, rid in enumerate(retrieved_ids):
        if rid in gold_set:
            return 1.0 / (i + 1)
    return 0.0


def run_benchmark():
    from usearch.index import Index

    if not DATASET_FILE.exists():
        print(f"ERROR: dataset not found at {DATASET_FILE}")
        print("Run: python bench/longmemeval/download.py")
        sys.exit(1)

    print(f"Loading dataset from {DATASET_FILE} ...")
    with open(DATASET_FILE) as f:
        dataset = json.load(f)
    print(f"  {len(dataset)} questions loaded")

    print(f"Loading embedding model: {EMBED_MODEL} ...")
    model = load_model()

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    all_results = []
    type_metrics: dict[str, list[dict]] = {}

    total_embed_time = 0.0
    total_search_time = 0.0
    total_sessions = 0

    for qi, item in enumerate(dataset):
        qid = item["question_id"]
        qtype = item.get("question_type", "unknown")
        question = item["question"]
        gold_session_ids = item.get("answer_session_ids", [])
        sessions = item.get("haystack_sessions", [])

        haystack_ids = item.get("haystack_session_ids", [])

        if not sessions or not gold_session_ids or not haystack_ids:
            continue

        # Build session texts; haystack_ids[i] is the string ID for sessions[i]
        session_texts = [flatten_session(s) for s in sessions]
        total_sessions += len(sessions)

        # Embed sessions
        t0 = time.perf_counter()
        session_embeddings = model.encode(session_texts, batch_size=64, show_progress_bar=False)
        query_embedding = model.encode(question)
        embed_time = time.perf_counter() - t0
        total_embed_time += embed_time

        # Build usearch index (matching Plico: cos metric, f16 quantization)
        idx = Index(ndim=EMBED_DIM, metric="cos", dtype="f16")
        for sid, emb in enumerate(session_embeddings):
            idx.add(sid, emb.astype(np.float32))

        # Search
        t1 = time.perf_counter()
        k_max = 20
        matches = idx.search(query_embedding.astype(np.float32), k_max)
        search_time = time.perf_counter() - t1
        total_search_time += search_time

        # Map integer keys back to string session IDs
        retrieved_ids = [haystack_ids[int(k)] for k in matches.keys if int(k) < len(haystack_ids)]

        # Compute metrics
        r5 = compute_recall_at_k(gold_session_ids, retrieved_ids, 5)
        r10 = compute_recall_at_k(gold_session_ids, retrieved_ids, 10)
        r20 = compute_recall_at_k(gold_session_ids, retrieved_ids, 20)
        ndcg10 = compute_ndcg_at_k(gold_session_ids, retrieved_ids, 10)
        mrr = compute_mrr(gold_session_ids, retrieved_ids)

        result = {
            "question_id": qid,
            "question_type": qtype,
            "recall@5": r5,
            "recall@10": r10,
            "recall@20": r20,
            "ndcg@10": ndcg10,
            "mrr": mrr,
            "n_sessions": len(sessions),
            "n_gold": len(gold_session_ids),
            "embed_time_s": round(embed_time, 3),
            "search_time_ms": round(search_time * 1000, 2),
        }
        all_results.append(result)

        if qtype not in type_metrics:
            type_metrics[qtype] = []
        type_metrics[qtype].append(result)

        if (qi + 1) % 50 == 0:
            avg_r5 = np.mean([r["recall@5"] for r in all_results])
            print(f"  [{qi+1}/{len(dataset)}] running R@5={avg_r5:.3f}")

    # Aggregate
    n = len(all_results)
    overall = {
        "n_questions": n,
        "total_sessions_indexed": total_sessions,
        "recall@5": round(np.mean([r["recall@5"] for r in all_results]) * 100, 1),
        "recall@10": round(np.mean([r["recall@10"] for r in all_results]) * 100, 1),
        "recall@20": round(np.mean([r["recall@20"] for r in all_results]) * 100, 1),
        "ndcg@10": round(np.mean([r["ndcg@10"] for r in all_results]) * 100, 1),
        "mrr": round(np.mean([r["mrr"] for r in all_results]) * 100, 1),
        "total_embed_time_s": round(total_embed_time, 1),
        "total_search_time_ms": round(total_search_time * 1000, 1),
        "avg_search_time_ms": round(total_search_time * 1000 / max(n, 1), 2),
        "embedding_model": EMBED_MODEL,
        "index_config": "usearch cos f16 (Plico-equivalent)",
    }

    by_type = {}
    for qtype, results in sorted(type_metrics.items()):
        by_type[qtype] = {
            "count": len(results),
            "recall@5": round(np.mean([r["recall@5"] for r in results]) * 100, 1),
            "recall@10": round(np.mean([r["recall@10"] for r in results]) * 100, 1),
            "ndcg@10": round(np.mean([r["ndcg@10"] for r in results]) * 100, 1),
            "mrr": round(np.mean([r["mrr"] for r in results]) * 100, 1),
        }

    report = {
        "benchmark": "LongMemEval-S Retrieval",
        "system": "Plico (usearch cos f16 + all-MiniLM-L6-v2)",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "overall": overall,
        "by_question_type": by_type,
        "per_question": all_results,
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)

    # Print summary
    print("\n" + "=" * 70)
    print("LongMemEval-S Retrieval Benchmark Results")
    print("=" * 70)
    print(f"System: Plico (usearch cos f16 + {EMBED_MODEL})")
    print(f"Questions: {n}")
    print()
    print(f"  {'Metric':<12} {'Plico':>8} {'agentmemory':>13} {'MemPalace':>10}")
    print(f"  {'─'*12} {'─'*8} {'─'*13} {'─'*10}")
    print(f"  {'R@5':<12} {overall['recall@5']:>7.1f}% {95.2:>12.1f}% {96.6:>9.1f}%")
    print(f"  {'R@10':<12} {overall['recall@10']:>7.1f}% {98.6:>12.1f}%")
    print(f"  {'R@20':<12} {overall['recall@20']:>7.1f}% {99.4:>12.1f}%")
    print(f"  {'NDCG@10':<12} {overall['ndcg@10']:>7.1f}% {87.9:>12.1f}%")
    print(f"  {'MRR':<12} {overall['mrr']:>7.1f}% {88.2:>12.1f}%")
    print()
    print("By question type:")
    for qtype, m in by_type.items():
        print(f"  {qtype:<30} R@5={m['recall@5']:5.1f}%  R@10={m['recall@10']:5.1f}%  n={m['count']}")
    print()
    print(f"Timing: embed={overall['total_embed_time_s']}s, avg_search={overall['avg_search_time_ms']}ms")
    print(f"Results saved to: {RESULTS_FILE}")
    return report


if __name__ == "__main__":
    run_benchmark()
