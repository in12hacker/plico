#!/usr/bin/env python3
"""
HNSW Parameter Sweep at Scale — synthetic 10K/50K/100K vector haystacks.

LongMemEval-S haystacks are too small to differentiate HNSW configs.
This test inserts N random vectors + known ground-truth vectors, then measures
recall@k and latency at scale to find the optimal parameter balance.
"""

import json
import math
import os
import sys
import time
from pathlib import Path

import numpy as np

RESULTS_DIR = Path(__file__).resolve().parent.parent / "results"
DIM = 384

CONFIGS = [
    # (name, M, ef_construction, ef_search)
    ("M16_efc128_efs64",   16, 128,   64),   # current Plico defaults
    ("M16_efc128_efs128",  16, 128,  128),
    ("M16_efc128_efs256",  16, 128,  256),
    ("M16_efc200_efs128",  16, 200,  128),
    ("M24_efc200_efs128",  24, 200,  128),
    ("M24_efc200_efs200",  24, 200,  200),
    ("M32_efc256_efs128",  32, 256,  128),
    ("M32_efc256_efs256",  32, 256,  256),
    ("M48_efc400_efs256",  48, 400,  256),
]

SCALES = [10_000, 50_000, 100_000]
N_QUERIES = 200
N_GROUND_TRUTH_PER_QUERY = 5


def generate_data(n_vectors, n_queries, n_gt_per_query, dim, rng):
    """Generate random haystack with planted ground-truth neighbors."""
    haystack = rng.standard_normal((n_vectors, dim)).astype(np.float32)
    # Normalize
    norms = np.linalg.norm(haystack, axis=1, keepdims=True)
    norms[norms == 0] = 1.0
    haystack = haystack / norms

    queries = rng.standard_normal((n_queries, dim)).astype(np.float32)
    queries = queries / np.linalg.norm(queries, axis=1, keepdims=True)

    # Plant ground truth: for each query, replace n_gt_per_query random positions
    # with vectors very similar to the query (add small noise)
    ground_truth = []
    for qi in range(n_queries):
        gt_indices = rng.choice(n_vectors, n_gt_per_query, replace=False)
        for gi in gt_indices:
            noise = rng.standard_normal(dim).astype(np.float32) * 0.05
            haystack[gi] = queries[qi] + noise
            haystack[gi] = haystack[gi] / np.linalg.norm(haystack[gi])
        ground_truth.append(gt_indices.tolist())

    return haystack, queries, ground_truth


def brute_force_topk(haystack, query, k):
    """Exact nearest neighbors via brute force cosine similarity."""
    sims = haystack @ query
    top_k = np.argsort(sims)[-k:][::-1]
    return top_k.tolist()


def run_config_at_scale(config_name, M, ef_c, ef_s, haystack, queries, ground_truth, k_values):
    """Build index, search, measure recall and latency."""
    from usearch.index import Index

    n = len(haystack)

    t0 = time.perf_counter()
    idx = Index(
        ndim=DIM,
        metric="cos",
        dtype="f16",
        connectivity=M,
        expansion_add=ef_c,
        expansion_search=ef_s,
    )
    idx.add(np.arange(n), haystack)
    build_s = time.perf_counter() - t0

    # Search all queries
    search_times = []
    recalls = {k: [] for k in k_values}

    for qi in range(len(queries)):
        t1 = time.perf_counter()
        matches = idx.search(queries[qi], max(k_values))
        search_ms = (time.perf_counter() - t1) * 1000
        search_times.append(search_ms)

        retrieved = [int(k) for k in matches.keys]
        gt_set = set(ground_truth[qi])

        for k in k_values:
            top_k = set(retrieved[:k])
            hit = len(top_k & gt_set)
            recalls[k].append(hit / min(len(gt_set), k))

    return {
        "config": config_name,
        "M": M,
        "ef_construction": ef_c,
        "ef_search": ef_s,
        "n_vectors": n,
        "build_s": round(build_s, 3),
        **{f"recall@{k}": round(np.mean(recalls[k]) * 100, 2) for k in k_values},
        "p50_search_ms": round(np.percentile(search_times, 50), 3),
        "p99_search_ms": round(np.percentile(search_times, 99), 3),
        "avg_search_ms": round(np.mean(search_times), 3),
    }


def main():
    rng = np.random.default_rng(42)
    k_values = [1, 5, 10, 20]
    all_results = []

    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    for n_vec in SCALES:
        print(f"\n{'='*80}")
        print(f"Scale: {n_vec:,} vectors")
        print(f"{'='*80}")

        haystack, queries, ground_truth = generate_data(n_vec, N_QUERIES, N_GROUND_TRUTH_PER_QUERY, DIM, rng)

        print(f"  Generated {n_vec:,} vectors, {N_QUERIES} queries, {N_GROUND_TRUTH_PER_QUERY} GT per query")
        mem_mb = haystack.nbytes / 1024 / 1024
        print(f"  Haystack memory: {mem_mb:.1f} MB")

        scale_results = []
        for cfg_name, M, ef_c, ef_s in CONFIGS:
            print(f"  {cfg_name:<26}", end="", flush=True)
            result = run_config_at_scale(cfg_name, M, ef_c, ef_s, haystack, queries, ground_truth, k_values)
            scale_results.append(result)
            all_results.append(result)
            print(f"  R@5={result['recall@5']:6.2f}%  R@10={result['recall@10']:6.2f}%  "
                  f"p50={result['p50_search_ms']:.3f}ms  p99={result['p99_search_ms']:.3f}ms  "
                  f"build={result['build_s']:.1f}s")

        # Print summary table for this scale
        print(f"\n  {'Config':<26} {'R@1':>7} {'R@5':>7} {'R@10':>7} {'R@20':>7} {'p50':>8} {'p99':>8} {'build':>7}")
        print(f"  {'-'*88}")
        for r in sorted(scale_results, key=lambda x: (-x["recall@10"], x["p50_search_ms"])):
            print(f"  {r['config']:<26} {r['recall@1']:>6.2f}% {r['recall@5']:>6.2f}% "
                  f"{r['recall@10']:>6.2f}% {r['recall@20']:>6.2f}% "
                  f"{r['p50_search_ms']:>7.3f}ms {r['p99_search_ms']:>7.3f}ms {r['build_s']:>6.1f}s")

    out_file = RESULTS_DIR / "hnsw_scale_sweep.json"
    with open(out_file, "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\n\nAll results saved to: {out_file}")

    # Final recommendation
    print("\n" + "=" * 80)
    print("RECOMMENDATION")
    print("=" * 80)
    # Find best balance at 100K scale
    big = [r for r in all_results if r["n_vectors"] == max(SCALES)]
    if big:
        # Score = recall@10 * 0.6 + recall@5 * 0.3 + (1 - normalized_latency) * 0.1
        max_lat = max(r["p50_search_ms"] for r in big)
        for r in big:
            r["_score"] = r["recall@10"] * 0.6 + r["recall@5"] * 0.3 + (1 - r["p50_search_ms"] / max_lat) * 10
        best = max(big, key=lambda r: r["_score"])
        print(f"Best balance at {max(SCALES):,} vectors:")
        print(f"  Config: {best['config']}")
        print(f"  M={best['M']}, ef_construction={best['ef_construction']}, ef_search={best['ef_search']}")
        print(f"  R@5={best['recall@5']}%, R@10={best['recall@10']}%, R@20={best['recall@20']}%")
        print(f"  p50={best['p50_search_ms']}ms, p99={best['p99_search_ms']}ms")
        print(f"  Build time: {best['build_s']}s")


if __name__ == "__main__":
    main()
