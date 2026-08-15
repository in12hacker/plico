"""Quality fixture for the public lexical Working Memory recall contract."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

import numpy as np
from ir_measures import RR, Recall, calc_aggregate, nDCG

from plico_benchmarks.core.config import get_config
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase

_MEASURES = {
    "recall@5": Recall @ 5,
    "recall@10": Recall @ 10,
    "recall@20": Recall @ 20,
    "mrr@10": RR @ 10,
    "ndcg@10": nDCG @ 10,
}


@dataclass(frozen=True)
class _RecallQuery:
    sample_id: str
    token: str
    expected_revision_ids: frozenset[str]


class MemoryRecallLexicalSuite(SuiteBase):
    name = "memory-recall-lexical"
    description = "Public memory.recall lexical quality fixture"

    def setup(self) -> None:
        self.wait_for_plico()

    def run(self) -> list[dict[str, Any]]:
        config = self._config()
        count = config["query_count"]
        if self.samples is not None:
            if isinstance(self.samples, bool) or self.samples <= 0:
                raise ValueError("memory recall samples override must be positive")
            count = self.samples
        queries = self._seed_queries(count, config["duplicate_every"])
        results = [self._recall(query, config["retrieve_limit"]) for query in queries]
        results.append(self._missing_probe(config["retrieve_limit"]))
        self._workload_config = {**config, "query_count": count}
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        scored = [item for item in raw if item["kind"] == "seeded"]
        missing = next(item for item in raw if item["kind"] == "missing_probe")
        summary = {
            "count": len(scored),
            "retriever": "lexical",
            "vector_recall": "unsupported",
            "hybrid_recall": "unsupported",
            "bm25_memory_recall": "unsupported",
            "duplicate_expected_queries": sum(item["expected_count"] > 1 for item in scored),
            "duplicate_revision_ids_returned": sum(
                item["duplicate_revision_ids_returned"] for item in scored
            ),
            "missing_probe_hit_count": missing["hit_count"],
        }
        for metric in _MEASURES:
            summary[metric] = round(float(np.mean([item[metric] for item in scored])), 6)
        return {
            "overall": summary,
            "capability_ledger": [
                {
                    "sample_id": item["sample_id"],
                    "capability": "memory_recall",
                    "domain": "canonical_working_memory",
                    "retriever": "lexical",
                    "status": item["status"],
                    **{
                        field: item[field]
                        for field in (
                            "expected_count",
                            "returned_count",
                            "duplicate_revision_ids_returned",
                            *_MEASURES,
                        )
                        if field in item
                    },
                }
                for item in raw
            ],
        }

    def report(self, metrics: dict[str, Any]) -> Report:
        return Report(
            {
                "metadata": {
                    "suite": self.name,
                    "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                    "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                },
                "config": {
                    "workload": self._workload_config,
                    "fresh_vault_required": True,
                    "retriever": "lexical",
                    "unsupported": ["vector", "hybrid", "bm25"],
                },
                "metrics": metrics,
                "costs": {
                    "llm_calls": 0,
                    "retrieval_query_embedding_calls": 0,
                    "background_projection_calls": "unavailable",
                },
                "raw_results": self._raw_results,
            }
        )

    def input_artifacts(self) -> list[dict[str, Any]]:
        workload = {
            "schema": "plico.benchmark.memory-recall-lexical-workload/v1",
            "seed": self.seed,
            "config": getattr(self, "_workload_config", self._config()),
        }
        payload = json.dumps(workload, sort_keys=True, separators=(",", ":")).encode()
        return [
            {
                "role": "memory_recall_lexical_workload",
                "file_name": "embedded:memory-recall-lexical.json",
                "bytes": len(payload),
                "sha256": hashlib.sha256(payload).hexdigest(),
            }
        ]

    def _config(self) -> dict[str, Any]:
        section = get_config().benchmark.get("suites", {}).get("memory_recall_lexical")
        if not isinstance(section, dict):
            raise ValueError("memory recall lexical configuration is missing")
        if (
            section.get("top_k") != [5, 10, 20]
            or section.get("retrieve_limit") != 20
            or section.get("retriever") != "lexical"
            or section.get("unsupported_retrievers") != ["vector", "hybrid", "bm25"]
        ):
            raise ValueError("memory recall lexical capability boundary is invalid")
        for field in ("query_count", "duplicate_every"):
            value = section.get(field)
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                raise ValueError(f"memory recall {field} must be positive")
        return dict(section)

    def _seed_queries(self, count: int, duplicate_every: int) -> list[_RecallQuery]:
        queries = []
        self._seed_corpus_terms: set[str] = set()
        for index in range(count):
            token = f"memrec-{self.seed}-{self.run_id[:12]}-{index}"
            primary = f"primary {token}"
            self._seed_corpus_terms.update(_lexical_terms(primary))
            revisions = {self._create_memory(primary)}
            if index % duplicate_every == 0:
                duplicate = f"duplicate {token}"
                self._seed_corpus_terms.update(_lexical_terms(duplicate))
                revisions.add(self._create_memory(duplicate))
            queries.append(
                _RecallQuery(
                    sample_id=f"memory-recall:{index}",
                    token=token,
                    expected_revision_ids=frozenset(revisions),
                )
            )
        return queries

    def _create_memory(self, content: str) -> str:
        result = self.client.memory_create(content, tags=[f"run:{self.run_id}", "recall-quality"])
        entry = result.get("entry")
        revision_id = entry.get("entry_id") if isinstance(entry, dict) else None
        if not isinstance(revision_id, str) or not revision_id:
            raise RuntimeError("memory.create returned no revision identity")
        return revision_id

    def _recall(self, query: _RecallQuery, limit: int) -> dict[str, Any]:
        response = self.client.memory_recall(query.token, limit=limit)
        hits = response.get("hits")
        if not isinstance(hits, list):
            raise RuntimeError("memory.recall returned no hits list")
        revision_ids = _validated_lexical_revision_ids(hits)
        duplicates = len(revision_ids) - len(set(revision_ids))
        if duplicates:
            raise RuntimeError("memory.recall returned a duplicate revision identity")
        run = {
            query.sample_id: {
                revision_id: float(len(revision_ids) - rank)
                for rank, revision_id in enumerate(revision_ids)
            }
        }
        qrels = {query.sample_id: {revision_id: 1 for revision_id in query.expected_revision_ids}}
        measured = calc_aggregate(_MEASURES.values(), qrels, run)
        return {
            "kind": "seeded",
            "sample_id": query.sample_id,
            "status": "ok",
            "expected_count": len(query.expected_revision_ids),
            "returned_count": len(revision_ids),
            "duplicate_revision_ids_returned": duplicates,
            **{name: float(measured[measure]) for name, measure in _MEASURES.items()},
        }

    def _missing_probe(self, limit: int) -> dict[str, Any]:
        seed_terms = getattr(self, "_seed_corpus_terms", set())
        token = "z" + hashlib.sha256(f"missing:{self.seed}:{self.run_id}".encode()).hexdigest()
        if not _lexical_terms(token) or _lexical_terms(token).intersection(seed_terms):
            raise RuntimeError(
                "missing memory probe is not lexically disjoint from the seed corpus"
            )
        response = self.client.memory_recall(token, limit=limit)
        hits = response.get("hits")
        if not isinstance(hits, list):
            raise RuntimeError("memory.recall missing probe returned no hits list")
        _validated_lexical_revision_ids(hits)
        return {
            "kind": "missing_probe",
            "sample_id": "memory-recall:missing",
            "status": "ok" if not hits else "partial",
            "hit_count": len(hits),
        }


def _validated_lexical_revision_ids(hits: list[Any]) -> list[str]:
    revision_ids = []
    for hit in hits:
        if (
            not isinstance(hit, dict)
            or hit.get("matched_by") != "lexical_overlap"
            or not isinstance(hit.get("entry"), dict)
            or not isinstance(hit["entry"].get("entry_id"), str)
            or not hit["entry"]["entry_id"]
        ):
            raise RuntimeError("memory.recall returned a non-lexical or malformed hit")
        revision_ids.append(hit["entry"]["entry_id"])
    return revision_ids


def _lexical_terms(text: str) -> set[str]:
    terms: set[str] = set()
    word: list[str] = []
    cjk: list[str] = []

    def flush_word() -> None:
        if word:
            terms.add("".join(word))
            word.clear()

    def flush_cjk() -> None:
        if len(cjk) == 1:
            terms.add(cjk[0])
        elif cjk:
            terms.update("".join(cjk[index : index + 2]) for index in range(len(cjk) - 1))
        cjk.clear()

    for character in text.casefold():
        codepoint = ord(character)
        is_cjk = (
            0x3400 <= codepoint <= 0x4DBF
            or 0x4E00 <= codepoint <= 0x9FFF
            or 0xF900 <= codepoint <= 0xFAFF
        )
        if is_cjk:
            flush_word()
            cjk.append(character)
        elif character.isalnum():
            flush_cjk()
            word.append(character)
        else:
            flush_word()
            flush_cjk()
    flush_word()
    flush_cjk()
    return terms
