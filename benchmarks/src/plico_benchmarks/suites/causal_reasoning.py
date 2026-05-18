"""Causal reasoning suite — Axiom 8: Causality before Correlation.

Tests Plico's CausalGraph integration in retrieval:
- Entries with causal relationships should be linked
- Causal chains should be traversable via KG
- Search should surface causally-related content

Note: CausalGraph is built internally during memory ingest (fact extraction).
This suite tests the observable behavior, not internal state.
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.metrics import accuracy_pct
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase

# Reader prompt: synthesize answer from retrieved context instead of using raw snippets
CAUSAL_READER_PROMPT = """Answer the causal question using ONLY the context below.

Context:
{context}

Question: {question}

Rules:
- Extract the causal relationship from the context
- If the context describes a cause, identify the effect
- If the context describes an effect, identify the cause
- Be concise — one sentence answer
- Only say "I don't know" if truly no causal information exists in the context"""


class CausalReasoningSuite(SuiteBase):
    name = "causal-reasoning"
    description = "Causal graph reasoning — chain traversal and causal retrieval"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 30
        results = []

        # Phase 1: Build causal knowledge base via KG
        results.extend(self._test_causal_chain_construction(n))

        # Phase 2: Causal path traversal
        results.extend(self._test_causal_path_traversal(n))

        # Phase 3: Causal retrieval — search should find causally linked items
        results.extend(self._test_causal_retrieval())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_memory_competitors, get_cross_benchmarks

        locomo = get_memory_competitors("locomo")
        longmemeval = get_memory_competitors("longmemeval")
        cross = get_cross_benchmarks()

        # Extract temporal/multi-hop competitors as causal reasoning proxies
        temporal_comps = []
        for c in locomo:
            if c.get("temporal") is not None:
                temporal_comps.append({"name": c["name"], "temporal": c["temporal"], "multi_hop": c.get("multi_hop"), "source": "LoCoMo"})
        for c in longmemeval:
            if c.get("temporal_reasoning") is not None:
                temporal_comps.append({"name": c["name"], "temporal_reasoning": c["temporal_reasoning"], "source": "LongMemEval"})

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "temporal_multi_hop": temporal_comps,
                "hotpotqa": cross.get("hotpotqa", {}).get("baselines", []),
                "bigbench_hard": cross.get("bigbench_hard", {}).get("baselines", []),
                "note": "Causal reasoning has no direct benchmark. Temporal/multi-hop scores are proxies.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _test_causal_chain_construction(self, n: int) -> list[dict[str, Any]]:
        """Build a causal chain: A --Causes--> B --Causes--> C --Causes--> D ..."""
        # Create hub node for the causal chain
        hub = self.client.add_node("causal-hub", node_type="Event", agent_id="causal-test")
        hub_id = hub.get("node_id", "causal-hub")

        nodes = []
        latencies = []
        for i in range(n):
            label = f"cause-event-{i}"
            t0 = time.perf_counter()
            resp = self.client.add_node(label, node_type="Event", agent_id="causal-test")
            latencies.append((time.perf_counter() - t0) * 1000)
            node_id = resp.get("node_id", f"e{i}")
            nodes.append(node_id)
            # Connect: hub -> node (related)
            self.client.add_edge(hub_id, node_id, edge_type="CausedBy", agent_id="causal-test")
            # Connect chain: node[i-1] -> node[i] (causal chain)
            if i > 0:
                self.client.add_edge(nodes[i - 1], node_id, edge_type="CausedBy", agent_id="causal-test")

        return [{
            "operation": "causal_chain_construction",
            "count": n,
            "avg_node_latency_ms": sum(latencies) / len(latencies) if latencies else 0,
        }]

    def _test_causal_path_traversal(self, n: int) -> list[dict[str, Any]]:
        """Test finding causal paths between events at various depths."""
        # Reuse the chain from construction test
        hub = self.client.add_node("causal-path-hub", node_type="Event", agent_id="causal-path")
        hub_id = hub.get("node_id", "hub")

        nodes = []
        for i in range(n):
            resp = self.client.add_node(f"path-event-{i}", node_type="Event", agent_id="causal-path")
            node_id = resp.get("node_id", f"pe{i}")
            nodes.append(node_id)
            self.client.add_edge(hub_id, node_id, edge_type="CausedBy", agent_id="causal-path")
            if i > 0:
                self.client.add_edge(nodes[i - 1], node_id, edge_type="CausedBy", agent_id="causal-path")

        # Test path finding at various depths
        results = []
        for depth in [2, 3, 4, min(n, 10)]:
            if depth > len(nodes):
                continue
            src, dst = nodes[0], nodes[min(depth - 1, len(nodes) - 1)]
            t0 = time.perf_counter()
            resp = self.client.find_paths(src, dst, max_depth=depth, agent_id="causal-path")
            latency = (time.perf_counter() - t0) * 1000
            paths = resp.get("paths", [])
            results.append({
                "operation": f"causal_path_depth_{depth}",
                "paths_found": len(paths),
                "latency_ms": latency,
            })

        return results

    def _test_causal_retrieval(self) -> list[dict[str, Any]]:
        """Test that search retrieves causally-related content.

        Create memories with causal language, then verify that searching
        for a cause finds the effect and vice versa.
        Uses Reader mode: LLM synthesizes answer from retrieved context.
        """
        agent_id = "causal-retrieval"

        # Create causally-linked memories
        causal_pairs = [
            ("The server crashed due to a memory leak in the connection pool",
             "After the memory leak was fixed, the server stability improved to 99.9% uptime"),
            ("High CPU usage caused by inefficient sorting algorithm",
             "Replacing bubble sort with quicksort reduced CPU usage by 80%"),
            ("Database deadlock occurred when two transactions accessed the same rows",
             "Implementing row-level locking resolved the deadlock issue"),
            ("Network timeout caused by DNS resolution failure",
             "Switching to a local DNS cache eliminated the timeout problem"),
            ("Memory overflow triggered by unbounded cache growth",
             "Adding LRU eviction policy kept memory usage stable at 2GB"),
        ]

        cids = []
        for cause, effect in causal_pairs:
            resp_c = self.client.create(cause, tags=["causal", "cause"], agent_id=agent_id)
            resp_e = self.client.create(effect, tags=["causal", "effect"], agent_id=agent_id)
            cids.append((resp_c.get("cid", ""), resp_e.get("cid", "")))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Search for cause — should find effect in results
        cause_finds_effect = 0
        effect_finds_cause = 0
        llm_scores = []
        for i, (cause_text, effect_text) in enumerate(causal_pairs):
            cause_cid, effect_cid = cids[i]

            # Search using cause text — should find effect
            resp = self.client.search(cause_text, agent_id=agent_id, limit=10)
            results = resp.get("results", [])
            result_cids = {h.get("cid", "") for h in results}
            if effect_cid and effect_cid in result_cids:
                cause_finds_effect += 1
            # Reader mode: synthesize answer from context
            context = "\n".join(h.get("snippet", "") for h in results[:5])
            if context.strip():
                prompt = CAUSAL_READER_PROMPT.format(context=context, question=f"What is the effect of: {cause_text}")
                answer = self.judge.llm.chat([{"role": "user", "content": prompt}])
                score, _ = self.judge.evaluate_scored(cause_text, effect_text, answer)
                llm_scores.append(score)

            # Search using effect text — should find cause
            resp = self.client.search(effect_text, agent_id=agent_id, limit=10)
            results = resp.get("results", [])
            result_cids = {h.get("cid", "") for h in results}
            if cause_cid and cause_cid in result_cids:
                effect_finds_cause += 1
            # Reader mode: synthesize answer from context
            context = "\n".join(h.get("snippet", "") for h in results[:5])
            if context.strip():
                prompt = CAUSAL_READER_PROMPT.format(context=context, question=f"What caused: {effect_text}")
                answer = self.judge.llm.chat([{"role": "user", "content": prompt}])
                score, _ = self.judge.evaluate_scored(effect_text, cause_text, answer)
                llm_scores.append(score)

        total = len(causal_pairs)
        return [{
            "operation": "causal_retrieval",
            "count": total,
            "cause_finds_effect_rate": cause_finds_effect / total if total else 0,
            "effect_finds_cause_rate": effect_finds_cause / total if total else 0,
            "bidirectional_rate": (cause_finds_effect + effect_finds_cause) / (2 * total) if total else 0,
            "accuracy_pct": accuracy_pct(llm_scores),
        }]
