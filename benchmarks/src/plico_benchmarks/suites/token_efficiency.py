"""Token efficiency suite — L0/L1/L2 context layering savings.

Measures Plico's token efficiency against competitors (Axiom 1: Token is Scarcest).
Tests how much context is returned per query vs full-context baselines.

Competitor baselines (from Memori Labs, hardcoded):
- Full-Context: 26,031 tokens/query (100%)
- Memori: 1,294 tokens/query (4.97%)
- Zep: 3,911 tokens/query (15.02%)
- Mem0: 1,764 tokens/query (6.78%)
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.metrics import estimate_tokens
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


# Hardcoded competitor baselines (tokens per query)
COMPETITOR_TOKENS = {
    "full_context": 26031,
    "memori": 1294,
    "zep": 3911,
    "mem0": 1764,
}


class TokenEfficiencySuite(SuiteBase):
    name = "token-efficiency"
    description = "Token efficiency — context layering vs competitors"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        n_queries = self.samples or 50
        results = []

        # Phase 1: Create a knowledge base with varying content sizes
        cids = self._seed_knowledge_base()

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 2: Measure search context at different limits (simulating L0/L1/L2)
        results.extend(self._measure_context_levels(n_queries, cids))

        # Phase 3: Measure recall (agent-scoped, uses layered context)
        results.extend(self._measure_recall_efficiency(n_queries))

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_token_efficiency_competitors

        competitors = get_token_efficiency_competitors()

        # Compute cost for Plico's best level (L0 or L1)
        # Using GPT-4o pricing: ~$0.0008/1K tokens (blended input/output)
        COST_PER_1K_TOKENS = 0.0008
        overall = metrics.get("overall", {})
        plico_costs = {}
        for level in ("context_l0", "context_l1", "context_l2", "recall_efficiency"):
            if level in overall:
                tokens = overall[level].get("avg_tokens_per_query", 0)
                plico_costs[level] = {
                    "tokens": tokens,
                    "cost_per_query_usd": round(tokens * COST_PER_1K_TOKENS / 1000, 6),
                    "context_footprint_pct": round(tokens / COMPETITOR_TOKENS["full_context"] * 100, 2),
                }

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "token_efficiency": competitors,
            },
            "costs": plico_costs,
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _seed_knowledge_base(self) -> list[str]:
        """Create knowledge items of varying sizes."""
        cids = []
        # Short items (~50 tokens)
        for i in range(30):
            content = f"Fact {i}: Machine learning is a subset of artificial intelligence that enables systems to learn from data."
            resp = self.client.create(content, tags=["token-test", "short"])
            cids.append(resp.get("cid", ""))

        # Medium items (~200 tokens)
        for i in range(20):
            content = (
                f"Document {i}: Artificial intelligence has transformed numerous industries. "
                "Machine learning algorithms can now process vast amounts of data to identify patterns "
                "and make predictions. Deep learning, a subset of machine learning, uses neural networks "
                "with multiple layers to model complex relationships. Natural language processing enables "
                "machines to understand and generate human language. Computer vision allows systems to "
                "interpret visual information from the world. Reinforcement learning trains agents through "
                "trial and error to maximize cumulative rewards. Transfer learning leverages pre-trained "
                "models to solve new tasks with limited data. Generative AI can create text, images, "
                "and code from natural language prompts."
            )
            resp = self.client.create(content, tags=["token-test", "medium"])
            cids.append(resp.get("cid", ""))

        # Long items (~500 tokens)
        for i in range(10):
            content = (
                f"Research paper {i}: The field of artificial intelligence has undergone remarkable "
                "transformations since its inception in the 1950s. Early symbolic AI systems relied on "
                "hand-crafted rules and logical reasoning. The expert systems boom of the 1980s demonstrated "
                "the potential of domain-specific knowledge bases. The connectionist revival of the late "
                "1980s brought neural networks back into focus. The statistical learning revolution of the "
                "1990s and 2000s introduced support vector machines, random forests, and other powerful algorithms. "
                "The deep learning revolution, sparked by AlexNet in 2012, transformed computer vision, "
                "natural language processing, and speech recognition. The transformer architecture, introduced "
                "in 2017, enabled the development of large language models like GPT, BERT, and their successors. "
                "The emergence of foundation models has created a paradigm shift where a single pre-trained model "
                "can be adapted to numerous downstream tasks. Multimodal models can process text, images, audio, "
                "and video simultaneously. The AI agent paradigm enables autonomous systems that can plan, reason, "
                "use tools, and interact with their environment. Retrieval-augmented generation combines the "
                "strengths of parametric and non-parametric knowledge. The current landscape includes both "
                "proprietary models from major tech companies and a vibrant open-source community."
            )
            resp = self.client.create(content, tags=["token-test", "long"])
            cids.append(resp.get("cid", ""))

        return cids

    def _measure_context_levels(self, n_queries: int, cids: list[str]) -> list[dict[str, Any]]:
        """Measure search context size at different result limits.

        L0-like: limit=3 (minimal context, ~100-300 tokens)
        L1-like: limit=5 (moderate context, ~300-1000 tokens)
        L2-like: limit=15 (full context, ~1000-3000 tokens)
        """
        queries = [
            "machine learning algorithms",
            "deep learning neural networks",
            "natural language processing",
            "computer vision applications",
            "reinforcement learning agents",
            "transformer architecture attention",
            "generative AI models",
            "retrieval augmented generation",
            "AI agent planning reasoning",
            "transfer learning fine-tuning",
        ] * (n_queries // 10 + 1)

        levels = {"L0": 3, "L1": 5, "L2": 15}
        level_stats: dict[str, list[int]] = {level: [] for level in levels}

        for i in range(n_queries):
            query = queries[i % len(queries)]
            for level, limit in levels.items():
                resp = self.client.search(query, limit=limit)
                snippets = [h.get("snippet", "") for h in resp.get("results", [])]
                total_chars = sum(len(s) for s in snippets)
                token_est = estimate_tokens("\n".join(snippets))
                level_stats[level].append(token_est)

        results = []
        for level, tokens_list in level_stats.items():
            avg_tokens = sum(tokens_list) / len(tokens_list) if tokens_list else 0
            results.append({
                "operation": f"context_{level.lower()}",
                "avg_tokens_per_query": round(avg_tokens, 1),
                "pct_of_full_context": round(avg_tokens / COMPETITOR_TOKENS["full_context"] * 100, 2),
                "vs_memori_ratio": round(avg_tokens / COMPETITOR_TOKENS["memori"], 2) if COMPETITOR_TOKENS["memori"] else 0,
            })

        return results

    def _measure_recall_efficiency(self, n_queries: int) -> list[dict[str, Any]]:
        """Measure recall token efficiency — agent-scoped memory recall."""
        agent_id = "token-eff-agent"
        # Seed agent memory
        for i in range(20):
            self.client.remember(agent_id, f"Agent fact {i}: the user is working on AI safety research")

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        total_tokens = 0
        queries = ["user research interests", "AI safety", "agent facts"] * (n_queries // 3 + 1)
        for i in range(n_queries):
            q = queries[i % len(queries)]
            resp = self.client.recall(agent_id, query=q, limit=5)
            snippets = [h.get("snippet", "") for h in resp.get("results", [])]
            total_tokens += estimate_tokens("\n".join(snippets))

        avg_tokens = total_tokens / n_queries if n_queries else 0
        return [{
            "operation": "recall_efficiency",
            "avg_tokens_per_query": round(avg_tokens, 1),
            "pct_of_full_context": round(avg_tokens / COMPETITOR_TOKENS["full_context"] * 100, 2),
            "vs_memori_ratio": round(avg_tokens / COMPETITOR_TOKENS["memori"], 2) if COMPETITOR_TOKENS["memori"] else 0,
        }]
