"""KG reasoning suite — multi-hop path finding."""

from __future__ import annotations

import time
from typing import Any

from plico_benchmarks.suites.base import SuiteBase
from plico_benchmarks.core.reporter import Report


class KGReasoningSuite(SuiteBase):
    name = "kg-reasoning"
    description = "Knowledge graph multi-hop reasoning"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 50
        # Build a star topology: hub node connected to all others.
        # Shortest path between any two leaf nodes = 2 hops (leaf -> hub -> leaf).
        # This tests path-finding at depth 2, 3, 4 realistically.
        hub = self.client.add_node("hub-entity", node_type="Entity")
        hub_id = hub.get("node_id", "hub")

        nodes = []
        for i in range(n):
            r = self.client.add_node(f"entity-{i}", node_type="Entity")
            node_id = r.get("node_id", f"e{i}")
            nodes.append(node_id)
            # Connect hub -> leaf
            self.client.add_edge(hub_id, node_id, edge_type="RelatedTo")
            # Also chain some leaves for longer paths: entity-i -> entity-(i+1)
            if i > 0:
                self.client.add_edge(nodes[i - 1], node_id, edge_type="Follows")

        results = []
        # Test path finding between leaf nodes at various depths
        src, dst = nodes[0], nodes[-1]
        for depth in [2, 3, 4, n]:
            t0 = time.perf_counter()
            resp = self.client.find_paths(src, dst, max_depth=depth)
            latency = (time.perf_counter() - t0) * 1000
            paths = resp.get("paths", [])
            results.append({
                "max_depth": depth,
                "paths_found": len(paths),
                "latency_ms": latency,
            })

        # Test weighted path
        t0 = time.perf_counter()
        resp = self.client.find_paths(src, dst, max_depth=n, weighted=True)
        weighted_latency = (time.perf_counter() - t0) * 1000
        weighted_paths = resp.get("paths", [])
        results.append({
            "max_depth": n,
            "paths_found": len(weighted_paths),
            "latency_ms": weighted_latency,
            "weighted": True,
        })

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        unweighted = [r for r in raw if not r.get("weighted")]
        weighted = [r for r in raw if r.get("weighted")]
        overall = {
            "n_nodes": self.samples or 50,
            "avg_paths_unweighted": (
                sum(r["paths_found"] for r in unweighted) / max(len(unweighted), 1)
            ),
            "avg_paths_weighted": (
                sum(r["paths_found"] for r in weighted) / max(len(weighted), 1)
            ),
            "avg_latency_ms": sum(r["latency_ms"] for r in raw) / max(len(raw), 1),
        }
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v46",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {},
            "metrics": metrics,
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)
