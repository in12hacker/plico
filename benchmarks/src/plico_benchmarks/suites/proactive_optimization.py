"""Proactive optimization suite — Axiom 7: Proactive Before Passive.

Tests Plico's proactive optimization mechanisms:
- Intent prefetch: does pre-loading context for predicted intents improve latency?
- Context layering: L0/L1/L2 reduces tokens without losing accuracy
- Skill discovery: are repeated patterns detected and consolidated?

Note: Some proactive features (intent prefetch, skill forge) are internal
optimizations. This suite tests observable behavior: latency patterns,
context efficiency, and pattern detection.
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.metrics import estimate_tokens
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


class ProactiveOptimizationSuite(SuiteBase):
    name = "proactive-optimization"
    description = "Proactive optimization — prefetch, context layering, pattern detection"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        results = []

        # Phase 1: Context layering efficiency (L0/L1/L2)
        results.extend(self._test_context_layering())

        # Phase 2: Repeated-query latency (tests prefetch/cache effects)
        results.extend(self._test_repeated_query_latency())

        # Phase 3: Pattern detection (repeated content creates consolidation)
        results.extend(self._test_pattern_detection())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_token_efficiency_competitors

        token_comps = get_token_efficiency_competitors()

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "token_efficiency": token_comps,
                "note": "Context layering (L0/L1/L2) vs competitors' token usage. Mastra's prompt-cacheable architecture and TencentDB's 61% token reduction are comparable approaches.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _test_context_layering(self) -> list[dict[str, Any]]:
        """Test L0/L1/L2 context layering — same query, different limits.

        L0 (limit=2): ~100 tokens, minimal context
        L1 (limit=5): ~300 tokens, moderate context
        L2 (limit=15): ~1000 tokens, full context

        Measures: token count at each level, hit rate preservation across levels.
        """
        # Seed data
        for i in range(30):
            self.client.create(
                f"Document {i}: Artificial intelligence encompasses machine learning, "
                f"deep learning, natural language processing, and computer vision. "
                f"Key applications include autonomous vehicles, medical diagnosis, "
                f"and financial fraud detection.",
                tags=["proactive-test", f"doc-{i}"],
            )

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        queries = [
            "artificial intelligence applications",
            "machine learning techniques",
            "natural language processing",
            "computer vision use cases",
            "deep learning architectures",
        ] * 10

        levels = {"L0": 2, "L1": 5, "L2": 15}
        level_tokens: dict[str, list[int]] = {k: [] for k in levels}
        level_hits: dict[str, list[bool]] = {k: [] for k in levels}

        for query in queries:
            for level, limit in levels.items():
                resp = self.client.search(query, limit=limit, require_tags=["proactive-test"])
                snippets = [h.get("snippet", "") for h in resp.get("results", [])]
                tokens = estimate_tokens("\n".join(snippets))
                level_tokens[level].append(tokens)
                level_hits[level].append(bool(snippets))

        results = []
        for level in levels:
            avg_tokens = sum(level_tokens[level]) / len(level_tokens[level]) if level_tokens[level] else 0
            hit_rate = sum(level_hits[level]) / len(level_hits[level]) if level_hits[level] else 0
            results.append({
                "operation": f"context_{level.lower()}",
                "avg_tokens_per_query": round(avg_tokens, 1),
                "hit_rate": round(hit_rate, 3),
                "pct_of_full": round(avg_tokens / 26031 * 100, 2),  # vs full-context baseline
            })

        return results

    def _test_repeated_query_latency(self) -> list[dict[str, Any]]:
        """Test if repeated queries benefit from caching/prefetch.

        Run the same query, then different queries, then the original again.
        If Plico's proactive optimization works, the second occurrence should
        be faster even after cache-busting queries in between.
        """
        # Seed data
        for i in range(20):
            self.client.create(
                f"Cache test {i}: The quick brown fox jumps over the lazy dog.",
                tags=["proactive-test", "cache"],
            )

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        target_query = "quick brown fox"
        bust_queries = [
            "artificial intelligence revolution",
            "quantum computing advances",
            "neural network architectures",
            "database optimization techniques",
            "cloud infrastructure scaling",
        ]

        # Phase 1: Cold — first occurrence of target query
        latencies_cold = []
        for _ in range(5):
            t0 = time.perf_counter()
            self.client.search(target_query, limit=10, require_tags=["proactive-test"])
            latencies_cold.append((time.perf_counter() - t0) * 1000)

        # Phase 2: Cache-bust — run different queries to flush any naive cache
        for q in bust_queries * 2:
            self.client.search(q, limit=10, require_tags=["proactive-test"])

        # Phase 3: Warm — target query again (should benefit from Plico's prefetch)
        latencies_warm = []
        for _ in range(5):
            t0 = time.perf_counter()
            self.client.search(target_query, limit=10, require_tags=["proactive-test"])
            latencies_warm.append((time.perf_counter() - t0) * 1000)

        avg_cold = sum(latencies_cold) / len(latencies_cold) if latencies_cold else 0
        avg_warm = sum(latencies_warm) / len(latencies_warm) if latencies_warm else 0
        speedup = (avg_cold - avg_warm) / avg_cold * 100 if avg_cold > 0 else 0

        return [{
            "operation": "repeated_query_latency",
            "cold_avg_ms": round(avg_cold, 2),
            "warm_avg_ms": round(avg_warm, 2),
            "speedup_pct": round(speedup, 1),
            "cache_bust_queries": len(bust_queries) * 2,
        }]

    def _test_pattern_detection(self) -> list[dict[str, Any]]:
        """Test if the system detects repeated content patterns.

        Create many items with similar content (repeated pattern).
        Then search for the pattern — should find all items.
        This tests whether the system consolidates or indexes repeated patterns.
        """
        agent_id = "pattern-test"
        n = 30

        # Create items with a repeated pattern, tracking CIDs
        pattern_cids = []
        for i in range(n):
            resp = self.client.create(
                f"Pattern item {i}: The user always deploys on Fridays at 5pm.",
                tags=["proactive-test", "pattern"],
                agent_id=agent_id,
            )
            pattern_cids.append(resp.get("cid", ""))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Search for the pattern — check by CID
        resp = self.client.search(
            "user deploys on Fridays", limit=20, require_tags=["proactive-test"]
        )
        result_cids = {r.get("cid", "") for r in resp.get("results", [])}
        pattern_hits = sum(1 for cid in pattern_cids if cid in result_cids)

        # Also test recall (agent-scoped) — check by CID
        resp_recall = self.client.recall(agent_id, query="deploys on Fridays", limit=20)
        recall_cids = {r.get("cid", "") for r in resp_recall.get("results", [])}
        recall_hits = sum(1 for cid in pattern_cids if cid in recall_cids)

        return [{
            "operation": "pattern_detection",
            "items_created": n,
            "search_hits": pattern_hits,
            "recall_hits": recall_hits,
            "search_recall_rate": round(pattern_hits / n, 3) if n else 0,
        }]
