#!/usr/bin/env python3
"""
CAS Write Micro-Benchmark — measures per-step latency of the Create API.

Tests both stub-embedding and real-embedding scenarios.
Outputs: total latency, per-write breakdown (CAS + index + persist).
"""

import json
import os
import socket
import struct
import time
import sys
from pathlib import Path

PLICOD_HOST = "127.0.0.1"
PLICOD_PORT = int(os.environ.get("PLICOD_PORT", "7878"))

def send_recv(sock, req: dict, timeout=30) -> dict:
    payload = json.dumps(req).encode("utf-8")
    header = struct.pack(">I", len(payload))
    sock.sendall(header + payload)
    raw_len = b""
    while len(raw_len) < 4:
        chunk = sock.recv(4 - len(raw_len))
        if not chunk:
            raise ConnectionError("connection closed reading length")
        raw_len += chunk
    length = struct.unpack(">I", raw_len)[0]
    data = b""
    while len(data) < length:
        chunk = sock.recv(min(length - len(data), 65536))
        if not chunk:
            raise ConnectionError("connection closed reading payload")
        data += chunk
    return json.loads(data)


def connect():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    sock.settimeout(30)
    sock.connect((PLICOD_HOST, PLICOD_PORT))
    return sock


def bench_create(n_writes=20, content_size=500):
    """Benchmark N sequential creates, measuring per-write latency."""
    sock = connect()

    # Warm up with health check
    resp = send_recv(sock, {"method": "health_report"})
    assert resp.get("ok"), f"Health check failed: {resp}"

    latencies = []
    for i in range(n_writes):
        content = f"Benchmark test document {i}. " + ("x" * content_size)

        t0 = time.perf_counter()
        resp = send_recv(sock, {
            "method": "create",
            "content": content,
            "tags": ["bench", f"doc-{i}"],
            "agent_id": "bench-agent",
        })
        elapsed_ms = (time.perf_counter() - t0) * 1000

        if not resp.get("ok"):
            print(f"  Write {i} FAILED: {resp.get('error', 'unknown')}")
            continue

        latencies.append(elapsed_ms)
        if (i + 1) % 5 == 0 or i == 0:
            print(f"  Write {i+1}/{n_writes}: {elapsed_ms:.1f}ms (cid={resp.get('cid', '?')[:12]}...)")

    sock.close()

    if not latencies:
        print("ERROR: No successful writes")
        return

    import statistics

    avg = statistics.mean(latencies)
    p50 = statistics.median(latencies)
    p95 = sorted(latencies)[int(len(latencies) * 0.95)]
    p99 = sorted(latencies)[int(len(latencies) * 0.99)]
    qps = 1000.0 / avg if avg > 0 else 0

    print(f"\n{'='*60}")
    print(f"CAS Write Benchmark Results ({n_writes} writes, {content_size} bytes)")
    print(f"{'='*60}")
    print(f"  Total: {sum(latencies):.0f}ms")
    print(f"  Mean:  {avg:.1f}ms")
    print(f"  P50:   {p50:.1f}ms")
    print(f"  P95:   {p95:.1f}ms")
    print(f"  P99:   {p99:.1f}ms")
    print(f"  Min:   {min(latencies):.1f}ms")
    print(f"  Max:   {max(latencies):.1f}ms")
    print(f"  QPS:   {qps:.1f}")
    print()

    # Identify persist spikes (every 50th write triggers full persist)
    spikes = [(i, lat) for i, lat in enumerate(latencies) if lat > avg * 2]
    if spikes:
        print(f"  Latency spikes (>2x mean):")
        for idx, lat in spikes:
            print(f"    Write #{idx}: {lat:.1f}ms")

    return {
        "n_writes": n_writes,
        "content_size": content_size,
        "avg_ms": round(avg, 1),
        "p50_ms": round(p50, 1),
        "p95_ms": round(p95, 1),
        "p99_ms": round(p99, 1),
        "min_ms": round(min(latencies), 1),
        "max_ms": round(max(latencies), 1),
        "qps": round(qps, 1),
        "latencies": [round(l, 1) for l in latencies],
    }


def bench_search(n_queries=20):
    """Benchmark N sequential searches."""
    sock = connect()
    resp = send_recv(sock, {"method": "health_report"})
    assert resp.get("ok")

    latencies = []
    queries = ["benchmark", "test document", "performance", "latency", "search index",
               "vector embedding", "knowledge graph", "memory system", "CAS storage", "AI agent"]

    for i in range(n_queries):
        q = queries[i % len(queries)]
        t0 = time.perf_counter()
        resp = send_recv(sock, {
            "method": "search",
            "query": q,
            "agent_id": "bench-agent",
            "limit": 10,
        })
        elapsed_ms = (time.perf_counter() - t0) * 1000
        n_results = len(resp.get("results", []))
        latencies.append(elapsed_ms)

        if (i + 1) % 5 == 0 or i == 0:
            print(f"  Search {i+1}/{n_queries}: {elapsed_ms:.1f}ms ({n_results} results)")

    sock.close()

    if not latencies:
        return

    import statistics
    avg = statistics.mean(latencies)
    p50 = statistics.median(latencies)

    print(f"\n  Search: avg={avg:.1f}ms  p50={p50:.1f}ms  qps={1000/avg:.1f}")
    return {"avg_ms": round(avg, 1), "p50_ms": round(p50, 1), "qps": round(1000/avg, 1)}


if __name__ == "__main__":
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 20

    print("=== CAS Write Benchmark ===")
    write_result = bench_create(n_writes=n)

    print("\n=== Search Benchmark ===")
    search_result = bench_search(n_queries=min(n, 20))

    results = {"write": write_result, "search": search_result}
    out = Path(__file__).resolve().parent.parent / "results" / "cas_write_bench.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nResults saved to {out}")
