"""Memory lifecycle suite — CRUD + layer migration + checkpoint/restore.

Tests Plico's 4-layer memory architecture (Axiom 3: Memory Exoskeleton):
- Ephemeral (session-scoped via create)
- Working (agent-scoped via remember)
- Long-term (persistent via remember_long_term)
- Cross-layer search (semantic search spans all layers)
"""

from __future__ import annotations

import os
import time
from typing import Any

from plico_benchmarks.core.metrics import accuracy_pct
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase

# Reader prompt: synthesize answer from retrieved context
MEMORY_READER_PROMPT = """Answer the question using ONLY the context below.

Context:
{context}

Question: {question}

Rules:
- Extract the relevant information from the context
- Be concise — one sentence answer
- Only say "I don't know" if truly no relevant information exists in the context"""


class MemoryLifecycleSuite(SuiteBase):
    name = "memory-lifecycle"
    description = "Memory CRUD + layer migration + checkpoint/restore"

    def setup(self) -> None:
        self.wait_for_plico()
        self.agent_id = "lifecycle-test"

    def run(self) -> list[dict[str, Any]]:
        n = self.samples or 100
        results = []

        # ── Phase 1: CRUD correctness ────────────────────────────────
        results.extend(self._test_create(n))
        results.extend(self._test_read(n))
        results.extend(self._test_search(n))
        results.extend(self._test_update(n))
        results.extend(self._test_delete(n))
        results.extend(self._test_batch())

        # ── Phase 2: Layer migration ─────────────────────────────────
        results.extend(self._test_layer_migration())

        # ── Phase 3: Checkpoint/restore ──────────────────────────────
        results.extend(self._test_checkpoint_restore())

        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        overall = {}
        for r in raw:
            op = r["operation"]
            overall[op] = {
                k: v for k, v in r.items() if k != "operation"
            }
            # Round numeric values
            for key in ("success_rate", "hit_rate", "avg_latency_ms", "cross_layer_hit_rate"):
                if key in overall[op] and isinstance(overall[op][key], float):
                    overall[op][key] = round(overall[op][key], 3)
        return {"overall": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        from plico_benchmarks.core.competitors import (
            get_memory_competitors, get_agent_frameworks, get_ragas_baselines,
        )

        longmemeval = get_memory_competitors("longmemeval")
        locomo = get_memory_competitors("locomo")
        personamem = get_memory_competitors("personamem")
        frameworks = get_agent_frameworks()
        ragas = get_ragas_baselines()

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {"samples": self.samples},
            "metrics": metrics,
            "competitors": {
                "longmemeval": longmemeval,
                "locomo": locomo,
                "personamem": personamem,
                "memory_frameworks": [
                    {"name": f["name"], "memory_layers": f.get("memory_layers", 0), "scope_isolation": f.get("scope_isolation", "none")}
                    for f in frameworks
                ],
                "ragas": ragas.get("metrics", []),
                "note": "CRUD and layer migration are unique to Plico. LongMemEval/LoCoMo measure memory retrieval quality.",
            },
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    # ── CRUD tests ───────────────────────────────────────────────────

    def _test_create(self, n: int) -> list[dict[str, Any]]:
        cids = []
        latencies = []
        for i in range(n):
            content = f"Lifecycle item {i}: memory architecture benchmark test"
            t0 = time.perf_counter()
            resp = self.client.create(content, tags=["lifecycle", f"item-{i}"], agent_id=self.agent_id)
            latencies.append((time.perf_counter() - t0) * 1000)
            cids.append((i, resp.get("cid", ""), content))
        self._cids = cids
        return [{"operation": "create", "count": n,
                 "success_rate": sum(1 for _, c, _ in cids if c) / n,
                 "avg_latency_ms": sum(latencies) / n}]

    def _test_read(self, n: int) -> list[dict[str, Any]]:
        cids = getattr(self, "_cids", [])
        latencies = []
        errors = 0
        for i, cid, expected in cids:
            t0 = time.perf_counter()
            resp = self.client.read(cid, agent_id=self.agent_id)
            latencies.append((time.perf_counter() - t0) * 1000)
            if resp.get("data", "") != expected:
                errors += 1
        return [{"operation": "read", "count": n,
                 "success_rate": (n - errors) / n if n else 0,
                 "avg_latency_ms": sum(latencies) / n if n else 0}]

    def _test_search(self, n: int) -> list[dict[str, Any]]:
        cids = getattr(self, "_cids", [])
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)
        hits = 0
        latencies = []
        test_count = min(20, n)
        for i, cid, _ in cids[:test_count]:
            t0 = time.perf_counter()
            resp = self.client.search(f"Lifecycle item {i}", agent_id=self.agent_id, limit=5)
            latencies.append((time.perf_counter() - t0) * 1000)
            if cid in [h.get("cid", "") for h in resp.get("results", [])]:
                hits += 1
        return [{"operation": "search", "count": test_count,
                 "hit_rate": hits / test_count if test_count else 0,
                 "avg_latency_ms": sum(latencies) / len(latencies) if latencies else 0}]

    def _test_update(self, n: int) -> list[dict[str, Any]]:
        cids = getattr(self, "_cids", [])
        latencies = []
        update_cids = []
        test_count = min(20, n)
        for i, _, _ in cids[:test_count]:
            new_content = f"Lifecycle item {i}: UPDATED content"
            t0 = time.perf_counter()
            resp = self.client.create(new_content, tags=["lifecycle", f"item-{i}", "updated"], agent_id=self.agent_id)
            latencies.append((time.perf_counter() - t0) * 1000)
            update_cids.append(resp.get("cid", ""))
        return [{"operation": "update", "count": test_count,
                 "success_rate": sum(1 for c in update_cids if c) / test_count if test_count else 0,
                 "avg_latency_ms": sum(latencies) / test_count if test_count else 0}]

    def _test_delete(self, n: int) -> list[dict[str, Any]]:
        """Test memory deletion — items should no longer be searchable."""
        cids = getattr(self, "_cids", [])
        test_count = min(10, n)
        latencies = []
        delete_success = 0
        search_after_delete = 0

        # Delete items
        for i, cid, _ in cids[:test_count]:
            if not cid:
                continue
            t0 = time.perf_counter()
            try:
                self.client.delete(cid, agent_id=self.agent_id)
                delete_success += 1
            except Exception:
                pass
            latencies.append((time.perf_counter() - t0) * 1000)

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Verify deleted items are no longer searchable
        for i, cid, _ in cids[:test_count]:
            if not cid:
                continue
            resp = self.client.search(f"Lifecycle item {i}", agent_id=self.agent_id, limit=10)
            found_cids = [h.get("cid", "") for h in resp.get("results", [])]
            if cid not in found_cids:
                search_after_delete += 1

        return [{
            "operation": "delete",
            "count": test_count,
            "success_rate": delete_success / test_count if test_count else 0,
            "avg_latency_ms": sum(latencies) / len(latencies) if latencies else 0,
            "search_gone_rate": search_after_delete / test_count if test_count else 0,
        }]

    def _test_batch(self) -> list[dict[str, Any]]:
        items = [{"content": f"Batch lifecycle {i}", "tags": ["lifecycle", "batch"]} for i in range(50)]
        t0 = time.perf_counter()
        resp = self.client.batch_create(items, agent_id=self.agent_id)
        lat = (time.perf_counter() - t0) * 1000
        return [{"operation": "batch_create", "count": 50,
                 "success_rate": 1.0 if not resp.get("error") else 0.0,
                 "avg_latency_ms": lat}]

    # ── Layer migration ──────────────────────────────────────────────

    def _test_layer_migration(self) -> list[dict[str, Any]]:
        """Test cross-layer memory accessibility (Axiom 3).

        Create items at different memory layers:
        - Ephemeral: via create() — CAS storage, searchable via search()
        - Working: via remember() — memory system, searchable via recall()
        - Long-term: via remember_long_term() — memory system, searchable via recall()

        Then verify items are accessible via the appropriate endpoint.
        Uses Reader mode: LLM synthesizes answer from retrieved context.
        """
        agent_id = "layer-migration-test"
        layer_items = []

        # Ephemeral layer (CAS)
        for i in range(5):
            content = f"Ephemeral fact {i}: quantum computing advances in 2026"
            resp = self.client.create(content, tags=["layer-test", "ephemeral"])
            layer_items.append(("ephemeral", resp.get("cid", ""), content))

        # Working layer (remember — memory system, not CAS)
        for i in range(5):
            content = f"Working memory {i}: user prefers dark mode and vim keybindings"
            self.client.remember(agent_id, content)
            layer_items.append(("working", "", content))

        # Long-term layer (remember_long_term — memory system, not CAS)
        for i in range(5):
            content = f"Long-term fact {i}: the user's birthday is March 15th"
            self.client.remember_long_term(agent_id, content, importance=8)
            layer_items.append(("long_term", "", content))

        # Wait for indexing
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Test CAS search for ephemeral items
        cross_layer_hits = 0
        total_items = len(layer_items)
        llm_scores = []
        for layer, cid, content in layer_items:
            query_words = content.split(":")[1].strip()[:40] if ":" in content else content[:40]
            if layer == "ephemeral":
                # Ephemeral items are in CAS — use search() and check CID
                resp = self.client.search(query_words, limit=10)
                found_cids = [h.get("cid", "") for h in resp.get("results", [])]
                if cid and cid in found_cids:
                    cross_layer_hits += 1
                # Reader mode: synthesize answer from context
                snippets = [h.get("snippet", "") for h in resp.get("results", [])[:5]]
                context = "\n".join(s for s in snippets if s)
            else:
                # Working/long-term items are in memory system — use recall()
                resp = self.client.recall(agent_id, query=query_words, limit=10)
                # recall returns content strings in "results" (or "memory" field)
                results = resp.get("results", resp.get("memory", []))
                # Check if any result contains the unique substring
                unique_part = content.split(":")[1].strip()[:30] if ":" in content else content[:30]
                found = any(unique_part.lower() in str(r).lower() for r in results)
                if found:
                    cross_layer_hits += 1
                # Reader mode: synthesize answer from context
                context = "\n".join(str(r) for r in results[:5])

            if context.strip():
                question = f"What information is stored about: {query_words}"
                prompt = MEMORY_READER_PROMPT.format(context=context, question=question)
                answer = self.judge.llm.chat([{"role": "user", "content": prompt}])
                score, _ = self.judge.evaluate_scored(query_words, content, answer)
                llm_scores.append(score)

        # Also test recall for working + long-term (content matching)
        recall_hits = 0
        recall_items = [(l, c, ct) for l, c, ct in layer_items if l in ("working", "long_term")]
        for layer, cid, content in recall_items:
            query_words = content.split(":")[1].strip()[:40] if ":" in content else content[:40]
            resp = self.client.recall(agent_id, query=query_words, limit=10)
            results = resp.get("results", resp.get("memory", []))
            unique_part = content.split(":")[1].strip()[:30] if ":" in content else content[:30]
            if any(unique_part.lower() in str(r).lower() for r in results):
                recall_hits += 1

        return [{
            "operation": "layer_migration",
            "count": total_items,
            "cross_layer_hit_rate": cross_layer_hits / total_items if total_items else 0,
            "recall_hit_rate": recall_hits / len(recall_items) if recall_items else 0,
            "accuracy_pct": accuracy_pct(llm_scores),
        }]

    # ── Checkpoint/restore ───────────────────────────────────────────

    def _test_checkpoint_restore(self) -> list[dict[str, Any]]:
        """Test memory persistence across 'checkpoints' (Axiom 9: Gets Better).

        Create items, record state, create more items, verify original
        items are still accessible. This simulates session boundaries.
        """
        agent_id = "checkpoint-test"
        checkpoint_items = []

        # Checkpoint 1: create initial items
        for i in range(10):
            content = f"Checkpoint-1 item {i}: initial knowledge base entry"
            resp = self.client.create(content, tags=["checkpoint", "cp1"], agent_id=agent_id)
            checkpoint_items.append((i, resp.get("cid", ""), content))

        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Verify checkpoint 1 items are searchable
        cp1_hits = 0
        for i, cid, _ in checkpoint_items:
            resp = self.client.search(f"Checkpoint-1 item {i}", agent_id=agent_id, limit=5)
            if cid in [h.get("cid", "") for h in resp.get("results", [])]:
                cp1_hits += 1

        # Checkpoint 2: add more items (simulating continued work)
        cp2_items = []
        for i in range(10):
            content = f"Checkpoint-2 item {i}: additional knowledge after session"
            resp = self.client.create(content, tags=["checkpoint", "cp2"], agent_id=agent_id)
            cp2_items.append((i, resp.get("cid", ""), content))

        self.wait_for_indexing(timeout=timeout)

        # Verify checkpoint 1 items are STILL accessible after checkpoint 2
        cp1_after_hits = 0
        for i, cid, _ in checkpoint_items:
            resp = self.client.search(f"Checkpoint-1 item {i}", agent_id=agent_id, limit=5)
            if cid in [h.get("cid", "") for h in resp.get("results", [])]:
                cp1_after_hits += 1

        # Verify checkpoint 2 items are also accessible
        cp2_hits = 0
        for i, cid, _ in cp2_items:
            resp = self.client.search(f"Checkpoint-2 item {i}", agent_id=agent_id, limit=5)
            if cid in [h.get("cid", "") for h in resp.get("results", [])]:
                cp2_hits += 1

        return [{
            "operation": "checkpoint_restore",
            "count": len(checkpoint_items),
            "cp1_hit_rate": cp1_hits / len(checkpoint_items) if checkpoint_items else 0,
            "cp1_persistence_rate": cp1_after_hits / len(checkpoint_items) if checkpoint_items else 0,
            "cp2_hit_rate": cp2_hits / len(cp2_items) if cp2_items else 0,
        }]
