"""Artifact-backed deterministic producer for P3-A dogfood evidence."""

from __future__ import annotations

from pathlib import Path

from pydantic import ValidationError

from plico_benchmarks.core.dogfood_artifacts import (
    ArtifactInputs,
    reject_output_overlap,
    verify_artifact_inputs,
)
from plico_benchmarks.core.dogfood_io import (
    EVIDENCE_FILE,
    canonical_json,
    commit_evidence_directory,
    read_regular,
    sha256,
    strict_json_object,
    verify_evidence_directory,
)
from plico_benchmarks.core.dogfood_schema import (
    DOGFOOD_DIGEST_SCHEMA,
    DogfoodCapture,
    DogfoodEvidence,
    assert_protocol_constant,
    validate_no_sensitive_values,
)


def generate_dogfood_evidence(
    *, capture_path: Path, inputs: ArtifactInputs, output_directory: Path
) -> DogfoodEvidence:
    """Verify concrete inputs and commit one owner-only evidence directory."""
    assert_protocol_constant()
    reject_output_overlap(output_directory, capture_path, inputs)
    capture_payload = read_regular(capture_path, private=True)
    try:
        capture = DogfoodCapture.model_validate(strict_json_object(capture_payload))
    except ValidationError as error:
        raise ValueError("dogfood capture failed its typed schema") from error
    capture_value = capture.model_dump(mode="json", by_alias=True)
    validate_no_sensitive_values(capture_value)
    canonical_capture = canonical_json(capture_value)
    if capture_payload != canonical_capture:
        raise ValueError("dogfood capture is not deterministic canonical JSON")

    verified = verify_artifact_inputs(capture, inputs)
    status = capture.request_ledger[8].typed_result_evidence
    rebuild = capture.request_ledger[9].typed_result_evidence
    if status is None or rebuild is None:
        raise ValueError("projection operations lack typed evidence")
    reader = capture.real_llm_reader.model_dump(mode="json")
    reader.update(
        {
            "trace_sha256": verified.reader_trace_sha256,
            "trace_bytes": verified.reader_trace_bytes,
            "trace_records": verified.reader_trace_records,
            "trace_request_count": verified.reader_trace_request_count,
            "privacy_canary_scan_passed": True,
        }
    )
    evidence = DogfoodEvidence.model_validate(
        {
            "schema": "plico.p3a.dogfood-evidence/v1",
            "protocol": capture.protocol,
            "bundle_run_id": capture.bundle_run_id,
            "captured_at_utc": capture.captured_at_utc,
            "scope": "single local personal vault; no tenant or organization semantics",
            "attestation_boundary": (
                "local_artifact_integrity_and_drift_detection_not_external_cryptographic_attestation"
            ),
            "capture_binding": {
                "schema": "plico.p3a.dogfood-capture-binding/v1",
                "canonical_capture_bytes": len(canonical_capture),
                "capture_sha256": sha256(canonical_capture),
            },
            "transport": capture.transport.model_dump(mode="json"),
            "embedding_provider": capture.embedding_provider.model_dump(mode="json"),
            "seed_evidence": capture.seed_evidence.model_dump(mode="json"),
            "build": {
                "plicod_binary": verified.plicod.model_dump(mode="json"),
                "plico_source_manifest": verified.plico_source.model_dump(
                    mode="json", by_alias=True
                ),
                "plico_agents_source_manifest": verified.agents_source.model_dump(
                    mode="json", by_alias=True
                ),
                "plico_agents_uv_lock": verified.agents_uv_lock.model_dump(mode="json"),
                "ollama_probe": verified.ollama_probe.model_dump(mode="json"),
            },
            "canonical_catalog": {
                "status": "passed",
                "catalog_exact_order_and_unique": True,
                "catalog_operation_count": 14,
                "observed_wire_operation_set_exact": True,
                "request_count": 14,
                "object_target_hit_verified": True,
                "memory_target_hit_verified": True,
                "delete_target_absence_verified": True,
                "session_watermark_monotonic": True,
                "projection_status_observed": True,
                "projection_rebuild_durable_receipt_verified": True,
                "request_ledger": [item.model_dump(mode="json") for item in capture.request_ledger],
            },
            "restart_replay": capture.restart_replay.model_dump(mode="json"),
            "write_disconnect_no_retry": {
                "status": "passed",
                "cases": [item.model_dump(mode="json") for item in capture.disconnect_cases],
            },
            "v1_reject": capture.v1_reject.model_dump(mode="json"),
            "v1_zero_state": verified.v1_zero_state,
            "canonical_fingerprint": verified.canonical_fingerprint.model_dump(
                mode="json", by_alias=True
            ),
            "daemon_trace": verified.daemon_trace.model_dump(mode="json", by_alias=True),
            "auxiliary_requests": {
                "request_count": len(verified.auxiliary_request_ids),
                "request_ids": list(verified.auxiliary_request_ids),
            },
            "post_restart_verification": verified.post_restart_verification.model_dump(mode="json"),
            "privacy": {
                "body_recorded": False,
                "query_recorded": False,
                "tag_recorded": False,
                "cid_recorded": False,
                "bearer_or_token_recorded": False,
                "provider_body_recorded": False,
                "provider_secret_recorded": False,
                "provider_endpoint_recorded": False,
                "full_vault_path_recorded": False,
                "canonical_root_or_content_hash_recorded": False,
            },
            "real_llm_reader": reader,
        }
    )
    evidence_value = evidence.model_dump(mode="json", by_alias=True)
    validate_no_sensitive_values(evidence_value)
    encoded = canonical_json(evidence_value)
    if any(canary in encoded for canary in verified.canaries):
        raise ValueError("generated evidence contains privacy canary material")
    sidecar = canonical_json(
        {
            "schema": DOGFOOD_DIGEST_SCHEMA,
            "file_name": EVIDENCE_FILE,
            "bytes": len(encoded),
            "sha256": sha256(encoded),
        }
    )
    commit_evidence_directory(output_directory, encoded, sidecar)
    return verify_dogfood_evidence(output_directory)


def verify_dogfood_evidence(directory: Path) -> DogfoodEvidence:
    """Deep-verify a committed evidence directory and its detached binding."""
    _, evidence = read_verified_dogfood_evidence(directory)
    return evidence


def read_verified_dogfood_evidence(directory: Path) -> tuple[bytes, DogfoodEvidence]:
    """Return exact artifact bytes only after directory, sidecar, and schema verification."""
    assert_protocol_constant()
    payload, sidecar_payload = verify_evidence_directory(directory)
    try:
        evidence = DogfoodEvidence.model_validate(strict_json_object(payload))
    except ValidationError as error:
        raise ValueError("dogfood evidence failed its typed schema") from error
    value = evidence.model_dump(mode="json", by_alias=True)
    validate_no_sensitive_values(value)
    if payload != canonical_json(value):
        raise ValueError("dogfood evidence is not deterministic canonical JSON")
    sidecar = strict_json_object(sidecar_payload)
    expected = {
        "schema": DOGFOOD_DIGEST_SCHEMA,
        "file_name": EVIDENCE_FILE,
        "bytes": len(payload),
        "sha256": sha256(payload),
    }
    if sidecar != expected or sidecar_payload != canonical_json(expected):
        raise ValueError("dogfood evidence sidecar does not bind the artifact")
    return payload, evidence
