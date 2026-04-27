#!/usr/bin/env python3
"""Compare embedding models: qwen2.5-coder-7b (3584d) vs jina-v5-small (1024d).

Metrics:
  1. Latency (single & batch)
  2. Semantic similarity quality (cosine similarity on known-similar/dissimilar pairs)
  3. Memory footprint per vector at HNSW scale
  4. Matryoshka dimension sweep for jina-v5 (1024 → 512 → 256 → 128)
"""

import json, time, sys, os
import urllib.request
import numpy as np
from dataclasses import dataclass

QWEN_URL = "http://localhost:18920/v1/embeddings"
JINA_URL = "http://localhost:18921/v1/embeddings"

SIMILAR_PAIRS = [
    ("The cat sat on the mat", "A kitten was sitting on a rug"),
    ("Machine learning is a subset of AI", "ML belongs to artificial intelligence"),
    ("Python is a popular programming language", "Python is widely used in software development"),
    ("The weather is sunny today", "It is a bright and clear day"),
    ("I love eating Italian food", "Italian cuisine is my favorite"),
]

DISSIMILAR_PAIRS = [
    ("The cat sat on the mat", "Quantum computing uses qubits"),
    ("Machine learning is a subset of AI", "The recipe calls for two eggs"),
    ("Python is a popular programming language", "The stock market crashed yesterday"),
    ("The weather is sunny today", "Database normalization reduces redundancy"),
    ("I love eating Italian food", "The Pythagorean theorem relates triangle sides"),
]

RETRIEVAL_CORPUS = [
    "Plico is an AI-native operating system kernel for AI agents",
    "Content-Addressed Storage uses SHA-256 hashes as file addresses",
    "HNSW is an approximate nearest neighbor algorithm for vector search",
    "BM25 is a ranking function used for keyword-based information retrieval",
    "Knowledge graphs represent relationships between entities as edges",
    "Embedding models convert text into dense vector representations",
    "Semantic search finds documents by meaning rather than exact keywords",
    "Rust provides memory safety without garbage collection",
    "TCP_NODELAY disables Nagle's algorithm to reduce latency",
    "The transformer architecture uses self-attention mechanisms",
    "Vector databases store and index high-dimensional embeddings",
    "Reciprocal Rank Fusion combines results from multiple retrieval systems",
    "Large language models generate text based on learned patterns",
    "Git uses content-addressed storage with SHA-1 hashes",
    "Docker containers provide isolated runtime environments",
]

RETRIEVAL_QUERIES = [
    ("How does Plico store files?", [0, 1]),
    ("What algorithm is used for vector search?", [2, 10]),
    ("How to combine keyword and vector search?", [3, 11]),
    ("What is an embedding?", [5, 6]),
    ("Tell me about knowledge representation", [4, 6]),
]


def embed(url: str, texts: list[str]) -> list[list[float]]:
    payload = json.dumps({"input": texts, "model": "test"}).encode()
    req = urllib.request.Request(url, data=payload, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req) as resp:
        data = json.loads(resp.read())
    sorted_data = sorted(data["data"], key=lambda x: x["index"])
    return [d["embedding"] for d in sorted_data]


def cosine_sim(a: list[float], b: list[float]) -> float:
    a, b = np.array(a), np.array(b)
    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-10))


def truncate_embedding(emb: list[float], dim: int) -> list[float]:
    v = np.array(emb[:dim])
    return (v / (np.linalg.norm(v) + 1e-10)).tolist()


@dataclass
class ModelResult:
    name: str
    dim: int
    latency_single_ms: float
    latency_batch_ms: float
    avg_similar_cos: float
    avg_dissimilar_cos: float
    separation: float
    retrieval_recall_at_3: float
    mem_per_10k_mb: float


