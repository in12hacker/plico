"""Five-run conversational-QA variance summary."""

from __future__ import annotations

import hashlib
import json
import math
from decimal import Decimal
from typing import Any

import numpy as np

from plico_benchmarks.core.client import PROTOCOL
from plico_benchmarks.core.qa_comparison_validation import (
    QaShadowInput,
    load_qa_shadow_input,
    validate_qa_shadow_inputs,
)

_METRICS = {
    "evidence_recall@10": None,
    "f1": "answerable",
    "bleu1": "answerable",
    "llm_score": "answerable",
    "adversarial_abstention_accuracy": "adversarial_unanswerable",
}

__all__ = ["QaShadowInput", "compare_qa_shadow", "load_qa_shadow_input"]


def compare_qa_shadow(
    inputs: list[QaShadowInput],
    *,
    seed: int = 42,
    bootstrap_samples: int = 10_000,
) -> dict[str, Any]:
    """Summarize fixed-input QA repetitions without creating a release gate."""
    if bootstrap_samples != 10_000:
        raise ValueError("QA shadow comparison requires the frozen 10000 bootstrap samples")
    campaign = validate_qa_shadow_inputs(inputs)
    rng = np.random.default_rng(seed)
    scopes = {}
    for scope, dataset in (
        ("overall", None),
        ("locomo", "locomo"),
        ("longmemeval", "longmemeval"),
    ):
        summaries = {}
        for metric, answerability in _METRICS.items():
            selected = [
                sample_id
                for sample_id in campaign.sample_ids
                if (dataset is None or campaign.sample_contract[sample_id][0] == dataset)
                and (
                    answerability is None or campaign.sample_contract[sample_id][2] == answerability
                )
            ]
            if not selected:
                continue
            matrix = np.asarray(
                [
                    [_metric_value(rows[sample_id], metric) for sample_id in selected]
                    for rows in campaign.rows_by_run
                ],
                dtype=np.float64,
            )
            summaries[metric] = _statistics(
                matrix,
                rng=rng,
                seed=seed,
                bootstrap_samples=bootstrap_samples,
            )
        scopes[scope] = summaries

    fingerprints = {role["system_fingerprint"] for role in campaign.llm_identity["roles"].values()}
    models = {role["response_model"] for role in campaign.llm_identity["roles"].values()}
    total_cost = sum((Decimal(cost) for cost in campaign.costs), Decimal(0))
    return {
        "schema": "plico.benchmark.qa-shadow-comparison/v1",
        "protocol": PROTOCOL,
        "suite": "conversational-qa",
        "independent_runs": 5,
        "samples_per_run": len(campaign.sample_ids),
        "run_ids_sha256": _canonical_hash(campaign.run_ids),
        "input_artifacts": campaign.artifacts,
        "sample_ids_sha256": _canonical_hash(campaign.sample_ids),
        "implementation": campaign.implementation,
        "suite_config_sha256": _canonical_hash(campaign.suite_config),
        "deepseek": {
            "response_models": sorted(models),
            "system_fingerprint": next(iter(fingerprints)),
            "role_config_sha256": _canonical_hash(campaign.role_configs),
            "identity_scope": "same_unattested_alias_fingerprint_across_five_runs",
        },
        "embedding_runtime": campaign.embedding_runtime,
        "attempt_counts_by_run": list(campaign.attempt_counts),
        "costs": {
            "currency": "USD",
            "total_usd": format(total_cost, "f"),
            "per_run_usd": list(campaign.costs),
        },
        "metrics": scopes,
        "bootstrap": {
            "method": "two_way_cluster_run_and_sample_percentile_v1",
            "samples": bootstrap_samples,
            "seed": seed,
        },
        "status": "qa_shadow_variance_only",
        "gate_eligible": False,
        "source_watermark_verified": campaign.source_watermark_verified,
        "comparative_inference": "shadow_only_not_a_release_gate",
    }


def _metric_value(row: dict[str, Any], metric: str) -> float:
    if metric == "adversarial_abstention_accuracy":
        value = row.get("abstention_correct")
        if not isinstance(value, bool):
            raise ValueError("QA shadow abstention evidence is not boolean")
        return float(value)
    value = row.get(metric)
    if not _finite_number(value):
        raise ValueError(f"QA shadow metric {metric} is missing or non-finite")
    numeric = float(value)
    upper = 5.0 if metric == "llm_score" else 1.0
    lower = 1.0 if metric == "llm_score" else 0.0
    if not lower <= numeric <= upper:
        raise ValueError(f"QA shadow metric {metric} is outside its typed range")
    return numeric


def _statistics(
    matrix: np.ndarray,
    *,
    rng: np.random.Generator,
    seed: int,
    bootstrap_samples: int,
) -> dict[str, Any]:
    if matrix.shape[0] != 5 or matrix.shape[1] <= 0 or not np.all(np.isfinite(matrix)):
        raise ValueError("QA shadow metric matrix is invalid")
    bootstrapped = np.empty(bootstrap_samples, dtype=np.float64)
    for iteration in range(bootstrap_samples):
        selected_runs = rng.integers(0, matrix.shape[0], size=matrix.shape[0])
        selected_samples = rng.integers(0, matrix.shape[1], size=matrix.shape[1])
        bootstrapped[iteration] = float(np.mean(matrix[np.ix_(selected_runs, selected_samples)]))
    run_means = np.mean(matrix, axis=1)
    low, high = np.percentile(bootstrapped, [2.5, 97.5])
    return {
        "mean": round(float(np.mean(matrix)), 8),
        "run_means": [round(float(value), 8) for value in run_means],
        "between_run_std": round(float(np.std(run_means, ddof=1)), 8),
        "between_run_variance": round(float(np.var(run_means, ddof=1)), 8),
        "ci95_low": round(float(low), 8),
        "ci95_high": round(float(high), 8),
        "observations_per_run": int(matrix.shape[1]),
        "bootstrap_samples": bootstrap_samples,
        "bootstrap_seed": seed,
    }


def _finite_number(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value)


def _canonical_hash(value: Any) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()
