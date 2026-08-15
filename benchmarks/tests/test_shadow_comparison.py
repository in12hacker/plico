"""Five-run paired shadow comparison contracts."""

from __future__ import annotations

import copy
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.core.comparison import compare_retrieval_shadow


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
