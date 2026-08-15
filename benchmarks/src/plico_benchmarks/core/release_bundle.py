"""Verified no-clobber V1-B/P3-A release evidence bundle."""

from __future__ import annotations

import os
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator, model_validator

from plico_benchmarks.core.client import PROTOCOL
from plico_benchmarks.core.dogfood_evidence import read_verified_dogfood_evidence
from plico_benchmarks.core.dogfood_io import (
    canonical_json,
    commit_artifact_directory,
    sha256,
    strict_json_object,
    verify_artifact_directory,
)
from plico_benchmarks.core.dogfood_schema import canonical_uuid, safe_label
from plico_benchmarks.core.result_artifact import (
    RESULT_FILE,
    RUN_MANIFEST_FILE,
    read_verified_result,
)

RELEASE_FILE = "release.json"
RELEASE_SIDECAR = "release.sha256.json"
RELEASE_DIGEST_SCHEMA = "plico.v1b.release-evidence-digest/v3"
RELEASE_COMMIT_SCHEMA = "plico.v1b.release-evidence-commit/v3"


class _StrictReleaseModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


def _digest(value: str) -> str:
    if len(value) != 64 or any(character not in "0123456789abcdef" for character in value):
        raise ValueError("release binding must be a lowercase SHA-256 digest")
    return value


def _file_name(value: str) -> str:
    if not value or Path(value).name != value or value in {".", ".."}:
        raise ValueError("release binding file name must be one safe basename")
    return value


class ReleaseSampling(_StrictReleaseModel):
    actual: int = Field(ge=0)
    scored: int = Field(ge=0)
    failed: Literal[0]
    excluded: Literal[0]

    @model_validator(mode="after")
    def conserved(self) -> ReleaseSampling:
        if self.actual != self.scored + self.failed + self.excluded:
            raise ValueError("release sampling is not conserved")
        return self


class ReleaseBenchmark(_StrictReleaseModel):
    run_id: str
    protocol: Literal["plico.personal.v2"]
    result_file_name: str
    result_bytes: int = Field(gt=0)
    result_sha256: str
    run_manifest_file_name: str
    run_manifest_bytes: int = Field(gt=0)
    run_manifest_sha256: str
    sampling: ReleaseSampling
    source_watermark: dict[str, Any] | str
    fault_observation_count: int = Field(ge=0)
    evidence_ledger_count: int = Field(gt=0)

    _result_hash = field_validator("result_sha256")(_digest)
    _manifest_hash = field_validator("run_manifest_sha256")(_digest)
    _result_name = field_validator("result_file_name")(_file_name)
    _manifest_name = field_validator("run_manifest_file_name")(_file_name)

    @field_validator("run_id")
    @classmethod
    def safe_run_id(cls, value: str) -> str:
        return safe_label(value)

    @field_validator("source_watermark")
    @classmethod
    def nonempty_source(cls, value: dict[str, Any] | str) -> dict[str, Any] | str:
        if value in ({}, ""):
            raise ValueError("release source watermark is empty")
        return value


class ReleaseDogfood(_StrictReleaseModel):
    bundle_run_id: str
    artifact_file_name: Literal["evidence.json"]
    bytes: int = Field(gt=0)
    sha256: str
    real_llm_workflow_run_id: str
    real_llm_trace_sha256: str
    real_llm_model: str
    plicod_sha256: str
    plico_source_aggregate_sha256: str
    plico_agents_source_aggregate_sha256: str
    public_request_count: Literal[14]
    disconnect_request_count: Literal[7]
    v1_zero_state_verified: Literal[True]
    post_restart_typed_responses_verified: Literal[True]

    _bundle_uuid = field_validator("bundle_run_id")(canonical_uuid)
    _reader_uuid = field_validator("real_llm_workflow_run_id")(canonical_uuid)
    _artifact_hash = field_validator("sha256")(_digest)
    _trace_hash = field_validator("real_llm_trace_sha256")(_digest)
    _binary_hash = field_validator("plicod_sha256")(_digest)
    _plico_hash = field_validator("plico_source_aggregate_sha256")(_digest)
    _agents_hash = field_validator("plico_agents_source_aggregate_sha256")(_digest)

    @field_validator("real_llm_model")
    @classmethod
    def model_label(cls, value: str) -> str:
        return safe_label(value)


class ReleaseBinaryBinding(_StrictReleaseModel):
    plicod_sha256: str

    _binary_hash = field_validator("plicod_sha256")(_digest)


class ReleaseClaims(_StrictReleaseModel):
    independent_runs_are_linked_not_merged: Literal[True]
    comparative_inference: Literal["not_available_single_run"]
    thermal_complete: Literal[False]
    public_protocol_exact_14: Literal[True]
    memory_embedding_control_plane_supported: Literal[True]
    memory_embedding_retrieval_supported: Literal[False]
    memory_vector_recall_supported: Literal[False]
    memory_hybrid_recall_supported: Literal[False]
    canonical_ack_independent_of_projection_availability: Literal[True]


