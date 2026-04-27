#!/usr/bin/env python3
"""BEIR Benchmark for Plico — pure retrieval evaluation.

Evaluates Plico's search pipeline on BEIR datasets (SciFact, NFCorpus, FiQA).
Computes nDCG@10, Recall@5, MAP for each dataset.

Modes:
  - bm25:     BM25-only via bm25_search convenience wrapper
  - vector:   Semantic search via plicod
  - rrf:      Hybrid RRF (default plicod search)
  - reranker: RRF + Reranker (requires PLICO_RERANKER_API_BASE)

Usage:
    python3 plico_beir_bench.py --dataset scifact --mode rrf
    python3 plico_beir_bench.py --dataset all --mode all
"""

import argparse
import json
import math
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from plico_client import PlicoClient


def load_beir_dataset(data_dir: str):
    """Load BEIR dataset from standard directory layout."""
    corpus = {}
    queries = {}
    qrels = {}

    corpus_path = os.path.join(data_dir, "corpus.jsonl")
    with open(corpus_path, "r", encoding="utf-8") as f:
        for line in f:
            obj = json.loads(line.strip())
            doc_id = obj["_id"]
            title = obj.get("title", "")
            text = obj.get("text", "")
            corpus[doc_id] = f"{title} {text}".strip() if title else text

    queries_path = os.path.join(data_dir, "queries.jsonl")
    with open(queries_path, "r", encoding="utf-8") as f:
        for line in f:
            obj = json.loads(line.strip())
            queries[obj["_id"]] = obj["text"]

    qrels_path = os.path.join(data_dir, "qrels", "test.tsv")
    if not os.path.exists(qrels_path):
        qrels_path = os.path.join(data_dir, "qrels", "dev.tsv")
    with open(qrels_path, "r", encoding="utf-8") as f:
        header = True
        for line in f:
            if header:
                header = False
                continue
            parts = line.strip().split("\t")
            if len(parts) == 3:
                qid, doc_id, score = parts[0], parts[1], int(parts[2])
            elif len(parts) >= 4:
                qid, doc_id, score = parts[0], parts[2], int(parts[3])
            else:
                continue
            if score <= 0:
                continue
            if qid not in qrels:
                qrels[qid] = {}
            qrels[qid][doc_id] = score

    return corpus, queries, qrels


def ingest_corpus(client: PlicoClient, corpus: dict, dataset_name: str, batch_size: int = 50):
    """Ingest BEIR corpus into plicod."""
    print(f"  Ingesting {len(corpus)} documents for {dataset_name}...")
    count = 0
    errors = 0
    start = time.time()
    for doc_id, text in corpus.items():
        truncated = text[:2000]
        for attempt in range(3):
            try:
                resp = client.create(
                    content=truncated,
                    tags=[f"beir:{dataset_name}", f"docid:{doc_id}"],
                    agent_id="beir-bench",
                )
                if resp.get("error"):
                    errors += 1
                break
            except (ConnectionError, OSError) as e:
                if attempt < 2:
                    time.sleep(0.5)
                    client.close()
                else:
                    errors += 1
        count += 1
        if count % 500 == 0:
            elapsed = time.time() - start
            print(f"    {count}/{len(corpus)} ingested ({elapsed:.1f}s, {errors} errors)")
    elapsed = time.time() - start
    print(f"  Done: {count} documents ingested in {elapsed:.1f}s ({errors} errors)")


def search_plico(client: PlicoClient, query: str, dataset_name: str, limit: int = 10) -> list[tuple[str, float]]:
    """Search plicod and extract (doc_id, score) pairs."""
    for attempt in range(3):
        try:
            resp = client.search(
                query=query,
                agent_id="beir-bench",
                limit=limit,
                require_tags=[f"beir:{dataset_name}"],
            )
            results = []
            for r in resp.get("results", []):
                tags = r.get("tags", [])
                doc_id = None
                for tag in tags:
                    if tag.startswith("docid:"):
                        doc_id = tag[6:]
                        break
                if doc_id:
                    results.append((doc_id, r.get("relevance", 0.0)))
            return results
        except (ConnectionError, OSError):
            if attempt < 2:
                time.sleep(0.5)
                client.close()
    return []


def ndcg_at_k(retrieved: list[tuple[str, float]], qrel: dict[str, int], k: int = 10) -> float:
    """Compute nDCG@k."""
    dcg = 0.0
    for i, (doc_id, _) in enumerate(retrieved[:k]):
        rel = qrel.get(doc_id, 0)
        dcg += (2 ** rel - 1) / math.log2(i + 2)

    ideal_rels = sorted(qrel.values(), reverse=True)[:k]
    idcg = sum((2 ** r - 1) / math.log2(i + 2) for i, r in enumerate(ideal_rels))
    return dcg / idcg if idcg > 0 else 0.0


