"""KG reasoning suite — multi-hop path finding."""

from __future__ import annotations

import os
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
            valid = self._validate_paths(paths, src, dst)
            results.append({
                "max_depth": depth,
                "paths_found": len(paths),
                "valid_paths": valid,
                "latency_ms": latency,
            })

        # Test weighted path
        t0 = time.perf_counter()
        resp = self.client.find_paths(src, dst, max_depth=n, weighted=True)
        weighted_latency = (time.perf_counter() - t0) * 1000
        weighted_paths = resp.get("paths", [])
        valid = self._validate_paths(weighted_paths, src, dst)
        results.append({
            "max_depth": n,
            "paths_found": len(weighted_paths),
            "valid_paths": valid,
            "latency_ms": weighted_latency,
            "weighted": True,
        })

        # Test causal chain: create A -> CausedBy -> B -> CausedBy -> C
        cause_nodes = []
        for i in range(5):
            r = self.client.add_node(f"cause-{i}", node_type="Event")
            cause_nodes.append(r.get("node_id", f"c{i}"))
        for i in range(len(cause_nodes) - 1):
            self.client.add_edge(cause_nodes[i], cause_nodes[i + 1], edge_type="CausedBy")

        # Verify causal path traversal
        t0 = time.perf_counter()
        resp = self.client.find_paths(cause_nodes[0], cause_nodes[-1], max_depth=10)
        causal_latency = (time.perf_counter() - t0) * 1000
        causal_paths = resp.get("paths", [])
        causal_valid = self._validate_paths(causal_paths, cause_nodes[0], cause_nodes[-1])
        results.append({
            "max_depth": 10,
            "paths_found": len(causal_paths),
            "valid_paths": causal_valid,
            "latency_ms": causal_latency,
            "causal_chain": True,
        })

        return results

    @staticmethod
    def _validate_paths(paths: list, src: str, dst: str) -> int:
        """Count paths that actually connect src to dst.

        Server returns paths as list[list[KGNodeDto]] — each path is a list
        of node dicts directly (not a dict with a "nodes" key).
        """
        valid = 0
        for path in paths:
            if not path:
                continue
            first_id = path[0].get("id", "")
            last_id = path[-1].get("id", "")
            if (first_id == src and last_id == dst) or \
               (first_id == dst and last_id == src):
                valid += 1
        return valid

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        unweighted = [r for r in raw if not r.get("weighted") and not r.get("causal_chain")]
        weighted = [r for r in raw if r.get("weighted")]
        causal = [r for r in raw if r.get("causal_chain")]

        total_paths = sum(r["paths_found"] for r in raw)
        total_valid = sum(r["valid_paths"] for r in raw)
        validity_rate = total_valid / max(total_paths, 1)

        overall = {
            "n_nodes": self.samples or 50,
            "avg_paths_unweighted": (
                sum(r["paths_found"] for r in unweighted) / max(len(unweighted), 1)
            ),
            "avg_paths_weighted": (
                sum(r["paths_found"] for r in weighted) / max(len(weighted), 1)
            ),
            "avg_latency_ms": sum(r["latency_ms"] for r in raw) / max(len(raw), 1),
            "path_validity_rate": validity_rate,
            "total_paths": total_paths,
            "valid_paths": total_valid,
        }

        if causal:
            overall["causal_paths_found"] = causal[0]["paths_found"]
            overall["causal_valid_paths"] = causal[0]["valid_paths"]
            overall["causal_latency_ms"] = causal[0]["latency_ms"]

        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_cross_benchmarks, get_agent_frameworks

        cross = get_cross_benchmarks()
        frameworks = get_agent_frameworks()
        kg_frameworks = [f for f in frameworks if f.get("kg_native")]

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "hotpotqa": cross.get("hotpotqa", {}).get("baselines", []),
                "agentbench": cross.get("agentbench", {}).get("baselines", []),
                "kg_frameworks": kg_frameworks,
                "note": "No direct KG path-finding benchmark exists. HotpotQA measures multi-hop QA (closest).",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)
