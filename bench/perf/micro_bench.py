#!/usr/bin/env python3
"""
Dimension 5: Performance Micro-benchmarks

Tests CAS, vector search, and KG operation throughput/latency
via plicod TCP API.

Metrics: QPS, latency P50/P95/P99
"""

import json
import os
import statistics
import sys
import time
from pathlib import Path

import numpy as np

BENCH_ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = BENCH_ROOT / "results"
RESULTS_FILE = RESULTS_DIR / "perf_micro.json"

sys.path.insert(0, str(BENCH_ROOT))
from plico_client import PlicoClient

HOST = os.environ.get("PLICO_HOST", "127.0.0.1")
PORT = int(os.environ.get("PLICO_PORT", "7878"))


def percentile(data: list[float], p: float) -> float:
    if not data:
        return 0.0
    data_sorted = sorted(data)
    idx = (len(data_sorted) - 1) * p / 100.0
    lo = int(idx)
    hi = min(lo + 1, len(data_sorted) - 1)
    frac = idx - lo
    return data_sorted[lo] * (1 - frac) + data_sorted[hi] * frac


def bench_cas_write(client: PlicoClient, n: int = 1000) -> dict:
    """Benchmark CAS create operations."""
    print(f"  CAS write ({n} objects) ...")
    latencies = []
    cids = []
    for i in range(n):
        content = f"benchmark object {i}: " + "x" * 200
        t0 = time.perf_counter()
        resp = client.create(content, tags=["bench", f"batch-{i % 10}"], agent_id="perf-bench")
        dt = (time.perf_counter() - t0) * 1000
        latencies.append(dt)
        if resp.get("ok"):
            cids.append(resp.get("cid", ""))

    elapsed = sum(latencies) / 1000
    return {
        "operation": "cas_write",
        "count": n,
        "qps": round(n / max(elapsed, 0.001)),
        "latency_p50_ms": round(percentile(latencies, 50), 2),
        "latency_p95_ms": round(percentile(latencies, 95), 2),
        "latency_p99_ms": round(percentile(latencies, 99), 2),
        "total_time_s": round(elapsed, 2),
        "_cids": cids[:10],
    }


def bench_cas_read(client: PlicoClient, cids: list[str]) -> dict:
    """Benchmark CAS read operations."""
    n = len(cids)
    if n == 0:
        return {"operation": "cas_read", "count": 0, "error": "no cids"}
    print(f"  CAS read ({n} objects) ...")
    latencies = []
    for cid in cids:
        t0 = time.perf_counter()
        client.read(cid, agent_id="perf-bench")
        dt = (time.perf_counter() - t0) * 1000
        latencies.append(dt)

    elapsed = sum(latencies) / 1000
    return {
        "operation": "cas_read",
        "count": n,
        "qps": round(n / max(elapsed, 0.001)),
        "latency_p50_ms": round(percentile(latencies, 50), 2),
        "latency_p95_ms": round(percentile(latencies, 95), 2),
        "latency_p99_ms": round(percentile(latencies, 99), 2),
        "total_time_s": round(elapsed, 2),
    }


def bench_search(client: PlicoClient, n_queries: int = 200) -> dict:
    """Benchmark semantic search (requires objects to already be indexed)."""
    print(f"  Semantic search ({n_queries} queries) ...")
    queries = [
        "benchmark performance test",
        "object storage retrieval",
        "memory recall system",
        "knowledge graph traversal",
        "agent scheduling coordination",
    ]
    latencies = []
    for i in range(n_queries):
        q = queries[i % len(queries)]
        t0 = time.perf_counter()
        client.search(q, agent_id="perf-bench", limit=10)
        dt = (time.perf_counter() - t0) * 1000
        latencies.append(dt)

    elapsed = sum(latencies) / 1000
    return {
        "operation": "search",
        "count": n_queries,
        "qps": round(n_queries / max(elapsed, 0.001)),
        "latency_p50_ms": round(percentile(latencies, 50), 2),
        "latency_p95_ms": round(percentile(latencies, 95), 2),
        "latency_p99_ms": round(percentile(latencies, 99), 2),
        "total_time_s": round(elapsed, 2),
    }


def bench_memory(client: PlicoClient, n: int = 500) -> dict:
    """Benchmark memory store + recall cycle."""
    print(f"  Memory store+recall ({n} cycles) ...")
    store_lats = []
    recall_lats = []
    agent = "perf-mem"

    for i in range(n):
        content = f"memory item {i}: important fact about topic {i % 20}"
        t0 = time.perf_counter()
        client.remember(agent, content)
        store_lats.append((time.perf_counter() - t0) * 1000)

    for i in range(min(n, 200)):
        t0 = time.perf_counter()
        client.recall(agent, query=f"topic {i % 20}", limit=5)
        recall_lats.append((time.perf_counter() - t0) * 1000)

    return {
        "operation": "memory_store_recall",
        "store_count": n,
        "recall_count": len(recall_lats),
        "store_qps": round(n / max(sum(store_lats) / 1000, 0.001)),
        "store_p50_ms": round(percentile(store_lats, 50), 2),
        "store_p95_ms": round(percentile(store_lats, 95), 2),
        "recall_qps": round(len(recall_lats) / max(sum(recall_lats) / 1000, 0.001)),
        "recall_p50_ms": round(percentile(recall_lats, 50), 2),
        "recall_p95_ms": round(percentile(recall_lats, 95), 2),
    }