class ReleaseBundle(_StrictReleaseModel):
    schema_id: Literal["plico.v1b.release-evidence-bundle/v3"] = Field(alias="schema")
    bundle_run_id: str
    generated_at_utc: str
    scope: Literal["single local personal vault evidence; no tenant or organization semantics"]
    attestation_boundary: Literal["local_artifact_integrity_not_external_cryptographic_attestation"]
    benchmark: ReleaseBenchmark
    dogfood: ReleaseDogfood
    binary_binding: ReleaseBinaryBinding
    claims: ReleaseClaims

    _bundle_uuid = field_validator("bundle_run_id")(canonical_uuid)

    @field_validator("generated_at_utc")
    @classmethod
    def timestamp(cls, value: str) -> str:
        try:
            parsed = datetime.strptime(value, "%Y-%m-%dT%H:%M:%SZ")
        except ValueError as error:
            raise ValueError("release timestamp is not canonical UTC seconds") from error
        if parsed.strftime("%Y-%m-%dT%H:%M:%SZ") != value:
            raise ValueError("release timestamp is not canonical UTC seconds")
        return value

    @model_validator(mode="after")
    def exact_bindings(self) -> ReleaseBundle:
        if self.binary_binding.plicod_sha256 != self.dogfood.plicod_sha256:
            raise ValueError("release bundle binary bindings differ")
        return self


def build_v1b_release_bundle(
    *, benchmark_result: Path, dogfood_bundle: Path, output: Path
) -> dict[str, Any]:
    """Bind benchmark and dogfood evidence into one committed owner-only directory."""
    _reject_output_overlap(output, (benchmark_result, dogfood_bundle))
    result_bytes, manifest_bytes, result = read_verified_result(benchmark_result)
    manifest = strict_json_object(manifest_bytes)
    dogfood_bytes, verified_dogfood = read_verified_dogfood_evidence(dogfood_bundle)
    dogfood = verified_dogfood.model_dump(mode="json", by_alias=True)

    embedded = result.get("run_manifest")
    detached_embedded = {key: value for key, value in manifest.items() if key != "result_artifact"}
    if not isinstance(embedded, dict) or embedded != detached_embedded:
        raise ValueError("benchmark embedded and detached run manifests differ")
    if embedded.get("protocol") != PROTOCOL:
        raise ValueError("benchmark release run used an unsupported public protocol")
    result_artifact = manifest.get("result_artifact")
    if not isinstance(result_artifact, dict) or result_artifact.get("sha256") != sha256(
        result_bytes
    ):
        raise ValueError("benchmark detached manifest does not bind result bytes")
    if result_artifact.get("bytes") != len(result_bytes):
        raise ValueError("benchmark detached manifest result byte count mismatch")
    sampling = embedded.get("sampling", {})
    actual = sampling.get("actual")
    scored = sampling.get("scored")
    failed = sampling.get("failed")
    excluded = sampling.get("excluded")
    if (
        not all(
            isinstance(value, int) and not isinstance(value, bool) and value >= 0
            for value in (actual, scored, failed, excluded)
        )
        or actual != scored + failed + excluded
        or failed != 0
        or excluded != 0
    ):
        raise ValueError("benchmark sampling ledger is not a zero-failure conservation")
    if embedded.get("failure_ledger"):
        raise ValueError("benchmark release run contains failures")

    reader = dogfood["real_llm_reader"]
    external = embedded.get("external_evidence")
    if not isinstance(external, list) or len(external) != 1:
        raise ValueError("benchmark must link exactly one external reader run")
    if (
        external[0].get("workflow_run_id") != reader["workflow_run_id"]
        or external[0].get("trace_sha256") != reader["trace_sha256"]
    ):
        raise ValueError("benchmark external reader link does not match dogfood evidence")
    benchmark_plicod = _artifact_digest(embedded, "plicod_binary")
    dogfood_plicod = dogfood["build"]["plicod_binary"]["sha256"]
    if benchmark_plicod != dogfood_plicod:
        raise ValueError("benchmark and dogfood did not execute the same plicod binary")

    bundle = {
        "schema": "plico.v1b.release-evidence-bundle/v3",
        "bundle_run_id": str(uuid.uuid4()),
        "generated_at_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "scope": "single local personal vault evidence; no tenant or organization semantics",
        "attestation_boundary": "local_artifact_integrity_not_external_cryptographic_attestation",
        "benchmark": {
            "run_id": embedded["run_id"],
            "protocol": embedded["protocol"],
            "result_file_name": RESULT_FILE,
            "result_bytes": len(result_bytes),
            "result_sha256": sha256(result_bytes),
            "run_manifest_file_name": RUN_MANIFEST_FILE,
            "run_manifest_bytes": len(manifest_bytes),
            "run_manifest_sha256": sha256(manifest_bytes),
            "sampling": embedded["sampling"],
            "source_watermark": embedded["pipeline"]["source_watermark"],
            "fault_observation_count": sum(
                int(item.get("count", 0)) for item in embedded.get("fault_ledger", [])
            ),
            "evidence_ledger_count": len(result.get("evidence_ledger", [])),
        },
        "dogfood": {
            "bundle_run_id": dogfood["bundle_run_id"],
            "artifact_file_name": "evidence.json",
            "bytes": len(dogfood_bytes),
            "sha256": sha256(dogfood_bytes),
            "real_llm_workflow_run_id": reader["workflow_run_id"],
            "real_llm_trace_sha256": reader["trace_sha256"],
            "real_llm_model": reader["model"],
            "plicod_sha256": dogfood_plicod,
            "plico_source_aggregate_sha256": dogfood["build"]["plico_source_manifest"][
                "aggregate_sha256"
            ],
            "plico_agents_source_aggregate_sha256": dogfood["build"][
                "plico_agents_source_manifest"
            ]["aggregate_sha256"],
            "public_request_count": dogfood["daemon_trace"]["public_request_count"],
            "disconnect_request_count": dogfood["daemon_trace"]["disconnect_request_count"],
            "v1_zero_state_verified": dogfood["v1_zero_state"]["unchanged"],
            "post_restart_typed_responses_verified": True,
        },
        "binary_binding": {"plicod_sha256": benchmark_plicod},
        "claims": {
            "independent_runs_are_linked_not_merged": True,
            "comparative_inference": "not_available_single_run",
            "thermal_complete": False,
            "public_protocol_exact_14": True,
            "memory_embedding_control_plane_supported": True,
            "memory_embedding_retrieval_supported": False,
            "memory_vector_recall_supported": False,
            "memory_hybrid_recall_supported": False,
            "canonical_ack_independent_of_projection_availability": True,
        },
    }
    typed_bundle = ReleaseBundle.model_validate(bundle)
    encoded = canonical_json(typed_bundle.model_dump(mode="json", by_alias=True))
    sidecar = canonical_json(
        {
            "schema": RELEASE_DIGEST_SCHEMA,
            "file_name": RELEASE_FILE,
            "bytes": len(encoded),
            "sha256": sha256(encoded),
        }
    )
    commit_artifact_directory(
        output,
        artifact_name=RELEASE_FILE,
        sidecar_name=RELEASE_SIDECAR,
        artifact=encoded,
        sidecar=sidecar,
        commit_schema=RELEASE_COMMIT_SCHEMA,
    )
    return verify_v1b_release_bundle(output)


