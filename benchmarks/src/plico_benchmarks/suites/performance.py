"""Performance micro-benchmark suite."""

from __future__ import annotations

import statistics
import time
from typing import Any

import numpy as np

from plico_benchmarks.suites.base import SuiteBase
from plico_benchmarks.core.metrics import latency_percentiles
from plico_benchmarks.core.reporter import Report


class PerformanceSuite(SuiteBase):
    name = "performance"
    description = "CAS write, search, memory recall, KG path micro-benchmarks"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        results = []
        results.append(self._bench_cas_write(1000))
        results.append(self._bench_search(500))
        results.append(self._bench_memory(500))
        results.append(self._bench_kg(200, 300))
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {
                "qps": r.get("qps"),
                "p50_ms": r.get("p50_ms"),
                "p95_ms": r.get("p95_ms"),
                "p99_ms": r.get("p99_ms"),
            }
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v44",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {},
            "metrics": metrics,
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _bench_cas_write(self, n: int = 1000) -> dict[str, Any]:
        latencies = []
        for i in range(n):
            content = f"perf obj {i}: " + "x" * 200
            t0 = time.perf_counter()
            self.client.create(content, tags=["perf", f"batch-{i % 10}"])
            latencies.append((time.perf_counter() - t0) * 1000)
        p = latency_percentiles(latencies)
        return {
            "operation": "cas_write",
            "count": n,
            "qps": round(n / (sum(latencies) / 1000), 1),
            "p50_ms": p["p50"],
            "p95_ms": p["p95"],
            "p99_ms": p["p99"],
        }

    def _bench_search(self, n: int = 500) -> dict[str, Any]:
        # Seed some data first
        for i in range(100):
            self.client.create(f"search seed {i} machine learning", tags=["perf", "ml"])
        # Wait for indexing before measuring search latency
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)
        latencies = []
        queries = ["machine learning", "neural network", "deep learning"] * (n // 3 + 1)
        for i in range(n):
            t0 = time.perf_counter()
            self.client.search(queries[i], limit=10)
            latencies.append((time.perf_counter() - t0) * 1000)
        p = latency_percentiles(latencies)
        return {
            "operation": "search",
            "count": n,
            "qps": round(n / (sum(latencies) / 1000), 1),
            "p50_ms": p["p50"],
            "p95_ms": p["p95"],
            "p99_ms": p["p99"],
        }

    def _bench_memory(self, n: int = 500) -> dict[str, Any]:
        agent_id = "perf-mem"
        for i in range(100):
            self.client.remember(agent_id, f"memory fact {i}: artificial intelligence")
        # Memory recall may also depend on async indexing
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)
        latencies = []
        for i in range(n):
            t0 = time.perf_counter()
            self.client.recall(agent_id, query="artificial intelligence", limit=10)
            latencies.append((time.perf_counter() - t0) * 1000)
        p = latency_percentiles(latencies)
        return {
            "operation": "memory_recall",
            "count": n,
            "qps": round(n / (sum(latencies) / 1000), 1),
            "p50_ms": p["p50"],
            "p95_ms": p["p95"],
            "p99_ms": p["p99"],
        }

    def _bench_kg(self, n_nodes: int = 200, n_edges: int = 300) -> dict[str, Any]:
        nodes = []
        t0 = time.perf_counter()
        for i in range(n_nodes):
            r = self.client.add_node(f"entity-{i}", node_type="Entity")
            nodes.append(r.get("node_id", f"e{i}"))
        node_lat = (time.perf_counter() - t0) * 1000

        t0 = time.perf_counter()
        for i in range(n_edges):
            src = nodes[i % len(nodes)]
            dst = nodes[(i + 1) % len(nodes)]
            self.client.add_edge(src, dst, edge_type="RelatedTo")
        edge_lat = (time.perf_counter() - t0) * 1000

        t0 = time.perf_counter()
        self.client.find_paths(nodes[0], nodes[-1], max_depth=3)
        path_lat = (time.perf_counter() - t0) * 1000

        return {
            "operation": "kg_path",
            "node_count": n_nodes,
            "edge_count": n_edges,
            "node_p50_ms": round(node_lat / n_nodes, 2),
            "edge_p50_ms": round(edge_lat / n_edges, 2),
            "path_latency_ms": round(path_lat, 2),
        }
