"""Focused V1-B release-evidence contracts."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import threading

import pytest

from plico_benchmarks.core import dogfood_io
from plico_benchmarks.core.client import PUBLIC_OPERATION_CATALOG
from plico_benchmarks.core.dogfood_evidence import generate_dogfood_evidence
from plico_benchmarks.core.dogfood_io import COMMITTED_FILE, canonical_json, sha256
from plico_benchmarks.core.release_bundle import (
    RELEASE_COMMIT_SCHEMA,
    RELEASE_DIGEST_SCHEMA,
    RELEASE_FILE,
    RELEASE_SIDECAR,
    build_v1b_release_bundle,
    verify_v1b_release_bundle,
)
from plico_benchmarks.core.result_artifact import (
    RUN_MANIFEST_FILE,
    commit_result_directory,
)
from plico_benchmarks.suites.v1b_release import (
    V1BReleaseSuite,
    _mutating_disconnect_probe,
    _redacted_evidence,
)


def _rewrite_committed_release(directory, value: dict) -> None:
    payload = canonical_json(value)
    sidecar = canonical_json(
        {
            "schema": RELEASE_DIGEST_SCHEMA,
            "file_name": RELEASE_FILE,
            "bytes": len(payload),
            "sha256": sha256(payload),
        }
    )
    committed = canonical_json(
        {
            "schema": RELEASE_COMMIT_SCHEMA,
            "artifact_file": RELEASE_FILE,
            "artifact_sha256": sha256(payload),
            "sidecar_file": RELEASE_SIDECAR,
            "sidecar_sha256": sha256(sidecar),
        }
    )
    for name, content in (
        (RELEASE_FILE, payload),
        (RELEASE_SIDECAR, sidecar),
        (COMMITTED_FILE, committed),
    ):
        (directory / name).write_bytes(content)
        (directory / name).chmod(0o600)


def test_public_catalog_is_exact_ordered_and_unique():
    assert PUBLIC_OPERATION_CATALOG == (
        "capabilities.describe",
        "runtime.readiness",
        "object.put",
        "object.get",
        "object.search",
        "memory.create",
        "memory.get",
        "memory.recall",
        "projection.status",
        "projection.rebuild",
        "memory.update",
        "memory.delete",
        "session.start",
        "session.end",
    )
    assert len(PUBLIC_OPERATION_CATALOG) == len(set(PUBLIC_OPERATION_CATALOG)) == 14


def test_all_seven_mutations_send_one_frame_and_do_not_retry_after_disconnect():
    result = _mutating_disconnect_probe()

    assert result["status"] == "measured"
    assert result["count"] == 7
    assert result["failure_count"] == 0
    assert [case["operation"] for case in result["fault_cases"]] == [
        "object.put",
        "memory.create",
        "projection.rebuild",
        "memory.update",
        "memory.delete",
        "session.start",
        "session.end",
    ]
    assert all(
        case["attempt_count"] == 1
        and case["frame_sent"] is True
        and case["response_observed"] is False
        and case["outcome"] == "connection_closed_after_frame"
        for case in result["fault_cases"]
    )


def test_embedded_evidence_is_redacted_but_retains_flow_and_watermarks():
    evidence = _redacted_evidence(
        {
            "operation": "memory.update_ack",
            "phase": "canonical",
            "count": 1,
            "status": "measured",
            "request_id": "00000000-0000-4000-8000-000000000001",
            "attempt_count": 1,
            "frame_sent": True,
            "response_observed": True,
            "generation_before": 1,
            "generation_after": 2,
            "revision_watermark_before": 1,
            "revision_watermark_after": 2,
            "content": "must-not-survive",
            "query": "must-not-survive",
            "target_root_hash": "f" * 64,
            "failure_count": 0,
        }
    )

    assert evidence["typed_outcome"] == "measured"
    assert evidence["attempt_count"] == 1
    assert evidence["generation_after"] == 2
    assert "content" not in evidence
    assert "query" not in evidence
    assert "target_root_hash" not in evidence


def test_external_reader_trace_is_linked_but_not_scored(tmp_path, monkeypatch):
    run_id = "00000000-0000-4000-8000-000000000009"
    records = [
        {"event": "workflow.analyst", "phase": "completed", "run_id": run_id},
        {"event": "workflow.reporter", "phase": "completed", "run_id": run_id},
        *[
            {"event": "transport.domain_result", "operation": operation, "ok": True}
            for operation in (
                "object.put",
                "memory.create",
                "object.search",
                "memory.recall",
                "session.start",
                "session.end",
                "memory.delete",
            )
        ],
    ]
    trace = tmp_path / "reader.jsonl"
    trace.write_text("\n".join(json.dumps(record) for record in records), encoding="utf-8")
    trace.chmod(0o600)
    monkeypatch.setenv("PLICO_BENCH_EXTERNAL_READER_TRACE", str(trace))
    monkeypatch.setenv("PLICO_BENCH_EXTERNAL_READER_RUN_ID", run_id)
    monkeypatch.setenv("PLICO_BENCH_EXTERNAL_READER_BACKEND", "openai-compatible")
    monkeypatch.setenv("PLICO_BENCH_EXTERNAL_READER_MODEL", "test-model")
    suite = object.__new__(V1BReleaseSuite)
    suite._input_artifacts = []
    suite._external_evidence = []

    suite._load_external_reader_evidence()

    assert suite._input_artifacts[0]["role"] == "external_real_llm_trace"
    assert suite._external_evidence == [
        {
            "relationship": "linked_not_scored_in_v1b_release_run",
            "workflow_run_id": run_id,
            "outcome": "pytest_1_of_1_pass",
            "backend": "openai-compatible",
            "model": "test-model",
            "transport": "uds",
            "trace_sha256": suite._input_artifacts[0]["sha256"],
            "trace_bytes": suite._input_artifacts[0]["bytes"],
            "independent_runs_observed": 1,
            "comparative_inference": "not_available_single_run",
        }
    ]


def test_release_bundle_binds_result_manifest_dogfood_and_binary(tmp_path, dogfood_artifacts):
    inputs, capture_path = dogfood_artifacts
    dogfood_path = tmp_path / "dogfood"
    dogfood = generate_dogfood_evidence(
        capture_path=capture_path, inputs=inputs, output_directory=dogfood_path
    )
    result_path = tmp_path / "result"
    reader = dogfood.real_llm_reader
    binary_sha256 = dogfood.build.plicod_binary.sha256
    result = {
        "metadata": {
            "suite": "v1b-release",
            "run_id": "00000000-0000-4000-8000-000000000b01",
        },
        "evidence_ledger": [{"operation": "memory.create_ack"}],
        "run_manifest": {
            "schema_version": "plico.memory-eval-run/v1",
            "run_id": "00000000-0000-4000-8000-000000000b01",
            "protocol": "plico.personal.v2",
            "suite": "v1b-release",
            "run_class": "release_evidence",
            "sampling": {"actual": 1, "scored": 1, "failed": 0, "excluded": 0},
            "independent_runs_observed": 1,
            "comparative_inference": "not_available_single_run",
            "failure_ledger": [],
            "fault_ledger": [{"count": 7}],
            "pipeline": {"source_watermark": {"generation": 1}},
            "artifacts": [{"role": "plicod_binary", "sha256": binary_sha256}],
            "external_evidence": [
                {
                    "workflow_run_id": reader.workflow_run_id,
                    "trace_sha256": reader.trace_sha256,
                }
            ],
        },
    }
    commit_result_directory(result_path, result)
    manifest_path = result_path / RUN_MANIFEST_FILE
    output = tmp_path / "bundle"

    bundle = build_v1b_release_bundle(
        benchmark_result=result_path,
        dogfood_bundle=dogfood_path,
        output=output,
    )

    assert bundle["claims"]["independent_runs_are_linked_not_merged"] is True
    assert bundle["schema"] == "plico.v1b.release-evidence-bundle/v3"
    assert bundle["claims"]["public_protocol_exact_14"] is True
    assert bundle["claims"]["memory_embedding_control_plane_supported"] is True
    assert bundle["claims"]["memory_embedding_retrieval_supported"] is False
    assert bundle["benchmark"]["run_id"] == "00000000-0000-4000-8000-000000000b01"
    assert bundle["dogfood"]["bundle_run_id"] == dogfood.bundle_run_id
    assert output.stat().st_mode & 0o777 == 0o700
    digest = json.loads((output / RELEASE_SIDECAR).read_text(encoding="utf-8"))
    assert digest["sha256"] == hashlib.sha256((output / RELEASE_FILE).read_bytes()).hexdigest()

    rebound = tmp_path / "rebound-bundle"
    shutil.copytree(output, rebound)
    rebound_value = json.loads((rebound / RELEASE_FILE).read_text())
    rebound_value["binary_binding"]["plicod_sha256"] = "0" * 64
    _rewrite_committed_release(rebound, rebound_value)
    with pytest.raises(ValueError, match="strict typed schema"):
        verify_v1b_release_bundle(rebound)

    for name, mutate in (
        ("missing-benchmark", lambda value: value.pop("benchmark")),
        (
            "false-claim",
            lambda value: value["claims"].update({"public_protocol_exact_14": False}),
        ),
        (
            "failed-sample",
            lambda value: value["benchmark"]["sampling"].update(
                {"actual": 1, "scored": 0, "failed": 1, "excluded": 0}
            ),
        ),
    ):
        tampered = tmp_path / name
        shutil.copytree(output, tampered)
        tampered_value = json.loads((tampered / RELEASE_FILE).read_text())
        mutate(tampered_value)
        _rewrite_committed_release(tampered, tampered_value)
        with pytest.raises(ValueError, match="strict typed schema"):
            verify_v1b_release_bundle(tampered)

    extra = output / "extra"
    extra.write_text("x")
    extra.chmod(0o600)
    with pytest.raises(ValueError, match="mixed-run"):
        verify_v1b_release_bundle(output)
    extra.unlink()

    fifo = tmp_path / "fifo"
    os.mkfifo(fifo, 0o600)
    with pytest.raises((OSError, ValueError)):
        build_v1b_release_bundle(
            benchmark_result=fifo,
            dogfood_bundle=dogfood_path,
            output=tmp_path / "fifo-bundle",
        )
    linked = tmp_path / "linked"
    linked.symlink_to(result_path, target_is_directory=True)
    with pytest.raises((OSError, ValueError)):
        build_v1b_release_bundle(
            benchmark_result=linked,
            dogfood_bundle=dogfood_path,
            output=tmp_path / "linked-bundle",
        )

    concurrent = tmp_path / "concurrent-bundle"
    outcomes = []

    def build() -> None:
        try:
            build_v1b_release_bundle(
                benchmark_result=result_path,
                dogfood_bundle=dogfood_path,
                output=concurrent,
            )
            outcomes.append("ok")
        except ValueError:
            outcomes.append("rejected")

    threads = [threading.Thread(target=build) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=5)
    assert sorted(outcomes) == ["ok", "rejected"]
    verify_v1b_release_bundle(concurrent)

    original = dogfood_io._atomic_write_at
    calls = 0

    def fail_second(directory_fd: int, name: str, payload: bytes) -> None:
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("release crash")
        original(directory_fd, name, payload)

    monkeypatch = pytest.MonkeyPatch()
    monkeypatch.setattr(dogfood_io, "_atomic_write_at", fail_second)
    crashed = tmp_path / "crashed-bundle"
    with pytest.raises(OSError, match="release crash"):
        build_v1b_release_bundle(
            benchmark_result=result_path,
            dogfood_bundle=dogfood_path,
            output=crashed,
        )
    monkeypatch.undo()
    with pytest.raises(ValueError, match="incomplete"):
        verify_v1b_release_bundle(crashed)

    detached = json.loads(manifest_path.read_text(encoding="utf-8"))
    detached["protocol"] = "tampered"
    manifest_path.write_text(json.dumps(detached), encoding="utf-8")
    manifest_path.chmod(0o600)
    with pytest.raises(ValueError, match="COMMITTED marker"):
        build_v1b_release_bundle(
            benchmark_result=result_path,
            dogfood_bundle=dogfood_path,
            output=output,
        )
