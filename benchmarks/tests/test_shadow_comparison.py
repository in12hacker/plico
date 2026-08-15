"""Five-run paired shadow comparison contracts."""

from __future__ import annotations

import copy
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.core.comparison import (
    commit_shadow_directory,
    compare_retrieval_shadow,
)
from plico_benchmarks.core.qa_comparison import QaShadowInput, compare_qa_shadow


def _result(run: int) -> dict:
    rows = []
    for sample, plico_score, bm25_score in (("q1", 1.0, 0.5), ("q2", 0.0, 0.25)):
        for candidate, score in (
            ("plico_object_search", plico_score),
            ("bm25_only", bm25_score),
            ("vector_only", None),
        ):
            row = {
                "dataset": "beir_scifact",
                "sample_id": f"beir_scifact:{sample}",
                "stratum": "scifact",
                "candidate": candidate,
                "domain": "plico_object_projection",
                "status": "unsupported" if score is None else "ok",
            }
            if score is not None:
                row["recall@10"] = score
            rows.append(row)
    return {
        "run_manifest": {
            "protocol": "plico.personal.v2",
            "suite": "retrieval",
            "run_id": f"run-{run}",
            "artifacts": [
                {"role": "dataset", "sha256": "a" * 64},
                {"role": "retrieval_candidate_matrix", "sha256": "b" * 64},
            ],
            "pipeline": {"source_watermark": "unavailable_public_v2"},
        },
        "metrics": {"capability_ledger": rows},
    }


def test_shadow_comparison_is_deterministic_and_never_promotes_unverified_source():
    results = [_result(index) for index in range(5)]

    first = compare_retrieval_shadow(
        results,
        candidate="plico_object_search",
        reference="bm25_only",
        metric="recall@10",
    )
    second = compare_retrieval_shadow(
        results,
        candidate="plico_object_search",
        reference="bm25_only",
        metric="recall@10",
    )

    assert first == second
    assert first["independent_runs"] == 5
    assert first["paired_samples_per_run"] == 2
    assert first["mean_paired_delta"] == pytest.approx(0.125)
    assert first["status"] == "shadow_no_approved_margin"
    assert first["gate_eligible"] is False
    assert first["comparative_inference"] == "exploratory_unverified_source_watermark"


@pytest.mark.parametrize("mutation", ["duplicate_run", "sample_rebound", "degraded"])
def test_shadow_comparison_rejects_nonindependent_or_unpaired_inputs(mutation):
    results = [_result(index) for index in range(5)]
    if mutation == "duplicate_run":
        results[4]["run_manifest"]["run_id"] = "run-0"
    elif mutation == "sample_rebound":
        results[4]["metrics"]["capability_ledger"][0]["sample_id"] = "beir_scifact:other"
    else:
        results[4]["metrics"]["capability_ledger"][0]["status"] = "degraded"

    with pytest.raises(ValueError):
        compare_retrieval_shadow(
            copy.deepcopy(results),
            candidate="plico_object_search",
            reference="bm25_only",
            metric="recall@10",
        )


def test_shadow_comparison_requires_exactly_five_runs():
    with pytest.raises(ValueError, match="exactly five"):
        compare_retrieval_shadow(
            [_result(index) for index in range(4)],
            candidate="plico_object_search",
            reference="bm25_only",
            metric="recall@10",
        )


