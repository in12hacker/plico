"""Dataset-domain and ground-truth integrity regressions."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

import pytest

from plico_benchmarks.core.result_artifact import validate_qa_retrieval_runtime
from plico_benchmarks.suites.conversational_qa import (
    ConversationalQASuite,
    _adversarial_abstention_correct,
    _verify_attempt_cost,
)
from plico_benchmarks.suites.retrieval import RetrievalSuite


class _StaticLlm:
    def chat(self, messages, max_tokens=128, **kwargs):
        return "expected"


class _AbstainingLlm:
    def chat(self, messages, max_tokens=128, **kwargs):
        return "No information available"


class _StaticJudge:
    def evaluate_scored(self, question, expected, predicted, **kwargs):
        return 5, "correct"

    def describe(self):
        return "StaticJudge(test-double)"


class _FailIfCalledJudge(_StaticJudge):
    def evaluate_scored(self, question, expected, predicted, **kwargs):
        raise AssertionError("adversarial unanswerable samples must not call the paid judge")


class _SearchClient:
    host = "127.0.0.1"
    port = 7878

    def __init__(self, hits):
        self.hits = hits
        self.searches = []

    def object_search(self, query, **kwargs):
        self.searches.append((query, kwargs))
        return {
            "hits": self.hits,
            "embedding_query": {"state": "succeeded"},
            "retrieval": [
                {"path": "bm25", "candidates": len(self.hits), "accepted": len(self.hits)},
                {"path": "vector", "candidates": len(self.hits), "accepted": len(self.hits)},
            ],
        }


def test_real_vector_artifact_accepts_object_only_openai_identity_without_overclaiming():
    runtime = {
        "requirement": "real_non_stub_vector_per_query",
        "configured_embedding_backend": "openai",
        "active_embedding_provider": "unavailable",
        "embedding_provider_state": "unavailable",
        "provider_identity_scope": "object_execution_only_unattested_provider",
        "ingest_watermark": {"accepted": 7, "completed": 7, "in_flight": 0},
    }
    ledger = [
        {
            "embedding_query_state": "succeeded",
            "verified_vector_execution": True,
            "retrieval_degraded": False,
        }
    ]

    validate_qa_retrieval_runtime({"retrieval_runtime": runtime}, ledger)

    with pytest.raises(ValueError, match="identity scope"):
        validate_qa_retrieval_runtime(
            {
                "retrieval_runtime": {
                    **runtime,
                    "provider_identity_scope": "projection_publishable_identity",
                }
            },
            ledger,
        )

    with pytest.raises(ValueError, match="real-vector"):
        validate_qa_retrieval_runtime(
            {
                "retrieval_runtime": {
                    **runtime,
                    "configured_embedding_backend": "ollama",
                    "provider_identity_scope": "unavailable",
                }
            },
            ledger,
        )

    with pytest.raises(ValueError, match="ingest watermark"):
        validate_qa_retrieval_runtime(
            {
                "retrieval_runtime": {
                    **runtime,
                    "ingest_watermark": {
                        "accepted": 7,
                        "completed": 6,
                        "in_flight": 1,
                    },
                }
            },
            ledger,
        )


def test_locomo_query_is_scoped_and_scores_ground_truth_evidence():
    client = _SearchClient([{"cid": "evidence-cid", "snippet": "expected"}])
    suite = ConversationalQASuite(client=client, seed=7)
    suite.llm = _StaticLlm()
    suite.judge = _StaticJudge()
    suite._locomo_sample = [
        (
            3,
            4,
            {},
            {
                "question": "what happened?",
                "answer": "expected",
                "category": 1,
                "evidence": ["D1:3"],
            },
        )
    ]
    suite._locomo_evidence_cids = {(3, "D1:3"): "evidence-cid"}

    result = suite._query_locomo()

    assert result[0]["sample_id"] == "locomo:conv-3:qa-4"
    assert result[0]["evidence_recall@10"] == 1.0
    assert result[0]["verified_vector_execution"] is True
    assert client.searches[0][1]["require_tags"] == [
        f"run:{suite.run_id}",
        "locomo",
        "conv-3",
    ]
    assert "has_context" not in result[0]


def test_locomo_adversarial_null_answer_uses_deterministic_abstention_not_judge():
    client = _SearchClient([{"cid": "evidence-cid", "snippet": "irrelevant context"}])
    suite = ConversationalQASuite(client=client, seed=7)
    suite.llm = _AbstainingLlm()
    suite.judge = _FailIfCalledJudge()
    suite._locomo_sample = [
        (
            3,
            5,
            {},
            {
                "question": "what was never stated?",
                "answer": None,
                "category": 5,
                "evidence": ["D1:3"],
            },
        )
    ]
    suite._locomo_evidence_cids = {(3, "D1:3"): "evidence-cid"}

    result = suite._query_locomo()

    assert result[0]["answerability"] == "adversarial_unanswerable"
    assert result[0]["abstention_correct"] is True
    assert result[0]["f1"] is None
    assert result[0]["bleu1"] is None
    assert result[0]["llm_score"] is None


def test_adversarial_abstention_requires_one_exact_normalized_phrase():
    assert _adversarial_abstention_correct("No information available.")
    assert _adversarial_abstention_correct("NOT MENTIONED")
    assert not _adversarial_abstention_correct("No information available, but probably yes")
    assert not _adversarial_abstention_correct("I don't know")


def test_longmemeval_query_is_scoped_to_question_evidence_domain():
    client = _SearchClient([{"cid": "answer-session-cid", "snippet": "expected"}])
    suite = ConversationalQASuite(client=client, seed=7)
    suite.llm = _StaticLlm()
    suite.judge = _StaticJudge()
    suite.longmemeval = [{}]
    suite._longmemeval_sample = [
        {
            "question_id": "question-17",
            "question": "what happened?",
            "answer": "expected",
            "question_type": "knowledge-update",
            "answer_session_ids": ["session-9"],
        }
    ]
    suite._longmemeval_evidence_cids = {("question-17", "session-9"): "answer-session-cid"}

    result = suite._query_longmemeval()

    assert result[0]["sample_id"] == "longmemeval:question-17"
    assert result[0]["evidence_recall@10"] == 1.0
    assert result[0]["verified_vector_execution"] is True
    assert client.searches[0][1]["require_tags"] == [
        f"run:{suite.run_id}",
        "longmemeval",
        "question:question-17",
    ]


def test_qa_sample_accounting_is_exact_and_cost_ledger_is_not_empty_placeholder():
    suite = ConversationalQASuite(seed=7)
    suite.llm = _StaticLlm()
    suite.judge = _StaticJudge()
    sample_id = "locomo:conv-3:qa-4"
    suite._selected_sample_ids = [sample_id]
    suite._qa_config = {"ragas_style_proxy_samples": 0}
    suite._qa_attempt_evidence = []
    suite._qa_attempt_keys = set()
    suite._qa_request_refs = {sample_id: []}
    suite._qa_budget_before = {
        "reader": {"status": "unavailable_test_double"},
        "judge": {"status": "unavailable_test_double"},
    }
    raw = [
        {
            "dataset": "locomo",
            "sample_id": sample_id,
            "category": "single_hop",
            "question": "not persisted in capability ledger",
            "expected": "expected",
            "predicted": "expected",
            "context": "context",
            "answerability": "answerable",
            "abstention_correct": None,
            "f1": 1.0,
            "bleu1": 1.0,
            "llm_score": 5,
            "evidence_recall@10": 1.0,
            "evidence_expected_count": 1,
            "evidence_retrieved_count": 1,
            "token_overlap": {
                "predicted_token_count": 1,
                "expected_token_count": 1,
                "common_token_count": 1,
            },
            "expected_sha256": "a" * 64,
            "predicted_sha256": "b" * 64,
            "embedding_query_state": "succeeded",
            "embedding_query_degradation": None,
            "retrieval_execution": [
                {"path": "bm25", "candidates": 1, "accepted": 1, "degradation": None},
                {"path": "vector", "candidates": 1, "accepted": 1, "degradation": None},
            ],
            "verified_vector_execution": True,
            "retrieval_degraded": False,
        }
    ]

    metrics = suite.evaluate(raw)

    assert metrics["sample_accounting"] == {
        "selected_ids": [sample_id],
        "scored_ids": [sample_id],
        "failed_ids": [],
        "excluded_ids": [],
    }
    assert metrics["llm_evidence"]["costs"] == {
        "currency": "USD",
        "accounting": "per_attempt_recomputed_and_budget_reconciled",
        "total_usd": "0",
        "by_role_usd": {},
        "attempt_count": 0,
    }
    rebound = [{**raw[0], "sample_id": "locomo:other"}]
    with pytest.raises(RuntimeError, match="selected and scored"):
        suite.evaluate(rebound)


def test_qa_attempt_cost_is_recomputed_instead_of_trusting_claimed_total():
    evidence = {
        "usd_basis": "actual_usage",
        "usage": {
            "prompt_cache_hit_tokens": 2,
            "prompt_cache_miss_tokens": 3,
            "completion_tokens": 5,
        },
        "pricing_cache_hit_per_million_usd": "1",
        "pricing_cache_miss_per_million_usd": "2",
        "pricing_output_per_million_usd": "3",
        "usd_accounted": "0.000023",
    }
    _verify_attempt_cost(evidence)
    evidence["usd_accounted"] = "0.000024"
    with pytest.raises(RuntimeError, match="cost evidence"):
        _verify_attempt_cost(evidence)


def test_mab_retrieval_requires_answer_bearing_content_not_arbitrary_document_chunk():
    suite = RetrievalSuite(seed=7)
    suite.mab_data = [
        {
            "chunks": [
                "same document but no answer",
                "The launch code is ORCHID-71.",
            ],
            "questions": [
                {
                    "question": "What is the launch code?",
                    "answers": ["ORCHID-71"],
                    "qa_pair_id": "qa-9",
                }
            ],
        }
    ]

    scopes, queries = suite._prepare_mab(limit=None)

    assert queries[0].sample_id == "memoryagentbench_ar:0:qa-9"
    assert len(queries[0].relevances) == 1
    relevant_id = next(iter(queries[0].relevances))
    assert queries[0].relevances[relevant_id] == 1.0
    assert scopes["mab-doc:0"][relevant_id] == "The launch code is ORCHID-71."
    assert len(scopes["mab-doc:0"]) == 2


def test_mab_official_columnar_schema_is_normalized_without_character_splitting():
    normalized = RetrievalSuite._normalize_mab_questions(
        {
            "questions": ["Where?"],
            "answers": [["Paris", "France"]],
            "metadata": {"qa_pair_ids": ["qa-1"]},
        }
    )

    assert normalized == [
        {
            "question": "Where?",
            "answers": ["Paris", "France"],
            "qa_pair_id": "qa-1",
        }
    ]
