"""Fail-closed run integrity records for benchmark artifacts."""

from __future__ import annotations

import hashlib
import os
import platform
import subprocess
import uuid
from pathlib import Path
from typing import Any

from plico_benchmarks.core.client import PROTOCOL
from plico_benchmarks.core.retrieval_execution import verified_vector_execution

_SUCCESS_STATES = {None, "measured", "model_refusal", "no_answer", "ok"}
_FAILURE_STATES = {
    "index_not_ready",
    "infra_error",
    "invalid_input",
    "judge_error",
    "partial",
    "timeout",
}
_EXCLUDED_STATES = {"unsupported", "degraded"}
_RUN_CLASSES = {"regression", "research", "release_evidence", "official"}
_SUITE_RUN_CLASSES = {
    "conversational-qa": {"regression", "research"},
    "memory-recall-lexical": {"regression", "research"},
    "retrieval": {"regression", "research"},
    "performance": {"regression", "research"},
    "v1b-release": {"release_evidence"},
}


def resolve_run_class(suite: str) -> str:
    """Resolve one typed run class before a suite performs any external action."""
    run_class = os.environ.get("PLICO_BENCH_RUN_CLASS", "regression").strip()
    if run_class not in _RUN_CLASSES:
        raise ValueError(f"unsupported benchmark run class: {run_class!r}")
    if run_class == "official":
        raise RuntimeError(
            "official conformance is unsupported until upstream provenance, full "
            "cardinality, and official adapters are pinned"
        )
    allowed = _SUITE_RUN_CLASSES.get(suite, {"regression", "research"})
    if run_class not in allowed:
        raise RuntimeError(f"run class {run_class!r} is not valid for suite {suite!r}")
    return run_class


def validate_real_embedding_requirement(raw_results: list[dict[str, Any]]) -> None:
    """Reject real-embedding runs that used a stub or observed degradation."""
    if os.environ.get("PLICO_BENCH_REQUIRE_REAL_EMBEDDING", "").lower() not in {
        "1",
        "true",
        "yes",
    }:
        return

    backend = os.environ.get("EMBEDDING_BACKEND", "").strip().lower()
    if backend in {"", "stub", "none", "disabled", "unknown"}:
        raise RuntimeError("real embedding run requires an explicit non-stub EMBEDDING_BACKEND")

    for result in raw_results:
        operation = result.get("operation")
        if operation in {
            "object.search_warm_repeated",
            "object.search_query_cold_unique",
        }:
            states = result.get("embedding_query_states")
            if states != {"succeeded": result.get("count")}:
                raise RuntimeError(
                    f"real embedding run observed non-succeeded query embedding: {states}"
                )
            ledger = result.get("query_execution_ledger")
            if (
                not isinstance(ledger, list)
                or len(ledger) != result.get("count")
                or any(
                    item.get("status") != "ok"
                    or not verified_vector_execution(
                        item.get("embedding_query_state"),
                        item.get("retrieval_execution", []),
                    )
                    for item in ledger
                    if isinstance(item, dict)
                )
                or any(not isinstance(item, dict) for item in ledger)
            ):
                raise RuntimeError(
                    "real embedding run lacks exact per-query vector execution evidence"
                )
        if operation == "projection.memory_embedding_catch_up" and (
            result.get("ready") != result.get("entries_observed")
            or result.get("failed")
            or result.get("timeout")
        ):
            raise RuntimeError("real embedding run did not reach a complete Ready watermark")


