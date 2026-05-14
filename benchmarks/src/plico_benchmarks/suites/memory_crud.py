"""Memory CRUD correctness suite — MemBench-style."""

from __future__ import annotations

import time
from typing import Any

from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


class MemoryCrudSuite(SuiteBase):
    name = "memory-crud"
    description = "Memory CRUD correctness evaluation"

    def setup(self) -> None:
        self.wait_for_plico()
        self.agent_id = "crud-test"

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 100
        results = []

        # CREATE
        cids = []
        create_latencies = []
        for i in range(n):
            content = f"CRUD item {i}: artificial intelligence benchmark test content"
            t0 = time.perf_counter()
            resp = self.client.create(
                content, tags=["crud", f"item-{i}"], agent_id=self.agent_id
            )
            create_latencies.append((time.perf_counter() - t0) * 1000)
            cid = resp.get("cid", "")
            cids.append((i, cid, content))

        results.append({
            "operation": "create",
            "count": n,
            "success_rate": sum(1 for _, c, _ in cids if c) / n,
            "avg_latency_ms": sum(create_latencies) / n,
        })

        # READ
        read_latencies = []
        read_errors = 0
        for i, cid, expected in cids:
            t0 = time.perf_counter()
            resp = self.client.read(cid, agent_id=self.agent_id)
            read_latencies.append((time.perf_counter() - t0) * 1000)
            actual = resp.get("data", "")
            if actual != expected:
                read_errors += 1

        results.append({
            "operation": "read",
            "count": n,
            "success_rate": (n - read_errors) / n,
            "avg_latency_ms": sum(read_latencies) / n,
        })

        # Wait for indexing before verifying retrievability
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # SEARCH (verify retrievability)
        search_hits = 0
        search_latencies = []
        for i, cid, _ in cids[: min(20, n)]:
            t0 = time.perf_counter()
            resp = self.client.search(f"CRUD item {i}", agent_id=self.agent_id, limit=5)
            search_latencies.append((time.perf_counter() - t0) * 1000)
            hits = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in hits:
                search_hits += 1

        results.append({
            "operation": "search",
            "count": min(20, n),
            "hit_rate": search_hits / min(20, n),
            "avg_latency_ms": sum(search_latencies) / len(search_latencies) if search_latencies else 0,
        })

        # UPDATE (re-create with same CID concept — via semantic update)
        update_latencies = []
        update_cids = []
        for i, old_cid, _ in cids[: min(20, n)]:
            new_content = f"CRUD item {i}: UPDATED content"
            t0 = time.perf_counter()
            resp = self.client.create(
                new_content, tags=["crud", f"item-{i}", "updated"], agent_id=self.agent_id
            )
            update_latencies.append((time.perf_counter() - t0) * 1000)
            update_cids.append((i, resp.get("cid", ""), new_content))

        results.append({
            "operation": "update",
            "count": len(update_cids),
            "success_rate": sum(1 for _, c, _ in update_cids if c) / len(update_cids),
            "avg_latency_ms": sum(update_latencies) / len(update_latencies) if update_latencies else 0,
        })

        # BATCH CREATE
        batch_items = [
            {"content": f"Batch item {i}", "tags": ["crud", "batch"]}
            for i in range(50)
        ]
        t0 = time.perf_counter()
        resp = self.client.batch_create(batch_items, agent_id=self.agent_id)
        batch_lat = (time.perf_counter() - t0) * 1000
        results.append({
            "operation": "batch_create",
            "count": 50,
            "success_rate": 1.0 if not resp.get("error") else 0.0,
            "avg_latency_ms": batch_lat,
        })

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {
                "count": r["count"],
                "success_rate": round(r.get("success_rate", 0) * 100, 1),
                "hit_rate": round(r.get("hit_rate", 0) * 100, 1),
                "avg_latency_ms": round(r.get("avg_latency_ms", 0), 2),
            }
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v44",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)