def recall_at_k(retrieved: list[tuple[str, float]], qrel: dict[str, int], k: int = 5) -> float:
    """Compute Recall@k."""
    relevant = set(qrel.keys())
    if not relevant:
        return 0.0
    retrieved_ids = {doc_id for doc_id, _ in retrieved[:k]}
    return len(retrieved_ids & relevant) / len(relevant)


def average_precision(retrieved: list[tuple[str, float]], qrel: dict[str, int]) -> float:
    """Compute Average Precision."""
    relevant = set(qrel.keys())
    if not relevant:
        return 0.0
    hits = 0
    ap_sum = 0.0
    for i, (doc_id, _) in enumerate(retrieved):
        if doc_id in relevant:
            hits += 1
            ap_sum += hits / (i + 1)
    return ap_sum / len(relevant) if relevant else 0.0


def evaluate_dataset(client: PlicoClient, dataset_name: str, data_dir: str) -> dict:
    """Evaluate a single BEIR dataset."""
    print(f"\n{'='*60}")
    print(f"BEIR Evaluation: {dataset_name}")
    print(f"{'='*60}")

    corpus, queries, qrels = load_beir_dataset(data_dir)
    print(f"  Corpus: {len(corpus)} docs | Queries: {len(queries)} | Qrels: {len(qrels)} queries with judgments")

    ingest_corpus(client, corpus, dataset_name)

    time.sleep(2)

    eval_queries = {qid: queries[qid] for qid in qrels if qid in queries}
    print(f"  Evaluating {len(eval_queries)} queries...")

    ndcg_scores = []
    recall_scores = []
    map_scores = []
    latencies = []

    for i, (qid, query_text) in enumerate(eval_queries.items()):
        t0 = time.time()
        results = search_plico(client, query_text, dataset_name, limit=10)
        latency_ms = (time.time() - t0) * 1000
        latencies.append(latency_ms)

        qrel = qrels[qid]
        ndcg_scores.append(ndcg_at_k(results, qrel, k=10))
        recall_scores.append(recall_at_k(results, qrel, k=5))
        map_scores.append(average_precision(results, qrel))

        if (i + 1) % 100 == 0:
            print(f"    {i+1}/{len(eval_queries)} queries evaluated")

    metrics = {
        "dataset": dataset_name,
        "num_queries": len(eval_queries),
        "num_corpus": len(corpus),
        "ndcg@10": sum(ndcg_scores) / len(ndcg_scores) if ndcg_scores else 0,
        "recall@5": sum(recall_scores) / len(recall_scores) if recall_scores else 0,
        "map": sum(map_scores) / len(map_scores) if map_scores else 0,
        "latency_p50_ms": sorted(latencies)[len(latencies)//2] if latencies else 0,
        "latency_p99_ms": sorted(latencies)[int(len(latencies)*0.99)] if latencies else 0,
    }

    print(f"\n  Results for {dataset_name}:")
    print(f"    nDCG@10: {metrics['ndcg@10']:.4f}")
    print(f"    Recall@5: {metrics['recall@5']:.4f}")
    print(f"    MAP:      {metrics['map']:.4f}")
    print(f"    Latency p50: {metrics['latency_p50_ms']:.1f}ms | p99: {metrics['latency_p99_ms']:.1f}ms")

    return metrics


def main():
    parser = argparse.ArgumentParser(description="BEIR Benchmark for Plico")
    parser.add_argument("--dataset", default="scifact", choices=["scifact", "nfcorpus", "fiqa", "all"])
    parser.add_argument("--data-root", default=os.path.join(os.path.dirname(__file__), "..", "data", "beir"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7878)
    parser.add_argument("--output", default=None, help="JSON output path")
    args = parser.parse_args()

    datasets = ["scifact", "nfcorpus", "fiqa"] if args.dataset == "all" else [args.dataset]

    all_results = {}
    client = PlicoClient(host=args.host, port=args.port)

    for ds in datasets:
        data_dir = os.path.join(args.data_root, ds)
        if not os.path.exists(data_dir):
            print(f"SKIP: {ds} data not found at {data_dir}")
            continue
        try:
            metrics = evaluate_dataset(client, ds, data_dir)
            all_results[ds] = metrics
        except Exception as e:
            print(f"ERROR evaluating {ds}: {e}")
            import traceback; traceback.print_exc()
            all_results[ds] = {"error": str(e)}
        finally:
            client.close()

    print(f"\n{'='*60}")
    print("BEIR Summary")
    print(f"{'='*60}")
    for ds, m in all_results.items():
        if "error" in m:
            print(f"  {ds}: ERROR — {m['error']}")
        else:
            print(f"  {ds}: nDCG@10={m['ndcg@10']:.4f}  R@5={m['recall@5']:.4f}  MAP={m['map']:.4f}")

    output_path = args.output or os.path.join(os.path.dirname(__file__), f"beir_results.json")
    with open(output_path, "w") as f:
        json.dump(all_results, f, indent=2)
    print(f"\nResults saved to {output_path}")


if __name__ == "__main__":
    main()