def benchmark_model(name: str, url: str, dim_override: int | None = None) -> ModelResult:
    effective_name = name if dim_override is None else f"{name}@{dim_override}d"
    print(f"\n{'='*60}")
    print(f"  Benchmarking: {effective_name}")
    print(f"{'='*60}")

    # 1. Latency: single
    times_single = []
    for text in ["Hello world", "The quick brown fox", "AI kernel"]:
        t0 = time.perf_counter()
        embed(url, [text])
        times_single.append((time.perf_counter() - t0) * 1000)
    lat_single = np.median(times_single)

    # 2. Latency: batch (all corpus)
    t0 = time.perf_counter()
    embed(url, RETRIEVAL_CORPUS)
    lat_batch = (time.perf_counter() - t0) * 1000

    # 3. Similar/dissimilar cosine similarity
    sim_scores = []
    for a_text, b_text in SIMILAR_PAIRS:
        embs = embed(url, [a_text, b_text])
        ea, eb = embs[0], embs[1]
        if dim_override:
            ea = truncate_embedding(ea, dim_override)
            eb = truncate_embedding(eb, dim_override)
        sim_scores.append(cosine_sim(ea, eb))

    dissim_scores = []
    for a_text, b_text in DISSIMILAR_PAIRS:
        embs = embed(url, [a_text, b_text])
        ea, eb = embs[0], embs[1]
        if dim_override:
            ea = truncate_embedding(ea, dim_override)
            eb = truncate_embedding(eb, dim_override)
        dissim_scores.append(cosine_sim(ea, eb))

    avg_sim = np.mean(sim_scores)
    avg_dissim = np.mean(dissim_scores)
    separation = avg_sim - avg_dissim

    # 4. Retrieval recall@3
    corpus_embs = embed(url, RETRIEVAL_CORPUS)
    if dim_override:
        corpus_embs = [truncate_embedding(e, dim_override) for e in corpus_embs]

    hits = 0
    total = 0
    for query, relevant_ids in RETRIEVAL_QUERIES:
        q_emb = embed(url, [query])[0]
        if dim_override:
            q_emb = truncate_embedding(q_emb, dim_override)
        scores = [(i, cosine_sim(q_emb, ce)) for i, ce in enumerate(corpus_embs)]
        scores.sort(key=lambda x: x[1], reverse=True)
        top3 = {s[0] for s in scores[:3]}
        for rid in relevant_ids:
            total += 1
            if rid in top3:
                hits += 1
    recall_at_3 = hits / total if total > 0 else 0

    actual_dim = dim_override if dim_override else len(embed(url, ["test"])[0])

    # f16 storage: 2 bytes per dimension
    mem_per_10k = actual_dim * 2 * 10000 / (1024 * 1024)

    result = ModelResult(
        name=effective_name,
        dim=actual_dim,
        latency_single_ms=lat_single,
        latency_batch_ms=lat_batch,
        avg_similar_cos=avg_sim,
        avg_dissimilar_cos=avg_dissim,
        separation=separation,
        retrieval_recall_at_3=recall_at_3,
        mem_per_10k_mb=mem_per_10k,
    )

    print(f"  Dimension:           {result.dim}")
    print(f"  Latency (single):    {result.latency_single_ms:.1f} ms")
    print(f"  Latency (batch 15):  {result.latency_batch_ms:.1f} ms")
    print(f"  Avg similar cos:     {result.avg_similar_cos:.4f}")
    print(f"  Avg dissimilar cos:  {result.avg_dissimilar_cos:.4f}")
    print(f"  Separation (Δ):      {result.separation:.4f}")
    print(f"  Retrieval R@3:       {result.retrieval_recall_at_3:.1%}")
    print(f"  HNSW mem/10K (F16):  {result.mem_per_10k_mb:.1f} MB")

    return result


def main():
    print("Embedding Model Comparison Benchmark")
    print("=" * 60)

    results: list[ModelResult] = []

    # Current model
    results.append(benchmark_model("qwen2.5-coder-7b", QWEN_URL))

    # Jina v5 full 1024d
    results.append(benchmark_model("jina-v5-small", JINA_URL))

    # Jina v5 Matryoshka dimensions
    for dim in [512, 384, 256, 128]:
        results.append(benchmark_model("jina-v5-small", JINA_URL, dim_override=dim))

    # Summary table
    print(f"\n\n{'='*100}")
    print("  COMPARISON SUMMARY")
    print(f"{'='*100}")
    header = f"{'Model':<25} {'Dim':>5} {'Lat1(ms)':>9} {'Lat15(ms)':>10} {'SimCos':>7} {'DisCos':>7} {'Δ':>7} {'R@3':>6} {'Mem/10K':>8}"
    print(header)
    print("-" * len(header))
    for r in results:
        print(
            f"{r.name:<25} {r.dim:>5} {r.latency_single_ms:>9.1f} {r.latency_batch_ms:>10.1f} "
            f"{r.avg_similar_cos:>7.4f} {r.avg_dissimilar_cos:>7.4f} {r.separation:>7.4f} "
            f"{r.retrieval_recall_at_3:>5.0%} {r.mem_per_10k_mb:>7.1f}MB"
        )

    # Recommendation
    print(f"\n{'='*60}")
    print("  RECOMMENDATION")
    print(f"{'='*60}")
    best = max(results, key=lambda r: r.separation * 0.4 + r.retrieval_recall_at_3 * 0.4 - r.mem_per_10k_mb / 100 * 0.2)
    print(f"  Best overall: {best.name} (dim={best.dim})")
    print(f"    Separation: {best.separation:.4f}, R@3: {best.retrieval_recall_at_3:.0%}, Mem/10K: {best.mem_per_10k_mb:.1f}MB")


if __name__ == "__main__":
    main()
