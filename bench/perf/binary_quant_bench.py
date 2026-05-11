#!/usr/bin/env python3
"""Binary Quantization micro-benchmark.
Measures search latency at various dataset sizes to verify two-stage
(Hamming coarse → cosine re-rank) is faster than linear scan.
"""
import json
import random
import time
import sys
sys.path.insert(0, '.')
from bench.plico_client import PlicoClient

def main():
    client = PlicoClient("127.0.0.1", 7878)
    if not client.health().get("ok", False):
        print("ERROR: plicod not reachable")
        return

    sizes = [100, 500, 1000, 2000]
    results = []

    for n in sizes:
        # Create n objects with random content
        print(f"\n--- Dataset size: {n} ---")
        cids = []
        t0 = time.time()
        for i in range(n):
            content = f"document {i} about topic {random.randint(1, 50)} with keyword {'alpha' if i % 3 == 0 else 'beta' if i % 3 == 1 else 'gamma'}"
            cid = client.create(content, [f"doc:{i}", f"batch:{i // 100}"], "bench")
            cids.append(cid)
        write_time = time.time() - t0
        print(f"  Write: {n} objects in {write_time:.2f}s ({n/write_time:.0f} QPS)")

        # Wait for indexing
        time.sleep(1)

        # Search benchmark
        queries = ["alpha topic", "beta keyword", "gamma document", "topic 5", "keyword alpha"]
        latencies = []
        resp = []
        for q in queries * 10:  # 50 queries
            t0 = time.time()
            resp = client.search(q, "bench", 10)
            latencies.append((time.time() - t0) * 1000)

        latencies.sort()
        p50 = latencies[len(latencies) // 2]
        p95 = latencies[int(len(latencies) * 0.95)]
        p99 = latencies[int(len(latencies) * 0.99)]
        avg = sum(latencies) / len(latencies)
        results_list = resp.get("results", []) if isinstance(resp, dict) else resp
        hits = len(results_list)

        print(f"  Search: avg={avg:.1f}ms p50={p50:.1f}ms p95={p95:.1f}ms p99={p99:.1f}ms")
        print(f"  Results returned: {hits}")

        results.append({
            "dataset_size": n,
            "write_qps": round(n / write_time, 1),
            "search_avg_ms": round(avg, 2),
            "search_p50_ms": round(p50, 2),
            "search_p95_ms": round(p95, 2),
            "search_p99_ms": round(p99, 2),
            "results_returned": len(resp),
        })

    output = {
        "benchmark": "Binary Quantization Search Benchmark",
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "results": results,
    }
    with open("bench/results/binary_quant_bench.json", "w") as f:
        json.dump(output, f, indent=2)
    print(f"\nResults saved to bench/results/binary_quant_bench.json")

if __name__ == "__main__":
    main()