def verify_v1b_release_bundle(directory: Path) -> dict[str, Any]:
    """Deep-verify the committed release pair and its canonical detached digest."""
    payload, sidecar_payload = verify_artifact_directory(
        directory,
        artifact_name=RELEASE_FILE,
        sidecar_name=RELEASE_SIDECAR,
        commit_schema=RELEASE_COMMIT_SCHEMA,
    )
    value = strict_json_object(payload)
    try:
        typed = ReleaseBundle.model_validate(value)
    except ValidationError as error:
        raise ValueError("release bundle failed its strict typed schema") from error
    canonical = canonical_json(typed.model_dump(mode="json", by_alias=True))
    if payload != canonical:
        raise ValueError("release bundle is not canonical supported JSON")
    sidecar = strict_json_object(sidecar_payload)
    expected = {
        "schema": RELEASE_DIGEST_SCHEMA,
        "file_name": RELEASE_FILE,
        "bytes": len(payload),
        "sha256": sha256(payload),
    }
    if sidecar != expected or sidecar_payload != canonical_json(expected):
        raise ValueError("release sidecar does not bind the canonical bundle")
    return typed.model_dump(mode="json", by_alias=True)


def _artifact_digest(manifest: dict[str, Any], role: str) -> str:
    matches = [
        artifact.get("sha256")
        for artifact in manifest.get("artifacts", [])
        if artifact.get("role") == role
    ]
    if len(matches) != 1 or not isinstance(matches[0], str):
        raise ValueError(f"benchmark manifest is missing artifact role: {role}")
    return matches[0]


def _reject_output_overlap(output: Path, inputs: tuple[Path, ...]) -> None:
    output_absolute = os.path.abspath(output)
    for value in inputs:
        candidate = os.path.abspath(value)
        common = os.path.commonpath((output_absolute, candidate))
        if common in {output_absolute, candidate}:
            raise ValueError("release output must not overlap an input artifact")