def bench_kg(client: PlicoClient, n_nodes: int = 200, n_edges: int = 300) -> dict:
    """Benchmark knowledge graph operations."""
    print(f"  KG operations ({n_nodes} nodes, {n_edges} edges) ...")
    agent = "perf-kg"
    node_lats = []
    edge_lats = []
    path_lats = []

    node_ids = []
    for i in range(n_nodes):
        t0 = time.perf_counter()
        resp = client.add_node(f"entity-{i}", "concept", agent_id=agent)
        node_lats.append((time.perf_counter() - t0) * 1000)
        nid = resp.get("node_id", "")
        if nid:
            node_ids.append(nid)

    for i in range(min(n_edges, len(node_ids) * (len(node_ids) - 1))):
        src = node_ids[i % len(node_ids)]
        tgt = node_ids[(i + 1) % len(node_ids)]
        t0 = time.perf_counter()
        client.add_edge(src, tgt, "related_to", agent_id=agent)
        edge_lats.append((time.perf_counter() - t0) * 1000)

    for i in range(min(50, len(node_ids) - 1)):
        src = node_ids[i]
        tgt = node_ids[(i + 5) % len(node_ids)]
        t0 = time.perf_counter()
        client.find_paths(src, tgt, agent_id=agent, max_depth=3)
        path_lats.append((time.perf_counter() - t0) * 1000)

    return {
        "operation": "kg_operations",
        "add_node_count": n_nodes,
        "add_edge_count": len(edge_lats),
        "find_paths_count": len(path_lats),
        "add_node_qps": round(n_nodes / max(sum(node_lats) / 1000, 0.001)),
        "add_node_p50_ms": round(percentile(node_lats, 50), 2),
        "add_edge_qps": round(len(edge_lats) / max(sum(edge_lats) / 1000, 0.001)) if edge_lats else 0,
        "add_edge_p50_ms": round(percentile(edge_lats, 50), 2) if edge_lats else 0,
        "find_paths_p50_ms": round(percentile(path_lats, 50), 2) if path_lats else 0,
        "find_paths_p95_ms": round(percentile(path_lats, 95), 2) if path_lats else 0,
    }


def run_benchmark():
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)

    print(f"Connecting to plicod at {HOST}:{PORT} ...")
    try:
        client = PlicoClient(HOST, PORT, timeout=30)
        client.connect()
    except Exception as e:
        print(f"ERROR: cannot connect to plicod: {e}")
        print("Start plicod first: EMBEDDING_BACKEND=stub target/release/plicod --port 7878")
        sys.exit(1)

    health = client.health()
    print(f"  plicod status: ok={health.get('ok')}")

    results = []

    # CAS write
    cas_w = bench_cas_write(client, n=1000)
    results.append(cas_w)
    cids = cas_w.pop("_cids", [])

    # CAS read (using cids from write)
    if cids:
        cas_r = bench_cas_read(client, cids * 100)  # repeat to get enough samples
        results.append(cas_r)

    # Search
    search_r = bench_search(client, n_queries=200)
    results.append(search_r)

    # Memory
    mem_r = bench_memory(client, n=500)
    results.append(mem_r)

    # KG
    kg_r = bench_kg(client, n_nodes=200, n_edges=300)
    results.append(kg_r)

    client.close()

    report = {
        "benchmark": "Plico Performance Micro-benchmarks",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "host": HOST,
        "port": PORT,
        "results": results,
    }

    with open(RESULTS_FILE, "w") as f:
        json.dump(report, f, indent=2)

    print("\n" + "=" * 70)
    print("Performance Micro-benchmark Results")
    print("=" * 70)
    for r in results:
        op = r["operation"]
        if "qps" in r:
            print(f"  {op:<25} QPS={r['qps']:>6}  P50={r.get('latency_p50_ms', r.get('store_p50_ms', 0)):>6.1f}ms  P95={r.get('latency_p95_ms', r.get('store_p95_ms', 0)):>6.1f}ms")
        else:
            for k, v in r.items():
                if k != "operation" and "qps" in k:
                    print(f"  {op}.{k:<20} {v}")
    print(f"\nResults saved to: {RESULTS_FILE}")
    return report


if __name__ == "__main__":
    run_benchmark()