def _qa_input(run: int) -> QaShadowInput:
    fingerprint = "f" * 32
    identity_role = {
        "provider": "deepseek",
        "api_origin": "https://api.deepseek.com",
        "requested_model_alias": "deepseek-v4-flash",
        "official_model_version": None,
        "model_revision_attestation": "unattested_alias",
        "response_model": "deepseek-v4-flash",
        "system_fingerprint": fingerprint,
        "identity_class": "unattested_alias_requires_same_fingerprint_and_five_runs",
        "cross_run_comparability": ("requires_same_system_fingerprint_and_five_run_variance_ci"),
        "thinking": "disabled",
        "reasoning_effort": None,
        "temperature": 0.0,
        "top_p": 1.0,
        "generation_seed": "provider_unavailable",
    }
    selected = ["locomo:adversarial", "longmemeval:answerable"]
    ledger = [
        {
            "sample_id": selected[0],
            "dataset": "locomo",
            "stratum": "adversarial",
            "status": "ok",
            "answerability": "adversarial_unanswerable",
            "abstention_correct": run % 2 == 0,
            "f1": None,
            "bleu1": None,
            "llm_score": None,
            "evidence_recall@10": 0.5,
            "embedding_query_state": "succeeded",
            "embedding_query_degradation": None,
            "retrieval_degraded": False,
            "verified_vector_execution": True,
        },
        {
            "sample_id": selected[1],
            "dataset": "longmemeval",
            "stratum": "multi-session",
            "status": "ok",
            "answerability": "answerable",
            "abstention_correct": None,
            "f1": 0.2 + run * 0.1,
            "bleu1": 0.1,
            "llm_score": 4,
            "evidence_recall@10": 1.0,
            "embedding_query_state": "succeeded",
            "embedding_query_degradation": None,
            "retrieval_degraded": False,
            "verified_vector_execution": True,
        },
    ]
    result = {
        "config": {
            "samples": 2,
            "run_id": f"qa-run-{run}",
            "sampling_profile": "regression",
            "sampling_strategy": "deterministic_sha256_stratified_v1",
        },
        "run_manifest": {
            "protocol": "plico.personal.v2",
            "suite": "conversational-qa",
            "run_class": "research",
            "run_id": f"qa-run-{run}",
            "schemas": {"result": "plico.benchmark-result/v5"},
            "sampling": {"actual": 2, "scored": 2, "failed": 0, "excluded": 0},
            "artifacts": [
                {"logical_name": "locomo", "sha256": "a" * 64},
                {"logical_name": "longmemeval", "sha256": "b" * 64},
                {
                    "role": "conversational_qa_sample_selection",
                    "sha256": "c" * 64,
                },
            ],
            "git_state": {
                "commit": "d" * 40,
                "dirty": False,
                "worktree_digest_sha256": "e" * 64,
            },
            "pipeline": {"source_watermark": "unavailable_public_v2"},
        },
        "metrics": {
            "capability_ledger": ledger,
            "sample_accounting": {
                "selected_ids": selected,
                "scored_ids": selected,
                "failed_ids": [],
                "excluded_ids": [],
            },
            "retrieval_runtime": {
                "configured_embedding_backend": "openai",
                "active_embedding_provider": "unavailable",
                "embedding_provider_state": "unavailable",
                "provider_identity_scope": "object_execution_only_unattested_provider",
                "requirement": "real_non_stub_vector_per_query",
                "cognitive_pipeline": {"max_in_flight": 4, "queue_capacity": 8192},
                "ingest_watermark": {
                    "accepted": 2,
                    "accepted_delta": 2,
                    "completed": 2,
                    "in_flight": 0,
                },
                "ingest_outcomes": {
                    "submitted": 2,
                    "unique_cids": 2,
                    "duplicate_cids": 0,
                    "queued_accepted_attempts": 2,
                    "inline_document_attempts": 0,
                    "document_vector_succeeded_attempts": 2,
                    "document_lexical_degraded_attempts": 0,
                    "task_failed_attempts": 0,
                    "other_succeeded_attempts": 0,
                },
            },
            "llm_evidence": {
                "journal": {
                    "status": "verified_complete",
                    "attempt_count": 2,
                    "finalized_attempt_count": 2,
                    "incomplete_pending_files": 0,
                    "incomplete_prepared_attempts": 0,
                },
                "costs": {"total_usd": "0.01"},
                "identity": {
                    "status": "verified_attempt_integrity_not_cross_run_comparability",
                    "roles": {
                        "reader": copy.deepcopy(identity_role),
                        "judge": copy.deepcopy(identity_role),
                    },
                },
            },
        },
    }
    role_configs = tuple(
        {
            "run_id": f"qa-run-{run}",
            "role": role,
            "provider": "deepseek",
            "requested_model_alias": "deepseek-v4-flash",
            "budget_max_usd": "0.10" if role == "reader" else "0.15",
        }
        for role in ("reader", "judge")
    )
    return QaShadowInput(result=result, role_configs=role_configs)


def test_qa_shadow_summarizes_fixed_five_run_variance_and_never_gates(tmp_path):
    comparison = compare_qa_shadow([_qa_input(index) for index in range(5)])

    assert comparison["independent_runs"] == 5
    assert comparison["samples_per_run"] == 2
    assert comparison["metrics"]["overall"]["f1"]["mean"] == pytest.approx(0.4)
    assert comparison["metrics"]["overall"]["f1"]["between_run_std"] == pytest.approx(0.15811388)
    assert comparison["deepseek"]["system_fingerprint"] == "f" * 32
    assert comparison["costs"]["total_usd"] == "0.05"
    assert comparison["status"] == "qa_shadow_variance_only"
    assert comparison["gate_eligible"] is False

    committed = commit_shadow_directory(tmp_path / "qa-shadow", comparison)
    assert committed == comparison

    comparison["gate_eligible"] = True
    with pytest.raises(ValueError, match="release-gate eligibility"):
        commit_shadow_directory(tmp_path / "invalid-qa-shadow", comparison)


@pytest.mark.parametrize(
    "mutation",
    [
        "fingerprint",
        "role_config",
        "sample_set",
        "degraded",
        "pipeline_config",
        "watermark_missing",
        "in_flight",
    ],
)
def test_qa_shadow_rejects_changed_or_unverified_campaign_inputs(mutation):
    inputs = [_qa_input(index) for index in range(5)]
    if mutation == "fingerprint":
        inputs[4].result["metrics"]["llm_evidence"]["identity"]["roles"]["judge"][
            "system_fingerprint"
        ] = "0" * 32
    elif mutation == "role_config":
        inputs[4].role_configs[0]["budget_max_usd"] = "9.99"
    elif mutation == "sample_set":
        inputs[4].result["metrics"]["sample_accounting"]["selected_ids"][0] = "locomo:other"
    elif mutation == "degraded":
        inputs[4].result["metrics"]["capability_ledger"][0]["retrieval_degraded"] = True
    elif mutation == "pipeline_config":
        inputs[4].result["metrics"]["retrieval_runtime"]["cognitive_pipeline"]["queue_capacity"] = (
            1024
        )
    elif mutation == "watermark_missing":
        del inputs[4].result["metrics"]["retrieval_runtime"]["ingest_watermark"]
    else:
        inputs[4].result["metrics"]["retrieval_runtime"]["ingest_watermark"]["in_flight"] = 1

    with pytest.raises(ValueError):
        compare_qa_shadow(inputs)
