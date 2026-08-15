"""Fail-closed retrieval and lexical-memory methodology contracts."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.baselines.bm25 import Bm25Candidate
from plico_benchmarks.core.retrieval_execution import (
    EMBEDDING_DEGRADATIONS,
    EMBEDDING_STATES,
    RETRIEVAL_PATHS,
    validate_embedding_query,
    validate_retrieval_execution,
)
from plico_benchmarks.suites.memory_recall import (
    MemoryRecallLexicalSuite,
    _lexical_terms,
    _validated_lexical_revision_ids,
)
from plico_benchmarks.suites.retrieval import RetrievalSuite, _Query


class _ObjectSearchClient:
    def __init__(self, response):
        self.response = response

    def object_search(self, query, limit, require_tags):
        return self.response


def _valid_object_search_response():
    return {
        "hits": [{"cid": "cid-1"}],
        "embedding_query": {"state": "succeeded"},
        "retrieval": [
            {"path": "bm25", "candidates": 3, "accepted": 2},
            {"path": "vector", "candidates": 2, "accepted": 1},
        ],
    }


def test_public_retrieval_execution_wire_contract_matches_exact_v2_enums():
    assert EMBEDDING_STATES == {"not_probed", "succeeded", "degraded"}
    assert EMBEDDING_DEGRADATIONS == {
        "provider_unavailable",
        "model_unavailable",
        "input_rejected",
        "execution_failed",
    }
    assert RETRIEVAL_PATHS == {
        "bm25",
        "vector",
        "tag_fallback",
        "knowledge_graph_temporal",
        "knowledge_graph_ppr",
        "knowledge_graph_path_discovery",
        "knowledge_graph_causal",
        "reranker",
    }
    assert validate_embedding_query({"state": "not_probed"}) == ("not_probed", None)
    assert [
        item["path"]
        for item in validate_retrieval_execution(
            [{"path": path, "candidates": 0, "accepted": 0} for path in sorted(RETRIEVAL_PATHS)]
        )
    ] == sorted(RETRIEVAL_PATHS)


def _bm25_config() -> dict:
    return {
        "implementation": "bm25s",
        "version": "0.3.10",
        "method": "lucene",
        "k1": 1.2,
        "b": 0.75,
        "tokenizer": {
            "contract": "bm25s_regex_words_v1",
            "lower": True,
            "token_pattern": r"(?u)\b\w\w+\b",
            "stopwords": "en",
            "stemmer": "none",
        },
    }


def test_bm25_candidate_uses_pinned_contract_and_deterministic_ties():
    candidate = Bm25Candidate.from_config(
        {"z-doc": "orchid beta", "a-doc": "orchid alpha", "b-doc": "gamma"},
        _bm25_config(),
    )

    ranked = candidate.search("orchid", limit=3)

    assert [item.document_id for item in ranked[:2]] == ["a-doc", "z-doc"]
    assert candidate.manifest() == {
        "candidate": "bm25_only",
        "domain": "benchmark_text_corpus",
        "implementation": "bm25s",
        "version": "0.3.10",
        "method": "lucene",
        "k1": 1.2,
        "b": 0.75,
        "tokenizer": _bm25_config()["tokenizer"],
    }


def test_bm25_tokenizer_contract_is_explicit_for_unicode_and_single_characters():
    candidate = Bm25Candidate.from_config(
        {"b-doc": "x", "a-doc": "机器 学习", "c-doc": "unrelated"},
        _bm25_config(),
    )

    assert candidate.search("机器", limit=3)[0].document_id == "a-doc"
    assert [item.document_id for item in candidate.search("x", limit=3)] == [
        "a-doc",
        "b-doc",
        "c-doc",
    ]


@pytest.mark.parametrize(
    "mutation",
    [
        lambda value: value.update(implementation="handwritten"),
        lambda value: value.update(version="0.3.9"),
        lambda value: value["tokenizer"].update(token_pattern=r"\w+"),
    ],
)
def test_bm25_config_drift_fails_closed(mutation):
    config = _bm25_config()
    mutation(config)
    with pytest.raises(ValueError):
        Bm25Candidate.from_config({"doc": "alpha beta"}, config)


def test_plico_candidate_preserves_typed_execution_ledger():
    suite = RetrievalSuite(client=_ObjectSearchClient(_valid_object_search_response()), seed=7)
    suite._plico_document_by_cid = {("scope-1", "cid-1"): "doc-1"}
    query = _Query("dataset", "sample-1", "stratum", "scope-1", "query", {"doc-1": 1.0})

    ranked, observation = suite._search_candidate(query, "plico_object_search")

    assert [result.document_id for result in ranked] == ["doc-1"]
    assert observation["embedding_query_state"] == "succeeded"
    assert observation["embedding_backend"] == "not_attested_by_public_response"
    assert observation["retrieval_execution"] == [
        {"path": "bm25", "candidates": 3, "accepted": 2, "degradation": None},
        {"path": "vector", "candidates": 2, "accepted": 1, "degradation": None},
    ]
    assert observation["degraded"] is False

    metrics = suite.evaluate(
        [
            {
                "dataset": "dataset",
                "sample_id": "sample-1",
                "stratum": "stratum",
                "candidate": "plico_object_search",
                "domain": "plico_object_projection",
                "status": "ok",
                **observation,
                "recall@5": 1.0,
                "recall@10": 1.0,
                "recall@20": 1.0,
                "mrr@10": 1.0,
                "ndcg@10": 1.0,
            }
        ]
    )
    assert (
        metrics["capability_ledger"][0]["retrieval_execution"] == observation["retrieval_execution"]
    )
    assert metrics["overall"]["dataset"]["plico_object_search"]["retrieval_path_counts"][
        "vector"
    ] == {"queries": 1, "candidates": 2, "accepted": 1, "degraded": 0}


@pytest.mark.parametrize(
    "mutation",
    [
        lambda value: value.update(retrieval=[]),
        lambda value: value["retrieval"][0].update(accepted=4),
        lambda value: value["retrieval"][0].update(path="invented"),
        lambda value: value.update(
            embedding_query={"state": "succeeded", "degradation": "execution_failed"}
        ),
    ],
)
def test_plico_candidate_rejects_invalid_or_self_inconsistent_execution(mutation):
    response = _valid_object_search_response()
    mutation(response)
    suite = RetrievalSuite(client=_ObjectSearchClient(response), seed=7)
    suite._plico_document_by_cid = {("scope-1", "cid-1"): "doc-1"}
    query = _Query("dataset", "sample-1", "stratum", "scope-1", "query", {"doc-1": 1.0})

    with pytest.raises(RuntimeError, match="object.search"):
        suite._search_candidate(query, "plico_object_search")


@pytest.mark.parametrize(
    "embedding,retrieval",
    [
        ({"state": "not_probed"}, [{"path": "bm25", "candidates": 3, "accepted": 2}]),
        (
            {"state": "succeeded"},
            [
                {"path": "bm25", "candidates": 3, "accepted": 2},
                {"path": "vector", "candidates": 2, "accepted": 0},
            ],
        ),
    ],
)
def test_real_embedding_gate_requires_an_accepted_non_degraded_vector_path(
    monkeypatch, embedding, retrieval
):
    monkeypatch.setenv("PLICO_BENCH_REQUIRE_REAL_EMBEDDING", "1")
    response = _valid_object_search_response()
    response["embedding_query"] = embedding
    response["retrieval"] = retrieval
    suite = RetrievalSuite(client=_ObjectSearchClient(response), seed=7)
    suite._plico_document_by_cid = {("scope-1", "cid-1"): "doc-1"}
    query = _Query("dataset", "sample-1", "stratum", "scope-1", "query", {"doc-1": 1.0})

    with pytest.raises(RuntimeError, match="succeeded vector execution"):
        suite._search_candidate(query, "plico_object_search")


def test_vector_candidate_is_unsupported_without_same_run_sealed_snapshot():
    suite = RetrievalSuite(seed=7)
    suite._scopes = {"scope-1": {"doc-1": "alpha beta"}}

    indexes = suite._build_candidate_indexes(suite._config())

    assert "vector_only" not in indexes["scope-1"]
    vector_manifest = next(
        manifest
        for manifest in suite._candidate_manifests
        if manifest["candidate"] == "vector_only"
    )
    assert vector_manifest == {
        "candidate": "vector_only",
        "domain": "benchmark_text_corpus",
        "status": "unsupported",
        "reason": "sealed_same_run_provider_snapshot_unavailable",
    }


def test_memory_missing_probe_uses_one_term_disjoint_from_seed_corpus():
    class RecallClient:
        query = None

        def memory_recall(self, query, limit):
            self.query = query
            return {"hits": []}

    client = RecallClient()
    suite = MemoryRecallLexicalSuite(client=client, seed=7)
    suite._seed_corpus_terms = _lexical_terms("primary memrec-7-runprefix-0 duplicate")

    result = suite._missing_probe(20)

    assert result["hit_count"] == 0
    assert len(_lexical_terms(client.query)) == 1
    assert _lexical_terms(client.query).isdisjoint(suite._seed_corpus_terms)


def test_memory_recall_rejects_nonlexical_or_malformed_hit():
    with pytest.raises(RuntimeError, match="non-lexical"):
        _validated_lexical_revision_ids(
            [{"entry": {"entry_id": "revision-1"}, "matched_by": "vector"}]
        )
