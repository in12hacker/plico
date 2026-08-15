"""Five-run paired shadow comparison for retrieval candidates."""

from __future__ import annotations

import hashlib
import json
import math
from pathlib import Path
from typing import Any

import numpy as np

from plico_benchmarks.core.client import PROTOCOL
from plico_benchmarks.core.dogfood_io import (
    canonical_json,
    commit_artifact_directory,
    sha256,
    strict_json_object,
    verify_artifact_directory,
)

_METRICS = {"recall@5", "recall@10", "recall@20", "mrr@10", "ndcg@10"}
_COMPARISON_FILE = "comparison.json"
_COMPARISON_SIDECAR = "comparison.sha256.json"
_COMPARISON_COMMIT_SCHEMA = "plico.benchmark.shadow-comparison-commit/v1"


def compare_retrieval_shadow(
    results: list[dict[str, Any]],
    *,
    candidate: str,
    reference: str,
    metric: str,
    seed: int = 42,
    bootstrap_samples: int = 10_000,
) -> dict[str, Any]:
    """Compare two candidates on exact paired samples across five fresh runs."""
    if len(results) != 5:
        raise ValueError("shadow comparison requires exactly five independent runs")
    if candidate == reference or metric not in _METRICS:
        raise ValueError("shadow comparison candidate/metric is invalid")
    if bootstrap_samples != 10_000:
        raise ValueError("shadow comparison requires the frozen 10000 bootstrap samples")

    run_ids: set[str] = set()
    artifact_contract: dict[str, str] | None = None
    sample_contract: set[tuple[str, str]] | None = None
    paired_by_run: list[list[float]] = []
    source_verified = True
    ordered_results = sorted(
        results,
        key=lambda result: str(result.get("run_manifest", {}).get("run_id", "")),
    )
    for result in ordered_results:
        manifest = result.get("run_manifest")
        if not isinstance(manifest, dict):
            raise ValueError("shadow input is missing its run manifest")
        if manifest.get("protocol") != PROTOCOL or manifest.get("suite") != "retrieval":
            raise ValueError("shadow input protocol/suite mismatch")
        run_id = manifest.get("run_id")
        if not isinstance(run_id, str) or not run_id or run_id in run_ids:
            raise ValueError("shadow input run IDs must be non-empty and unique")
        run_ids.add(run_id)
        current_artifacts = {
            str(artifact.get("role", artifact.get("logical_name"))): str(artifact.get("sha256"))
            for artifact in manifest.get("artifacts", [])
        }
        if len(current_artifacts) != len(manifest.get("artifacts", [])):
            raise ValueError("shadow input artifact roles are incomplete or duplicated")
        if artifact_contract is None:
            artifact_contract = current_artifacts
        elif current_artifacts != artifact_contract:
            raise ValueError("shadow inputs did not consume the exact same artifacts")
        source_verified &= manifest.get("pipeline", {}).get("source_watermark") not in {
            None,
            "unavailable_public_v2",
        }

        rows = result.get("metrics", {}).get("capability_ledger")
        if not isinstance(rows, list):
            raise ValueError("shadow input has no persistent capability ledger")
        indexed: dict[tuple[str, str, str], dict[str, Any]] = {}
        for row in rows:
            if not isinstance(row, dict):
                raise ValueError("shadow capability ledger contains a non-object row")
            key = (str(row.get("dataset")), str(row.get("sample_id")), str(row.get("candidate")))
            if key in indexed:
                raise ValueError("shadow capability ledger contains a duplicate candidate row")
            indexed[key] = row
        samples = {
            (dataset, sample_id)
            for dataset, sample_id, observed_candidate in indexed
            if observed_candidate in {candidate, reference}
        }
        if sample_contract is None:
            sample_contract = samples
        elif samples != sample_contract:
            raise ValueError("shadow inputs do not have an exact stable sample set")
        deltas = []
        for dataset, sample_id in sorted(samples):
            left = indexed.get((dataset, sample_id, candidate))
            right = indexed.get((dataset, sample_id, reference))
            if left is None or right is None:
                raise ValueError("shadow input has an unpaired candidate sample")
            if left.get("status") != "ok" or right.get("status") != "ok":
                raise ValueError("degraded or unsupported rows cannot enter paired inference")
            left_score = left.get(metric)
            right_score = right.get(metric)
            if (
                not _finite_number(left_score)
                or not _finite_number(right_score)
                or not 0 <= float(left_score) <= 1
                or not 0 <= float(right_score) <= 1
            ):
                raise ValueError("shadow input metric is missing or non-finite")
            deltas.append(float(left_score) - float(right_score))
        if not deltas:
            raise ValueError("shadow input has no paired samples")
        paired_by_run.append(deltas)

    rng = np.random.default_rng(seed)
    bootstrapped = np.empty(bootstrap_samples, dtype=np.float64)
    paired_matrix = np.asarray(paired_by_run, dtype=np.float64)
    for iteration in range(bootstrap_samples):
        selected_runs = rng.integers(0, len(paired_by_run), size=len(paired_by_run))
        selected_samples = rng.integers(0, paired_matrix.shape[1], size=paired_matrix.shape[1])
        bootstrapped[iteration] = float(
            np.mean(paired_matrix[np.ix_(selected_runs, selected_samples)])
        )
    all_deltas = paired_matrix.reshape(-1)
    low, high = np.percentile(bootstrapped, [2.5, 97.5])
    run_means = np.asarray([float(np.mean(run)) for run in paired_by_run])
    return {
        "schema": "plico.benchmark.shadow-comparison/v1",
        "protocol": PROTOCOL,
        "suite": "retrieval",
        "candidate": candidate,
        "reference": reference,
        "metric": metric,
        "independent_runs": 5,
        "paired_samples_per_run": len(paired_by_run[0]),
        "run_ids_sha256": _canonical_hash(sorted(run_ids)),
        "input_artifacts": artifact_contract,
        "mean_paired_delta": round(float(np.mean(all_deltas)), 8),
        "run_mean_deltas": [round(value, 8) for value in run_means],
        "between_run_std": round(float(np.std(run_means, ddof=1)), 8),
        "between_run_variance": round(float(np.var(run_means, ddof=1)), 8),
        "ci95_low": round(float(low), 8),
        "ci95_high": round(float(high), 8),
        "bootstrap": {
            "method": "two_way_cluster_run_and_query_percentile_v1",
            "samples": bootstrap_samples,
            "seed": seed,
        },
        "status": "shadow_no_approved_margin",
        "gate_eligible": False,
        "source_watermark_verified": source_verified,
        "comparative_inference": (
            "shadow_only" if source_verified else "exploratory_unverified_source_watermark"
        ),
    }


