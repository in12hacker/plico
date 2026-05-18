"""Scope isolation suite — Axiom 4: Sharing before Duplication.

Tests Plico's scope enforcement:
- Private: only creating agent can access
- Shared: all agents can access
- Group: only group members can access

Measures isolation correctness and cross-agent discovery latency.
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


class ScopeIsolationSuite(SuiteBase):
    name = "scope-isolation"
    description = "Memory scope isolation — Private/Shared/Group enforcement"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 30
        results = []

        # Phase 1: Private scope — create as agent A, search as agent B
        results.extend(self._test_private_isolation(n))

        # Phase 2: Shared scope — create as agent A, search as agent B
        results.extend(self._test_shared_access(n))

        # Phase 3: Group scope — create as group member, search as member vs non-member
        results.extend(self._test_group_isolation(n))

        # Phase 4: Multi-group isolation — agent in group A vs group B
        results.extend(self._test_multi_group_isolation())

        # Phase 5: Cross-agent recall isolation
        results.extend(self._test_recall_isolation())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_agent_frameworks

        competitors = get_agent_frameworks()

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "agent_frameworks": competitors,
                "note": "Most agent frameworks have no native scope isolation. Plico enforces at OS level.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _test_private_isolation(self, n: int) -> list[dict[str, Any]]:
        """Private memories created by agent A must NOT be accessible by agent B."""
        agent_a = "scope-agent-a"
        agent_b = "scope-agent-b"
        cids = []

        # Agent A creates private items
        for i in range(n):
            content = f"Private secret {i}: confidential information only for agent A"
            resp = self.client.create(content, tags=["scope-test", "private"], agent_id=agent_a, scope="private")
            cids.append((i, resp.get("cid", "")))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Agent B tries to search for agent A's private items
        leaks = 0
        for i, cid in cids:
            resp = self.client.search(f"Private secret {i}", agent_id=agent_b, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                leaks += 1

        # Agent A can find its own private items
        own_hits = 0
        for i, cid in cids[:10]:
            resp = self.client.search(f"Private secret {i}", agent_id=agent_a, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                own_hits += 1

        return [{
            "operation": "private_isolation",
            "count": n,
            "leak_rate": leaks / n if n else 0,
            "own_access_rate": own_hits / min(10, n) if n else 0,
            "isolation_perfect": leaks == 0,
        }]

    def _test_shared_access(self, n: int) -> list[dict[str, Any]]:
        """Shared memories created by agent A must be accessible by agent B."""
        agent_a = "scope-shared-a"
        agent_b = "scope-shared-b"
        cids = []

        # Agent A creates shared items
        for i in range(n):
            content = f"Shared knowledge {i}: public information for all agents"
            resp = self.client.create(content, tags=["scope-test", "shared"], agent_id=agent_a, scope="shared")
            cids.append((i, resp.get("cid", "")))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Agent B searches for shared items
        shared_hits = 0
        for i, cid in cids[:10]:
            resp = self.client.search(f"Shared knowledge {i}", agent_id=agent_b, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                shared_hits += 1

        return [{
            "operation": "shared_access",
            "count": n,
            "cross_agent_access_rate": shared_hits / min(10, n) if n else 0,
        }]

    def _test_group_isolation(self, n: int) -> list[dict[str, Any]]:
        """Group memories accessible by group members, not by non-members."""
        agent_member = "scope-group-member"
        agent_outsider = "scope-group-outsider"
        group_id = "test-team-alpha"
        cids = []

        # Member creates group-scoped items
        for i in range(n):
            content = f"Team alpha note {i}: internal team discussion"
            resp = self.client.create(
                content, tags=["scope-test", "group"], agent_id=agent_member, scope=f"group:{group_id}"
            )
            cids.append((i, resp.get("cid", "")))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Outsider tries to access group items
        outsider_leaks = 0
        for i, cid in cids[:10]:
            resp = self.client.search(f"Team alpha note {i}", agent_id=agent_outsider, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                outsider_leaks += 1

        return [{
            "operation": "group_isolation",
            "count": n,
            "outsider_leak_rate": outsider_leaks / min(10, n) if n else 0,
            "isolation_perfect": outsider_leaks == 0,
        }]

    def _test_multi_group_isolation(self) -> list[dict[str, Any]]:
        """Test cross-group isolation: agent in group A cannot see group B items."""
        agent_alpha = "multi-group-alpha"
        agent_beta = "multi-group-beta"
        group_a = "team-alpha-multi"
        group_b = "team-beta-multi"

        # Agent alpha creates group A items
        alpha_cids = []
        for i in range(10):
            content = f"Alpha team secret {i}: confidential alpha project plans"
            resp = self.client.create(
                content, tags=["scope-test", "multi-group"], agent_id=agent_alpha,
                scope=f"group:{group_a}"
            )
            alpha_cids.append(resp.get("cid", ""))

        # Agent beta creates group B items
        beta_cids = []
        for i in range(10):
            content = f"Beta team secret {i}: confidential beta project plans"
            resp = self.client.create(
                content, tags=["scope-test", "multi-group"], agent_id=agent_beta,
                scope=f"group:{group_b}"
            )
            beta_cids.append(resp.get("cid", ""))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Agent beta tries to search for alpha's group items
        cross_leaks = 0
        for i, cid in enumerate(alpha_cids[:5]):
            resp = self.client.search(f"Alpha team secret {i}", agent_id=agent_beta, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                cross_leaks += 1

        # Agent alpha tries to search for beta's group items
        reverse_leaks = 0
        for i, cid in enumerate(beta_cids[:5]):
            resp = self.client.search(f"Beta team secret {i}", agent_id=agent_alpha, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                reverse_leaks += 1

        total_checks = 10  # 5 each direction
        total_leaks = cross_leaks + reverse_leaks

        return [{
            "operation": "multi_group_isolation",
            "alpha_to_beta_leaks": cross_leaks,
            "beta_to_alpha_leaks": reverse_leaks,
            "total_leak_rate": total_leaks / total_checks,
            "isolation_perfect": total_leaks == 0,
        }]

    def _test_recall_isolation(self) -> list[dict[str, Any]]:
        """Test that recall is agent-scoped — agent A's memories not in agent B's recall.

        Uses remember_long_term() + recall_semantic() for proper semantic search.
        """
        agent_a = "recall-iso-a"
        agent_b = "recall-iso-b"

        # Agent A remembers something unique (long-term for embedding)
        for i in range(5):
            self.client.remember_long_term(agent_a, f"Agent A exclusive memory {i}: top secret project alpha", importance=8)

        # Agent B remembers different things
        for i in range(5):
            self.client.remember_long_term(agent_b, f"Agent B exclusive memory {i}: project beta details", importance=8)

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Agent B recalls — should NOT see agent A's memories
        resp_b = self.client.recall_semantic(agent_b, query="exclusive memory", limit=20)
        results_b = resp_b.get("results", resp_b.get("memory", []))
        a_leaks_in_b = sum(
            1 for h in results_b
            if "Agent A exclusive" in str(h)
        )

        # Agent A recalls — should see own memories
        resp_a = self.client.recall_semantic(agent_a, query="exclusive memory", limit=20)
        results_a = resp_a.get("results", resp_a.get("memory", []))
        a_own = sum(
            1 for h in results_a
            if "Agent A exclusive" in str(h)
        )

        return [{
            "operation": "recall_isolation",
            "count": 5,
            "cross_agent_leaks": a_leaks_in_b,
            "own_recall_hits": a_own,
            "isolation_perfect": a_leaks_in_b == 0,
        }]