def build_run_manifest(
    *,
    run_id: str,
    suite: str,
    requested: int | None,
    actual: int,
    seed: int,
    input_artifacts: list[dict[str, Any]],
    raw_results: list[dict[str, Any]],
    source_watermark: dict[str, Any] | str,
    external_evidence: list[dict[str, Any]],
    run_class: str,
    llm_evidence: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Build a machine-checkable single-run ledger without statistical claims."""
    failures = []
    excluded = 0
    for index, result in enumerate(raw_results):
        state = result.get("status")
        if state in _SUCCESS_STATES:
            continue
        if state in _EXCLUDED_STATES:
            excluded += _result_weight(result)
            continue
        if state not in _FAILURE_STATES:
            raise ValueError(f"unknown benchmark result status: {state!r}")
        weight = _result_weight(result)
        failures.append(
            {
                "result_index": index,
                "operation": result.get("operation"),
                "dataset": result.get("dataset"),
                "status": state,
                "count": weight,
            }
        )

    failed = sum(item["count"] for item in failures)
    if failed + excluded > actual:
        raise ValueError("failure ledger exceeds evaluated sample count")
    scored = actual - failed - excluded
    fault_ledger = [
        {
            "operation": result.get("operation"),
            "injected_fault": result.get("injected_fault"),
            "count": result.get("count"),
            "observations": result.get("fault_cases", []),
        }
        for result in raw_results
        if result.get("fault_injection") is True
    ]
    verified_llm = (
        {
            "status": "verified_attempt_integrity_not_cross_run_comparability",
            "journal": llm_evidence["journal"],
            "identity": llm_evidence["identity"],
            "costs": llm_evidence["costs"],
        }
        if isinstance(llm_evidence, dict)
        and llm_evidence.get("journal", {}).get("status") == "verified_complete"
        and llm_evidence.get("identity", {}).get("status")
        == "verified_attempt_integrity_not_cross_run_comparability"
        else {"status": "unavailable_without_verified_attempt_journal"}
    )
    manifest = {
        "schema_version": "plico.memory-eval-run/v1",
        "protocol": PROTOCOL,
        "schemas": {
            "result": (
                "plico.benchmark-result/v6"
                if suite == "conversational-qa"
                else "plico.benchmark-result/v4"
            ),
            "canonical_ledger_root": "plico.memory.root/v1",
            "canonical_revision": "plico.memory.revision/v1",
            "migration_source_manifest": "plico.memory.migration-source-manifest/v1",
            "migration_target_manifest": "plico.memory.migration-target-manifest/v1",
            "projection_manifest_root": "plico.projection.manifest-root/v1",
            "projection_manifest_record": "plico.projection.manifest-record/v1",
            "projection_current_view": "plico.projection.current-view/v1",
            "projection_embedding_artifact": "plico.projection.embedding-artifact/v1",
        },
        "run_id": run_id,
        "run_class": run_class,
        "suite": suite,
        "sampling": {
            "requested": requested,
            "actual": actual,
            "scored": scored,
            "failed": failed,
            "excluded": excluded,
            "seed": seed,
        },
        "independent_runs_observed": 1,
        "comparative_inference": "not_available_single_run",
        "artifacts": input_artifacts,
        "external_evidence": external_evidence,
        "artifact_binding": {
            "method": "detached_run_manifest_sidecar_sha256",
            "status": "bound_when_result_is_saved",
        },
        "git_state": _git_state(),
        "hardware": _hardware(),
        "pipeline": {
            "source_watermark": source_watermark,
            "embedding_identity": {"status": "unverified_without_same_run_provider_snapshot"},
            "llm_identity": verified_llm,
        },
        "failure_ledger": failures,
        "fault_ledger": fault_ledger,
    }
    validate_run_manifest(manifest)
    return manifest


def validate_run_manifest(manifest: dict[str, Any]) -> None:
    """Validate count conservation and the absence of unsupported claims."""
    if manifest.get("schema_version") != "plico.memory-eval-run/v1":
        raise ValueError("unsupported run manifest schema")
    if manifest.get("protocol") != PROTOCOL:
        raise ValueError("run manifest protocol mismatch")
    run_id = manifest.get("run_id")
    try:
        canonical_run_id = str(uuid.UUID(str(run_id)))
    except (ValueError, AttributeError) as error:
        raise ValueError("run manifest run_id must be a canonical UUID") from error
    if run_id != canonical_run_id:
        raise ValueError("run manifest run_id must use canonical hyphenated form")
    run_class = manifest.get("run_class")
    suite = manifest.get("suite")
    if run_class not in _RUN_CLASSES or run_class == "official":
        raise ValueError("run manifest run class is unsupported")
    if run_class not in _SUITE_RUN_CLASSES.get(str(suite), {"regression", "research"}):
        raise ValueError("run manifest suite/run class combination is invalid")
    result_schema = manifest.get("schemas", {}).get("result")
    supported_result_schemas = (
        {
            "plico.benchmark-result/v4",
            "plico.benchmark-result/v5",
            "plico.benchmark-result/v6",
        }
        if suite == "conversational-qa"
        else {"plico.benchmark-result/v4"}
    )
    if not (suite == "v1b-release" and result_schema is None) and (
        result_schema not in supported_result_schemas
    ):
        raise ValueError("run manifest result schema is unsupported")
    sampling = manifest["sampling"]
    accounted = sampling["scored"] + sampling["failed"] + sampling["excluded"]
    if sampling["actual"] != accounted:
        raise ValueError("actual must equal scored + failed + excluded")
    if manifest["independent_runs_observed"] != 1:
        raise ValueError("one suite execution records exactly one independent run")
    if manifest["comparative_inference"] != "not_available_single_run":
        raise ValueError("a single run cannot claim comparative inference")
    if "source_watermark" not in manifest.get("pipeline", {}):
        raise ValueError("run manifest must bind a source watermark")
    artifact_identities = set()
    for artifact in manifest.get("artifacts", []):
        identity = artifact.get("role", artifact.get("logical_name"))
        digest = artifact.get("sha256")
        if not isinstance(identity, str) or not identity or identity in artifact_identities:
            raise ValueError("run manifest artifact identities must be unique")
        if (
            not isinstance(digest, str)
            or len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
        ):
            raise ValueError("run manifest artifact digest must be lowercase SHA-256")
        artifact_identities.add(identity)


def _result_weight(result: dict[str, Any]) -> int:
    count = result.get("failure_count", result.get("count", 1))
    if isinstance(count, bool) or not isinstance(count, int) or count < 0:
        raise ValueError(f"result count must be a non-negative integer: {count!r}")
    return count


def _git_state() -> dict[str, Any]:
    """Bind a run to source state without exposing changed file paths."""
    project_root = Path(__file__).resolve().parents[4]
    pathspec = ["--", ".", ":(exclude)benchmarks/results/**"]
    try:
        commit = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=False,
        ).stdout.strip()
        status = subprocess.run(
            ["git", "status", "--porcelain=v1", "-z", "--untracked-files=all", *pathspec],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=False,
        ).stdout
        tracked_diff = subprocess.run(
            ["git", "diff", "--binary", "HEAD", *pathspec],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=False,
        ).stdout
        untracked = subprocess.run(
            ["git", "ls-files", "--others", "--exclude-standard", "-z", *pathspec],
            cwd=project_root,
            check=True,
            capture_output=True,
            text=False,
        ).stdout.split(b"\0")
    except (OSError, subprocess.CalledProcessError):
        return {"state": "unavailable"}
    worktree = hashlib.sha256()
    worktree.update(b"plico.git-worktree.v1\0")
    worktree.update(status)
    worktree.update(tracked_diff)
    untracked_count = 0
    for encoded_path in sorted(path for path in untracked if path):
        path = project_root / os.fsdecode(encoded_path)
        if not path.is_file() or path.is_symlink():
            continue
        untracked_count += 1
        worktree.update(encoded_path)
        worktree.update(b"\0")
        worktree.update(path.read_bytes())
        worktree.update(b"\0")
    return {
        "state": "available",
        "commit": commit.decode("ascii"),
        "dirty": bool(status),
        "status_entry_count": sum(1 for item in status.split(b"\0") if item),
        "status_sha256": hashlib.sha256(status).hexdigest(),
        "untracked_regular_file_count": untracked_count,
        "worktree_digest_sha256": worktree.hexdigest(),
        "digest_rule": (
            "sha256(domain || porcelain-z || git-diff-binary-HEAD || "
            "sorted(untracked-path || bytes)); benchmarks/results excluded"
        ),
    }


def _hardware() -> dict[str, Any]:
    logical_cpus = os.cpu_count()
    memory_bytes: int | None = None
    try:
        memory_bytes = os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES")
    except (AttributeError, OSError, ValueError):
        pass
    return {
        "operating_system": platform.system().lower() or "unknown",
        "kernel_release": platform.release() or "unknown",
        "architecture": platform.machine() or "unknown",
        "logical_cpus": logical_cpus,
        "memory_bytes": memory_bytes,
    }