def load_result(path: Path) -> dict[str, Any]:
    """Deep-verify one explicit committed result directory."""
    from plico_benchmarks.core.result_artifact import verify_result_directory

    return verify_result_directory(path)


def commit_shadow_directory(output: Path, comparison: dict[str, Any]) -> dict[str, Any]:
    payload = canonical_json(comparison)
    sidecar = canonical_json(
        {
            "schema": "plico.benchmark.shadow-comparison-digest/v1",
            "file_name": _COMPARISON_FILE,
            "bytes": len(payload),
            "sha256": sha256(payload),
        }
    )
    commit_artifact_directory(
        output,
        artifact_name=_COMPARISON_FILE,
        sidecar_name=_COMPARISON_SIDECAR,
        artifact=payload,
        sidecar=sidecar,
        commit_schema=_COMPARISON_COMMIT_SCHEMA,
    )
    return verify_shadow_directory(output)


def verify_shadow_directory(output: Path) -> dict[str, Any]:
    payload, sidecar_payload = verify_artifact_directory(
        output,
        artifact_name=_COMPARISON_FILE,
        sidecar_name=_COMPARISON_SIDECAR,
        commit_schema=_COMPARISON_COMMIT_SCHEMA,
    )
    value = strict_json_object(payload)
    if payload != canonical_json(value) or value.get("schema") != (
        "plico.benchmark.shadow-comparison/v1"
    ):
        raise ValueError("shadow comparison artifact is not canonical supported JSON")
    expected_sidecar = {
        "schema": "plico.benchmark.shadow-comparison-digest/v1",
        "file_name": _COMPARISON_FILE,
        "bytes": len(payload),
        "sha256": sha256(payload),
    }
    if strict_json_object(sidecar_payload) != expected_sidecar or sidecar_payload != (
        canonical_json(expected_sidecar)
    ):
        raise ValueError("shadow comparison detached digest is invalid")
    return value


def _finite_number(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, (int, float)) and math.isfinite(value)


def _canonical_hash(value: Any) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()
