"""Paired retrieval evaluation over Plico object search and simple baselines."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass
from datetime import datetime, timezone
from typing import Any

import numpy as np
from ir_measures import RR, Recall, calc_aggregate, nDCG

from plico_benchmarks.baselines import (
    Bm25Candidate,
    ExactVectorCandidate,
    RetrievalCandidate,
    SearchResult,
)
from plico_benchmarks.core.config import get_config
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.core.retrieval_execution import (
    real_embedding_required,
    validate_embedding_query,
    validate_retrieval_execution,
    verified_vector_execution,
)
from plico_benchmarks.core.sampling import (
    configured_limit,
    configured_profile,
    selection_artifact,
    stable_stratified_sample,
)
from plico_benchmarks.datasets.beir import BeirDataset
from plico_benchmarks.datasets.memoryagentbench import MABDataset
from plico_benchmarks.suites.base import SuiteBase

_MEASURES = {
    "recall@5": Recall @ 5,
    "recall@10": Recall @ 10,
    "recall@20": Recall @ 20,
    "mrr@10": RR @ 10,
    "ndcg@10": nDCG @ 10,
}
_REQUIRED_CANDIDATES = {"plico_object_search", "bm25_only", "vector_only"}


@dataclass(frozen=True)
class _Query:
    dataset: str
    sample_id: str
    stratum: str
    scope: str
    text: str
    relevances: dict[str, float]


class RetrievalSuite(SuiteBase):
    name = "retrieval"
    description = "Paired SciFact and MAB AR object-retrieval evaluation"

    def setup(self) -> None:
        self.wait_for_plico()
        self._beir_dataset = BeirDataset()
        self._mab_dataset = MABDataset()
        self.beir_data = self._beir_dataset.load()
        self.mab_data = self._mab_dataset.load()

    def run(self) -> list[dict[str, Any]]:
        section = self._config()
        self._profile = configured_profile(section)
        self._scopes, self._queries = self._prepare_workloads(section)
        self._selection_artifact = selection_artifact(
            role="retrieval_sample_selection",
            seed=self.seed,
            profile=self._profile,
            sample_ids=[query.sample_id for query in self._queries],
        )
        self._ingest_plico_objects()
        self.wait_for_indexing(timeout=getattr(self, "_preprocess_timeout", 120.0))
        self._candidate_indexes = self._build_candidate_indexes(section)
        return self._evaluate_queries(section)

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        grouped: dict[tuple[str, str], list[dict[str, Any]]] = {}
        for item in raw:
            grouped.setdefault((item["dataset"], item["candidate"]), []).append(item)
        overall: dict[str, dict[str, Any]] = {}
        for (dataset, candidate), items in sorted(grouped.items()):
            measured = [item for item in items if item["status"] == "ok"]
            summary: dict[str, Any] = {
                "requested": len(items),
                "measured": len(measured),
                "degraded": sum(item["status"] == "degraded" for item in items),
                "unsupported": sum(item["status"] == "unsupported" for item in items),
                "status": (
                    "measured"
                    if len(measured) == len(items)
                    else "partial"
                    if measured
                    else "unsupported"
                ),
            }
            for metric in _MEASURES:
                summary[metric] = (
                    round(float(np.mean([item[metric] for item in measured])), 6)
                    if measured
                    else None
                )
            if candidate == "plico_object_search":
                path_counts: dict[str, dict[str, int]] = {}
                for item in items:
                    for execution in item.get("retrieval_execution", []):
                        counts = path_counts.setdefault(
                            execution["path"],
                            {"queries": 0, "candidates": 0, "accepted": 0, "degraded": 0},
                        )
                        counts["queries"] += 1
                        counts["candidates"] += execution["candidates"]
                        counts["accepted"] += execution["accepted"]
                        counts["degraded"] += execution["degradation"] is not None
                summary["retrieval_path_counts"] = dict(sorted(path_counts.items()))
            overall.setdefault(dataset, {})[candidate] = summary
        capability_ledger = []
        for item in raw:
            row = {
                "sample_id": item["sample_id"],
                "dataset": item["dataset"],
                "stratum": item["stratum"],
                "candidate": item["candidate"],
                "capability": "object_retrieval",
                "domain": item["domain"],
                "status": item["status"],
            }
            row.update({metric: item[metric] for metric in _MEASURES if metric in item})
            row.update(
                {
                    field: item[field]
                    for field in (
                        "embedding_query_state",
                        "embedding_query_degradation",
                        "embedding_backend",
                        "retrieval_execution",
                        "retriever",
                        "degraded",
                    )
                    if field in item
                }
            )
            capability_ledger.append(row)
        return {
            "overall": overall,
            "per_dataset_candidate": overall,
            "capability_ledger": capability_ledger,
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
                    "run_id": self.run_id,
                    "sampling_profile": self._profile,
                    "sampling_strategy": "deterministic_sha256_stratified_v1",
                    "candidate_domains": {
                        "plico_object_search": "plico_object_projection",
                        "bm25_only": "benchmark_text_corpus",
                        "vector_only": "benchmark_text_corpus",
                    },
                    "memory_recall_claimed": False,
                },
                "metrics": metrics,
                "costs": {},
                "raw_results": self._raw_results,
            }
        )

    def input_artifacts(self) -> list[dict[str, Any]]:
        artifacts = [self._beir_dataset.artifact_manifest(), self._mab_dataset.artifact_manifest()]
        selection = getattr(self, "_selection_artifact", None)
        if selection is not None:
            artifacts.append(selection)
        manifests = getattr(self, "_candidate_manifests", None)
        if manifests:
            payload = json.dumps(manifests, sort_keys=True, separators=(",", ":")).encode()
            artifacts.append(
                {
                    "role": "retrieval_candidate_matrix",
                    "file_name": "embedded:retrieval-candidates.json",
                    "bytes": len(payload),
                    "sha256": hashlib.sha256(payload).hexdigest(),
                }
            )
        return artifacts

    def _config(self) -> dict[str, Any]:
        section = get_config().benchmark.get("suites", {}).get("retrieval")
        if not isinstance(section, dict):
            raise ValueError("retrieval benchmark configuration is missing")
        candidates = section.get("candidates")
        if (
            not isinstance(candidates, list)
            or len(candidates) != len(set(candidates))
            or set(candidates) != _REQUIRED_CANDIDATES
        ):
            raise ValueError("retrieval candidate matrix must contain exact three candidates")
        top_k = section.get("top_k")
        if top_k != [5, 10, 20] or section.get("retrieve_limit") != 20:
            raise ValueError("retrieval metrics require exact top-k [5,10,20] and limit 20")
        return section

    def _prepare_workloads(
        self, section: dict[str, Any]
    ) -> tuple[dict[str, dict[str, str]], list[_Query]]:
        scopes, beir_queries = self._prepare_beir(
            configured_limit(
                section,
                profile=self._profile,
                dataset="beir_scifact",
                override=self.samples,
            )
        )
        mab_scopes, mab_queries = self._prepare_mab(
            configured_limit(
                section,
                profile=self._profile,
                dataset="memoryagentbench_ar",
                override=self.samples,
            )
        )
        overlap = set(scopes).intersection(mab_scopes)
        if overlap:
            raise ValueError(f"retrieval scope collision: {sorted(overlap)!r}")
        scopes.update(mab_scopes)
        queries = beir_queries + mab_queries
        if len({query.sample_id for query in queries}) != len(queries):
            raise ValueError("retrieval sample IDs collide across datasets")
        return scopes, queries

    def _prepare_beir(self, limit: int | None) -> tuple[dict[str, dict[str, str]], list[_Query]]:
        corpus = self.beir_data.get("corpus", {})
        raw_queries = self.beir_data.get("queries", [])
        qrels = self.beir_data.get("qrels", {})
        if not isinstance(corpus, dict) or not isinstance(raw_queries, list):
            raise TypeError("SciFact cache has an invalid normalized schema")
        raw_documents = {
            str(document_id): _beir_document_text(document)
            for document_id, document in corpus.items()
        }
        documents, aliases, duplicate_groups = _deduplicate_documents(raw_documents)
        candidates: list[_Query] = []
        for raw in raw_queries:
            query_id = str(raw.get("_id", raw.get("id", "")))
            relevance = _normalized_relevance(qrels.get(query_id))
            _reject_ambiguous_duplicate_qrels(relevance, duplicate_groups, query_id)
            relevance = {
                aliases[doc_id]: score for doc_id, score in relevance.items() if doc_id in aliases
            }
            if query_id and relevance:
                candidates.append(
                    _Query(
                        dataset="beir_scifact",
                        sample_id=f"beir_scifact:{query_id}",
                        stratum="scifact",
                        scope="beir_scifact",
                        text=str(raw.get("text", "")),
                        relevances=relevance,
                    )
                )
        selected = stable_stratified_sample(
            candidates,
            limit=limit,
            seed=self.seed,
            namespace="retrieval:beir_scifact",
            sample_id=lambda query: query.sample_id,
            stratum=lambda query: query.stratum,
        )
        return {"beir_scifact": documents}, selected

    def _prepare_mab(self, limit: int | None) -> tuple[dict[str, dict[str, str]], list[_Query]]:
        if not isinstance(self.mab_data, list):
            raise TypeError("MemoryAgentBench AR cache must be a list of documents")
        scopes: dict[str, dict[str, str]] = {}
        queries: list[_Query] = []
        for document_index, document in enumerate(self.mab_data):
            chunks = document.get("chunks") or (
                [document["context"]] if document.get("context") else []
            )
            questions = self._normalize_mab_questions(document)
            if not chunks or not questions:
                raise ValueError(f"MemoryAgentBench document {document_index} is incomplete")
            scope = f"mab-doc:{document_index}"
            raw_documents = {
                f"{scope}:chunk:{chunk_index}": str(chunk)
                for chunk_index, chunk in enumerate(chunks)
            }
            documents, aliases, _ = _deduplicate_documents(raw_documents)
            scopes[scope] = documents
            for question_index, question in enumerate(questions):
                qa_id = str(question.get("qa_pair_id") or f"question-{question_index}")
                answers = _normalized_answers(question.get("answers"))
                relevances = {
                    aliases[document_id]: 1.0
                    for document_id, text in raw_documents.items()
                    if any(answer in text.casefold() for answer in answers)
                }
                if not relevances:
                    raise ValueError(f"{scope}:{qa_id} has no answer-bearing source chunk")
                queries.append(
                    _Query(
                        dataset="memoryagentbench_ar_proxy",
                        sample_id=f"memoryagentbench_ar:{document_index}:{qa_id}",
                        stratum="accurate_retrieval",
                        scope=scope,
                        text=str(question.get("question", "")),
                        relevances=relevances,
                    )
                )
        selected = stable_stratified_sample(
            queries,
            limit=limit,
            seed=self.seed,
            namespace="retrieval:memoryagentbench_ar",
            sample_id=lambda query: query.sample_id,
            stratum=lambda query: query.stratum,
        )
        selected_scopes = {query.scope for query in selected}
        return {scope: scopes[scope] for scope in selected_scopes}, selected

    def _ingest_plico_objects(self) -> None:
        self._plico_document_by_cid: dict[tuple[str, str], str] = {}
        for scope, documents in self._scopes.items():
            for document_id, text in documents.items():
                response = self.client.object_put(
                    text,
                    tags=[f"run:{self.run_id}", "retrieval", scope],
                )
                cid = response.get("cid")
                if not isinstance(cid, str) or not cid:
                    raise RuntimeError(f"object ingest returned no CID for {document_id}")
                self._plico_document_by_cid[(scope, cid)] = document_id

    def _build_candidate_indexes(
        self, section: dict[str, Any]
    ) -> dict[str, dict[str, RetrievalCandidate]]:
        encoder = getattr(self, "_vector_encoder_override", None)
        vector_unavailable = encoder is None
        indexes: dict[str, dict[str, RetrievalCandidate]] = {}
        manifests: dict[str, dict[str, object]] = {}
        for scope, documents in self._scopes.items():
            bm25 = Bm25Candidate.from_config(documents, section["bm25"])
            indexes[scope] = {bm25.name: bm25}
            manifests.setdefault(bm25.name, bm25.manifest())
            if not vector_unavailable:
                vector = ExactVectorCandidate(
                    documents,
                    encoder=encoder,
                    model="deterministic-test-vector",
                )
                indexes[scope][vector.name] = vector
                manifests.setdefault(vector.name, vector.manifest())
        if vector_unavailable:
            manifests["vector_only"] = {
                "candidate": "vector_only",
                "domain": "benchmark_text_corpus",
                "status": "unsupported",
                "reason": "sealed_same_run_provider_snapshot_unavailable",
            }
        manifests["plico_object_search"] = {
            "candidate": "plico_object_search",
            "domain": "plico_object_projection",
            "public_operation": "object.search",
            "memory_recall": False,
        }
        self._candidate_manifests = [manifests[name] for name in section["candidates"]]
        return indexes

    def _evaluate_queries(self, section: dict[str, Any]) -> list[dict[str, Any]]:
        results = []
        for query in self._queries:
            for candidate in section["candidates"]:
                if candidate not in self._candidate_indexes[query.scope] and candidate != (
                    "plico_object_search"
                ):
                    results.append(
                        {
                            "dataset": query.dataset,
                            "sample_id": query.sample_id,
                            "stratum": query.stratum,
                            "candidate": candidate,
                            "domain": "benchmark_text_corpus",
                            "status": "unsupported",
                            "reason": "identity_unavailable",
                        }
                    )
                    continue
                ranked, observation = self._search_candidate(query, candidate)
                metrics = _ir_metrics(query, ranked)
                results.append(
                    {
                        "dataset": query.dataset,
                        "sample_id": query.sample_id,
                        "stratum": query.stratum,
                        "candidate": candidate,
                        "domain": (
                            "plico_object_projection"
                            if candidate == "plico_object_search"
                            else "benchmark_text_corpus"
                        ),
                        "status": "degraded" if observation["degraded"] else "ok",
                        **observation,
                        **metrics,
                    }
                )
        return results

    def _search_candidate(
        self, query: _Query, candidate: str
    ) -> tuple[list[SearchResult], dict[str, Any]]:
        if candidate == "plico_object_search":
            response = self.client.object_search(
                query.text,
                limit=20,
                require_tags=[f"run:{self.run_id}", "retrieval", query.scope],
            )
            hits = response.get("hits")
            if not isinstance(hits, list):
                raise RuntimeError("object.search returned no hits list")
            embedding_query = response.get("embedding_query")
            embedding_state, embedding_degradation = validate_embedding_query(embedding_query)
            retrieval_execution = validate_retrieval_execution(response.get("retrieval"))
            degraded = embedding_state == "degraded" or any(
                item.get("degradation") is not None for item in retrieval_execution
            )
            if real_embedding_required() and not verified_vector_execution(
                embedding_state, retrieval_execution
            ):
                raise RuntimeError(
                    "real-embedding Plico candidate did not prove succeeded vector execution"
                )
            results = []
            for rank, hit in enumerate(hits):
                cid = hit.get("cid") if isinstance(hit, dict) else None
                document_id = self._plico_document_by_cid.get((query.scope, str(cid)))
                if document_id is not None:
                    results.append(SearchResult(document_id, float(len(hits) - rank)))
            return results, {
                "embedding_query_state": str(embedding_state),
                "embedding_query_degradation": embedding_degradation,
                "retrieval_execution": retrieval_execution,
                "embedding_backend": "not_attested_by_public_response",
                "retriever": "public_object_search",
                "degraded": degraded,
            }
        observation = {
            "embedding_query_state": (
                "not_applicable" if candidate == "bm25_only" else "succeeded"
            ),
            "embedding_backend": ("not_applicable" if candidate == "bm25_only" else "verified"),
            "retriever": candidate,
            "degraded": False,
        }
        return (
            self._candidate_indexes[query.scope][candidate].search(query.text, limit=20),
            observation,
        )

    @staticmethod
    def _normalize_mab_questions(document: dict[str, Any]) -> list[dict[str, Any]]:
        questions = document.get("questions", [])
        if questions and isinstance(questions[0], dict):
            return [
                {
                    **question,
                    "answers": (
                        question.get("answers", [])
                        if isinstance(question.get("answers", []), list)
                        else [question.get("answers")]
                    ),
                    "qa_pair_id": question.get("qa_pair_id", f"question-{index}"),
                }
                for index, question in enumerate(questions)
            ]
        answers = document.get("answers", [])
        qa_pair_ids = (document.get("metadata") or {}).get("qa_pair_ids", [])
        normalized = []
        for index, question in enumerate(questions):
            expected = answers[index] if index < len(answers) else []
            normalized.append(
                {
                    "question": str(question),
                    "answers": expected if isinstance(expected, list) else [expected],
                    "qa_pair_id": (
                        qa_pair_ids[index] if index < len(qa_pair_ids) else f"question-{index}"
                    ),
                }
            )
        return normalized


def _beir_document_text(document: Any) -> str:
    if not isinstance(document, dict):
        raise TypeError("SciFact corpus document must be an object")
    text = "\n".join(
        part.strip()
        for part in (str(document.get("title", "")), str(document.get("text", "")))
        if part.strip()
    )
    if not text:
        raise ValueError("SciFact corpus document is blank")
    return text


def _normalized_relevance(value: Any) -> dict[str, float]:
    if isinstance(value, dict):
        normalized = {}
        for document_id, score in value.items():
            if isinstance(score, bool) or not isinstance(score, (int, float)):
                raise ValueError("qrel score must be numeric")
            numeric = float(score)
            if not np.isfinite(numeric) or numeric < 0:
                raise ValueError("qrel score must be finite and non-negative")
            if numeric > 0:
                normalized[str(document_id)] = numeric
        return normalized
    if isinstance(value, list):
        return {str(document_id): 1.0 for document_id in value}
    return {}


def _normalized_answers(value: Any) -> list[str]:
    values = value if isinstance(value, list) else [value]
    answers = [str(answer).strip().casefold() for answer in values]
    answers = [answer for answer in answers if answer]
    if not answers:
        raise ValueError("MemoryAgentBench question has no answers")
    return answers


def _ir_metrics(query: _Query, ranked: list[SearchResult]) -> dict[str, float]:
    run = {
        query.sample_id: {
            result.document_id: result.score for result in ranked if result.document_id
        }
    }
    qrels = {query.sample_id: query.relevances}
    calculated = calc_aggregate(_MEASURES.values(), qrels, run)
    return {name: float(calculated[measure]) for name, measure in _MEASURES.items()}


def _deduplicate_documents(
    documents: dict[str, str],
) -> tuple[dict[str, str], dict[str, str], dict[str, list[str]]]:
    canonical: dict[str, str] = {}
    aliases: dict[str, str] = {}
    groups: dict[str, list[str]] = {}
    for document_id, text in documents.items():
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
        canonical_id = f"content-sha256:{digest}"
        canonical.setdefault(canonical_id, text)
        aliases[document_id] = canonical_id
        groups.setdefault(canonical_id, []).append(document_id)
    return canonical, aliases, groups


def _reject_ambiguous_duplicate_qrels(
    relevances: dict[str, float], duplicate_groups: dict[str, list[str]], query_id: str
) -> None:
    for aliases in duplicate_groups.values():
        grades = {relevances.get(document_id, 0.0) for document_id in aliases}
        if len(grades) > 1:
            raise ValueError(f"SciFact query {query_id} has ambiguous qrels for duplicate content")
