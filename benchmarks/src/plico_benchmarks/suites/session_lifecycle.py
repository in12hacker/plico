"""Session lifecycle suite — Axiom 10: Session is First-Class Citizen.

Tests Plico's session management:
- StartSession/EndSession lifecycle
- Session-scoped memory access
- Cross-session memory persistence
- Warm context assembly (delta between sessions)
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase


class SessionLifecycleSuite(SuiteBase):
    name = "session-lifecycle"
    description = "Session lifecycle — start/end, cross-session delta, warm context"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 20
        results = []

        # Phase 1: Basic session lifecycle
        results.extend(self._test_session_lifecycle(n))

        # Phase 2: Cross-session memory persistence
        results.extend(self._test_cross_session_memory(n))

        # Phase 3: Session-scoped vs persistent memory
        results.extend(self._test_session_vs_persistent())

        # Phase 4: Warm context assembly (delta between sessions)
        results.extend(self._test_warm_context_delta())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {k: v for k, v in r.items() if k != "operation"}
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import get_agent_frameworks, get_memory_competitors

        frameworks = get_agent_frameworks()
        locomo = get_memory_competitors("locomo")

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "agent_frameworks": [
                    {"name": f["name"], "session_mgmt": f.get("session_mgmt", "none"), "memory_layers": f.get("memory_layers", 0)}
                    for f in frameworks
                ],
                "locomo": [{"name": c["name"], "overall": c.get("overall"), "temporal": c.get("temporal")} for c in locomo],
                "note": "Cross-session memory persistence measured by LoCoMo. Most frameworks lack native session management.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def _test_session_lifecycle(self, n: int) -> list[dict[str, Any]]:
        """Test StartSession → create memories → EndSession lifecycle."""
        agent_id = "session-lifecycle-test"
        session_latencies = []
        end_latencies = []
        errors = 0

        for i in range(n):
            # Start session
            t0 = time.perf_counter()
            resp = self.client.start_session(agent_id, goals=[f"Session {i} goal"])
            session_latencies.append((time.perf_counter() - t0) * 1000)
            started = resp.get("session_started", {})
            session_id = started.get("session_id", "") if isinstance(started, dict) else ""
            if not session_id:
                errors += 1
                continue

            # Create session-scoped memories
            for j in range(5):
                self.client.create(
                    f"Session {i} memory {j}: working on task {i}",
                    tags=["session-test", f"session-{i}"],
                    agent_id=agent_id,
                )

            # End session
            t0 = time.perf_counter()
            self.client.end_session(agent_id, session_id=session_id)
            end_latencies.append((time.perf_counter() - t0) * 1000)

        return [{
            "operation": "session_lifecycle",
            "count": n,
            "success_rate": (n - errors) / n if n else 0,
            "avg_start_latency_ms": sum(session_latencies) / len(session_latencies) if session_latencies else 0,
            "avg_end_latency_ms": sum(end_latencies) / len(end_latencies) if end_latencies else 0,
        }]

    def _test_cross_session_memory(self, n: int) -> list[dict[str, Any]]:
        """Test that memories persist across sessions (Axiom 9+10)."""
        agent_id = "cross-session-test"

        # Session 1: create memories
        resp = self.client.start_session(agent_id, goals=["Create initial knowledge"])
        sid1 = resp.get("session_started", {}).get("session_id", "")
        session1_cids = []
        for i in range(10):
            resp = self.client.create(
                f"Cross-session fact {i}: important knowledge from session 1",
                tags=["cross-session"],
                agent_id=agent_id,
            )
            session1_cids.append((i, resp.get("cid", "")))
        self.client.end_session(agent_id, session_id=sid1)

        # Wait for indexing
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Session 2: verify session 1 memories are accessible
        resp = self.client.start_session(agent_id, goals=["Verify previous knowledge"])
        sid2 = resp.get("session_started", {}).get("session_id", "")
        hit_count = 0
        for i, cid in session1_cids:
            resp = self.client.search(f"Cross-session fact {i}", agent_id=agent_id, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                hit_count += 1

        # Also test recall (agent-scoped, should work across sessions)
        recall_hits = 0
        for i, cid in session1_cids[:5]:
            resp = self.client.recall(agent_id, query=f"Cross-session fact {i}", limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid in found_cids:
                recall_hits += 1

        self.client.end_session(agent_id, session_id=sid2)

        return [{
            "operation": "cross_session_memory",
            "count": len(session1_cids),
            "search_persistence_rate": hit_count / len(session1_cids) if session1_cids else 0,
            "recall_persistence_rate": recall_hits / min(5, len(session1_cids)) if session1_cids else 0,
        }]

    def _test_session_vs_persistent(self) -> list[dict[str, Any]]:
        """Test that remember() (working) and remember_long_term() persist across sessions."""
        agent_id = "session-persist-test"

        # Create working and long-term memories
        resp = self.client.start_session(agent_id)
        sid1 = resp.get("session_started", {}).get("session_id", "")
        for i in range(5):
            self.client.remember(agent_id, f"Working memory {i}: temporary context")
        for i in range(5):
            self.client.remember_long_term(agent_id, f"Long-term fact {i}: permanent knowledge", importance=8)
        self.client.end_session(agent_id, session_id=sid1)

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # New session: verify both types are accessible
        resp = self.client.start_session(agent_id)
        sid2 = resp.get("session_started", {}).get("session_id", "")
        working_hits = 0
        for i in range(5):
            resp = self.client.recall(agent_id, query=f"Working memory {i}", limit=5)
            if any("Working memory" in h.get("snippet", "") for h in resp.get("results", [])):
                working_hits += 1

        longterm_hits = 0
        for i in range(5):
            resp = self.client.recall(agent_id, query=f"Long-term fact {i}", limit=5)
            if any("Long-term fact" in h.get("snippet", "") for h in resp.get("results", [])):
                longterm_hits += 1
        self.client.end_session(agent_id, session_id=sid2)

        return [{
            "operation": "session_vs_persistent",
            "count": 5,
            "working_memory_persistence": working_hits / 5,
            "longterm_memory_persistence": longterm_hits / 5,
        }]

    def _test_warm_context_delta(self) -> list[dict[str, Any]]:
        """Test warm context assembly — what carries over between sessions.

        Measures:
        - How many items from session N appear in session N+1's context
        - Token cost of the delta
        - Latency of context assembly
        """
        agent_id = "warm-context-test"

        # Session 1: create memories with distinct content
        resp = self.client.start_session(agent_id, goals=["Build knowledge base"])
        sid1 = resp.get("session_started", {}).get("session_id", "")
        session1_items = []
        for i in range(10):
            resp = self.client.create(
                f"Warm context item {i}: unique knowledge about topic {i}",
                tags=["warm-context", f"topic-{i}"],
                agent_id=agent_id,
            )
            session1_items.append(resp.get("cid", ""))
        self.client.end_session(agent_id, session_id=sid1)

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Session 2: measure what context is assembled
        t0 = time.perf_counter()
        resp = self.client.start_session(agent_id, goals=["Continue from previous session"])
        sid2 = resp.get("session_started", {}).get("session_id", "")
        assembly_latency = (time.perf_counter() - t0) * 1000

        # Search for session 1 items to verify they're in context
        hits = 0
        for i in range(10):
            resp = self.client.search(f"Warm context item {i}", agent_id=agent_id, limit=5)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if session1_items[i] in found_cids:
                hits += 1

        # Create new items in session 2
        for i in range(5):
            self.client.create(
                f"Session 2 item {i}: new knowledge from continuation",
                tags=["warm-context", "session-2"],
                agent_id=agent_id,
            )

        self.client.end_session(agent_id, session_id=sid2)

        return [{
            "operation": "warm_context_delta",
            "session1_items": len(session1_items),
            "cross_session_hit_rate": hits / len(session1_items) if session1_items else 0,
            "assembly_latency_ms": assembly_latency,
        }]
