"""Intent routing suite — Axiom 2: Intent Before Action.

Tests Plico's intent-aware retrieval routing:
- Intent classification accuracy (rule-based and LLM)
- Intent-specific retrieval strategy effectiveness
- Context assembly quality with vs without intent hints

Each intent type routes to a different retrieval strategy:
- Factual: BM25+vector, no KG, top_k=15
- Temporal: BM25+vector+KG, time-decay boost, episodic type
- MultiHop: BM25+vector+KG+PPR, top_k=30
- Preference: semantic-type top-k
- Aggregation: broad recall + dedup
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.metrics import accuracy_pct
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


# Intent types and their expected retrieval behavior
INTENTS = ["factual", "temporal", "multi_hop", "preference", "aggregation"]

# Test queries designed to trigger specific intent routing
TEST_QUERIES = {
    "factual": [
        "What is the capital of France?",
        "Who invented the telephone?",
        "What is the boiling point of water?",
        "When was Python first released?",
        "What is the speed of light?",
    ],
    "temporal": [
        "When did the server crash last week?",
        "What happened yesterday at 3pm?",
        "What was discussed in the meeting on Monday?",
        "When did we deploy version 2.0?",
        "What changes were made last month?",
    ],
    "multi_hop": [
        "Why did the deployment fail after the database migration?",
        "What caused the performance regression in the search module?",
        "How did the code review feedback lead to the architecture change?",
        "What chain of events led to the outage?",
        "How are the authentication and authorization modules related?",
    ],
    "preference": [
        "What IDE does the user prefer?",
        "What coding style does the team follow?",
        "What is the preferred deployment strategy?",
        "What testing framework does the project use?",
        "What is the user's preferred communication style?",
    ],
    "aggregation": [
        "List all bugs reported this sprint",
        "Summarize all pull requests from last week",
        "What are all the configuration options?",
        "Show all memory entries related to deployment",
        "What are the common patterns in recent failures?",
    ],
}


class IntentRoutingSuite(SuiteBase):
    name = "intent-routing"
    description = "Intent-aware retrieval routing — Axiom 2: Intent Before Action"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        results = []

        # Phase 1: Seed knowledge base with intent-typed content
        self._seed_knowledge_base()

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 2: Test intent routing effectiveness
        results.extend(self._test_intent_routing())

        # Phase 3: Test with-intent vs without-intent comparison
        results.extend(self._test_intent_vs_no_intent())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_memory_competitors, get_ragas_baselines

        locomo = get_memory_competitors("locomo")
        longmemeval = get_memory_competitors("longmemeval")
        ragas = get_ragas_baselines()

        # Per-category competitors as intent-routing proxies
        intent_comps = []
        for c in locomo:
            intent_comps.append({
                "name": c["name"],
                "single_hop": c.get("single_hop"),
                "multi_hop": c.get("multi_hop"),
                "temporal": c.get("temporal"),
                "open_domain": c.get("open_domain"),
                "source": "LoCoMo",
            })

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "intent_category_scores": intent_comps,
                "ragas": ragas.get("metrics", []),
                "note": "No direct intent-routing benchmark. LoCoMo per-category scores show how different query types perform. RAGAS Context Precision/Recall are affected by intent routing quality.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _seed_knowledge_base(self) -> None:
        """Create knowledge items with intent-typed content. Track CIDs for validation."""
        self._intent_cids: dict[str, set[str]] = {intent: set() for intent in INTENTS}

        # Factual items
        for i in range(20):
            resp = self.client.create(
                f"Fact {i}: The Earth orbits the Sun at approximately 149.6 million km.",
                tags=["intent-test", "factual"],
            )
            cid = resp.get("cid", "")
            if cid:
                self._intent_cids["factual"].add(cid)

        # Temporal items with date context
        for i in range(20):
            resp = self.client.create(
                f"[Date: 2026-05-{10+i}] Event {i}: Server was restarted due to memory pressure.",
                tags=["intent-test", "temporal"],
            )
            cid = resp.get("cid", "")
            if cid:
                self._intent_cids["temporal"].add(cid)

        # Multi-hop items (causal chains)
        for i in range(15):
            resp = self.client.create(
                f"Step {i}: The deployment triggered a cascade starting with cache invalidation, "
                f"then database connection pool exhaustion, finally leading to 503 errors.",
                tags=["intent-test", "multi_hop"],
            )
            cid = resp.get("cid", "")
            if cid:
                self._intent_cids["multi_hop"].add(cid)

        # Preference items
        for i in range(15):
            resp = self.client.create(
                f"Preference {i}: The user strongly prefers dark mode, vim keybindings, "
                f"and Python with type hints.",
                tags=["intent-test", "preference"],
            )
            cid = resp.get("cid", "")
            if cid:
                self._intent_cids["preference"].add(cid)

        # Aggregation items
        for i in range(20):
            resp = self.client.create(
                f"Bug report {i}: Issue #{100+i} - intermittent test failure in CI pipeline.",
                tags=["intent-test", "aggregation", f"bug-{i}"],
            )
            cid = resp.get("cid", "")
            if cid:
                self._intent_cids["aggregation"].add(cid)

    def _test_intent_routing(self) -> list[dict[str, Any]]:
        """Test search with explicit intent hints for each intent type."""
        results = []

        for intent, queries in TEST_QUERIES.items():
            hits = 0
            total = 0
            latencies = []
            llm_scores = []
            intent_cids = self._intent_cids.get(intent, set())

            for query in queries:
                t0 = time.perf_counter()
                resp = self.client.search(
                    query, limit=10, intent=intent, require_tags=["intent-test"]
                )
                latencies.append((time.perf_counter() - t0) * 1000)

                # Count results that belong to the correct intent type (by CID)
                top_snippet = ""
                for r in resp.get("results", []):
                    total += 1
                    cid = r.get("cid", "")
                    if cid in intent_cids:
                        hits += 1
                    if not top_snippet:
                        top_snippet = r.get("snippet", "")

                # LLM-as-judge: does top result contain intent-relevant content?
                if top_snippet:
                    expected = f"Content tagged as {intent} intent"
                    score, _ = self.judge.evaluate_scored(query, expected, top_snippet)
                    llm_scores.append(score)

            results.append({
                "operation": f"intent_{intent}",
                "query_count": len(queries),
                "hit_rate": round(hits / total, 3) if total else 0,
                "accuracy_pct": accuracy_pct(llm_scores),
                "avg_latency_ms": round(sum(latencies) / len(latencies), 2) if latencies else 0,
            })

        return results

    def _test_intent_vs_no_intent(self) -> list[dict[str, Any]]:
        """Compare retrieval quality with intent hint vs without."""
        results = []

        for intent, queries in TEST_QUERIES.items():
            with_intent_hits = 0
            without_intent_hits = 0
            sample_queries = queries[:3]  # Sample for efficiency
            intent_cids = self._intent_cids.get(intent, set())

            for query in sample_queries:
                # With intent
                resp_with = self.client.search(
                    query, limit=10, intent=intent, require_tags=["intent-test"]
                )
                # Without intent
                resp_without = self.client.search(
                    query, limit=10, require_tags=["intent-test"]
                )

                # Count relevant results by CID
                for r in resp_with.get("results", []):
                    if r.get("cid", "") in intent_cids:
                        with_intent_hits += 1
                for r in resp_without.get("results", []):
                    if r.get("cid", "") in intent_cids:
                        without_intent_hits += 1

            total = len(sample_queries) * 10  # 10 results per query
            improvement = (
                (with_intent_hits - without_intent_hits) / without_intent_hits * 100
                if without_intent_hits > 0 else 0
            )

            results.append({
                "operation": f"intent_vs_no_intent_{intent}",
                "with_intent_hits": with_intent_hits,
                "without_intent_hits": without_intent_hits,
                "improvement_pct": round(improvement, 1),
            })

        return results
