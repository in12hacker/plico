"""Retrieval accuracy suite — BEIR + MemoryAgentBench AR."""

from __future__ import annotations

import time
from typing import Any

import numpy as np

from plico_benchmarks.suites.base import SuiteBase
from plico_benchmarks.core.metrics import recall_at_k
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.datasets.beir import BeirDataset
from plico_benchmarks.datasets.memoryagentbench import MABDataset


class RetrievalSuite(SuiteBase):
    name = "retrieval"
    description = "BEIR SciFact + MemoryAgentBench AR retrieval accuracy"

    def setup(self) -> None:
        self.wait_for_plico()
        self.beir_data = BeirDataset().load()
        try:
            self.mab_data = MABDataset().load()
        except FileNotFoundError:
            self.mab_data = None

    def run(self) -> list[dict[str, Any]]:
        # Phase 1: Ingest all data
        self._ingest_beir()
        if self.mab_data:
            self._ingest_mab()

        # Phase 2: Wait for async indexing
        timeout = getattr(self, "_preprocess_timeout", 120.0)
        self.wait_for_indexing(timeout=timeout)

        # Phase 3: Query
        results = []
        results.extend(self._query_beir())
        if self.mab_data:
            results.extend(self._query_mab())
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        by_dataset: dict[str, list[dict[str, Any]]] = {}
        for r in raw:
            ds = r.get("dataset", "unknown")
            by_dataset.setdefault(ds, []).append(r)

        overall = {}
        for ds, items in by_dataset.items():
            r5 = np.mean([r["recall@5"] for r in items]) if items else 0.0
            r10 = np.mean([r["recall@10"] for r in items]) if items else 0.0
            overall[ds] = {
                "count": len(items),
                "recall@5": round(r5, 3),
                "recall@10": round(r10, 3),
            }
        return {"overall": overall, "per_dataset": overall}

    def report(self, metrics: dict[str, Any]) -> Report:
        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v44",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
            },
            "config": {},
            "metrics": metrics,
            "costs": {},
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    # ── BEIR SciFact ───────────────────────────────────────────────

    def _ingest_beir(self) -> None:
        corpus = self.beir_data.get("corpus", {})
        self._doc_to_cid: dict[str, str] = {}
        # Ingest up to 3000 docs to improve qrels coverage (scifact has ~5k total)
        for doc_id, doc in list(corpus.items())[:3000]:
            text = doc.get("text", doc.get("title", ""))
            resp = self.client.create(text, tags=["beir", "scifact", f"doc:{doc_id}"])
            cid = resp.get("cid", "")
            if cid:
                self._doc_to_cid[doc_id] = cid

    def _query_beir(self) -> list[dict[str, Any]]:
        queries = self.beir_data.get("queries", [])
        qrels = self.beir_data.get("qrels", {})
        doc_to_cid = getattr(self, "_doc_to_cid", {})
        results = []

        queries_with_qrels = [q for q in queries if qrels.get(q.get("_id", q.get("id")))]
        max_q = self.samples or min(50, len(queries_with_qrels))
        for q in queries_with_qrels[:max_q]:
            qid = q.get("_id", q.get("id"))
            query_text = q.get("text", "")
            relevant_doc_ids = set(qrels.get(qid, []))
            if not relevant_doc_ids:
                continue
            relevant_cids = {doc_to_cid[d] for d in relevant_doc_ids if d in doc_to_cid}
            if not relevant_cids:
                continue

            resp = self.client.search(query_text, limit=20)
            hits = resp.get("results", [])
            retrieved = [h.get("cid", "") for h in hits]
            r5 = recall_at_k(relevant_cids, retrieved, 5)
            r10 = recall_at_k(relevant_cids, retrieved, 10)
            results.append({
                "dataset": "beir_scifact",
                "query_id": qid,
                "recall@5": r5,
                "recall@10": r10,
            })
        return results

    # ── MemoryAgentBench AR ────────────────────────────────────────

    def _ingest_mab(self) -> None:
        data = self.mab_data
        if not isinstance(data, list):
            return
        docs = data[: min(self.samples or 50, len(data))]
        self._mab_chunk_cids: list[tuple[list[str], list[dict[str, Any]]]] = []
        for doc in docs:
            chunks = doc.get("chunks", [])
            questions = doc.get("questions", [])
            chunk_cids: list[str] = []
            for chunk in chunks:
                resp = self.client.create(chunk, tags=["mab"])
                chunk_cids.append(resp.get("cid", ""))
            self._mab_chunk_cids.append((chunk_cids, questions))

    def _query_mab(self) -> list[dict[str, Any]]:
        results = []
        for chunk_cids, questions in getattr(self, "_mab_chunk_cids", []):
            for q in questions:
                query = q.get("question", "")
                answer = q.get("answers", "")
                # answer may be list or str — normalise to a single searchable string
                if isinstance(answer, list):
                    answer_text = " ".join(str(a) for a in answer)
                else:
                    answer_text = str(answer)

                resp = self.client.search(query, limit=10)
                hits = [h.get("snippet", "") for h in resp.get("results", [])]
                hit = any(answer_text.lower() in h.lower() for h in hits)
                results.append({
                    "dataset": "mab_ar",
                    "question": query,
                    "hit": hit,
                    "recall@5": 1.0 if hit else 0.0,
                    "recall@10": 1.0 if hit else 0.0,
                })
        return results
