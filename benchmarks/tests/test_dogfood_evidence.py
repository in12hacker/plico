"""Artifact-backed P3-A dogfood evidence producer contracts."""

from __future__ import annotations

import json
import os
import threading
from dataclasses import replace
from pathlib import Path
from uuid import UUID

import pytest
from typer.testing import CliRunner

from plico_benchmarks.cli import app
from plico_benchmarks.core import dogfood_io
from plico_benchmarks.core.dogfood_collectors import collect_ollama_probe
from plico_benchmarks.core.dogfood_evidence import (
    generate_dogfood_evidence,
    verify_dogfood_evidence,
)
from plico_benchmarks.core.dogfood_io import (
    COMMITTED_FILE,
    EVIDENCE_FILE,
    SIDECAR_FILE,
    canonical_json,
    sha256,
    strict_json_object,
)
from plico_benchmarks.core.dogfood_schema import DOGFOOD_DIGEST_SCHEMA


@pytest.mark.parametrize("constant", ["NaN", "Infinity", "-Infinity"])
def test_evidence_json_rejects_nonstandard_nonfinite_numbers(constant: str) -> None:
    with pytest.raises(ValueError, match="non-finite JSON number"):
        strict_json_object(f'{{"value":{constant}}}'.encode())
    with pytest.raises(ValueError):
        canonical_json({"value": float(constant)})


def _rewrite_committed_evidence(directory: Path, value: dict) -> None:
    payload = canonical_json(value)
    sidecar = canonical_json(
        {
            "schema": DOGFOOD_DIGEST_SCHEMA,
            "file_name": EVIDENCE_FILE,
            "bytes": len(payload),
            "sha256": sha256(payload),
        }
    )
    committed = canonical_json(
        {
            "schema": "plico.p3a.dogfood-evidence-commit/v1",
            "artifact_file": EVIDENCE_FILE,
            "artifact_sha256": sha256(payload),
            "sidecar_file": SIDECAR_FILE,
            "sidecar_sha256": sha256(sidecar),
        }
    )
    for name, content in (
        (EVIDENCE_FILE, payload),
        (SIDECAR_FILE, sidecar),
        (COMMITTED_FILE, committed),
    ):
        (directory / name).write_bytes(content)
        (directory / name).chmod(0o600)


def test_artifact_backed_generator_is_private_deterministic_and_deep_verifiable(
    tmp_path: Path, dogfood_artifacts
) -> None:
    inputs, capture = dogfood_artifacts
    first = tmp_path / "evidence-first"
    second = tmp_path / "evidence-second"

    evidence = generate_dogfood_evidence(
        capture_path=capture, inputs=inputs, output_directory=first
    )
    generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=second)

    assert evidence.schema_id == "plico.p3a.dogfood-evidence/v1"
    assert (first / EVIDENCE_FILE).read_bytes() == (second / EVIDENCE_FILE).read_bytes()
    assert first.stat().st_mode & 0o777 == 0o700
    assert all(item.stat().st_mode & 0o777 == 0o600 for item in first.iterdir())
    assert verify_dogfood_evidence(first).bundle_run_id == evidence.bundle_run_id
    assert evidence.daemon_trace.public_request_count == 14
    assert evidence.daemon_trace.disconnect_request_count == 7
    assert evidence.real_llm_reader.trace_request_count == 4
    assert evidence.build.plicod_binary.trust == "sealed_owner_only_executable_0700"
    assert evidence.build.plico_source_manifest.trust == "live_same_euid_non_world_writable"
    assert any(item.bytes == 0 for item in evidence.build.plico_source_manifest.files)


def test_dogfood_generate_and_verify_cli_use_the_exact_artifact_contract(
    tmp_path: Path, dogfood_artifacts
) -> None:
    inputs, capture = dogfood_artifacts
    output = tmp_path / "cli-evidence"
    options = {
        "capture": capture,
        "plicod-binary": inputs.plicod_binary,
        "uds-socket": inputs.uds_socket,
        "plico-root": inputs.plico_root,
        "plico-agents-root": inputs.plico_agents_root,
        "uv-lock": inputs.uv_lock,
        "daemon-trace": inputs.daemon_trace,
        "reader-trace": inputs.reader_trace,
        "canonical-before-rebuild": inputs.canonical_before_rebuild,
        "canonical-after-rebuild": inputs.canonical_after_rebuild,
        "canonical-before-restart": inputs.canonical_before_restart,
        "canonical-after-restart": inputs.canonical_after_restart,
        "canary": inputs.canary,
        "ollama-probe": inputs.ollama_probe,
        "canonical-vault": inputs.canonical_vault,
        "v1-zero-before": inputs.v1_zero_before,
        "v1-zero-after": inputs.v1_zero_after,
        "output-dir": output,
    }
    arguments = ["dogfood-evidence"]
    for name, value in options.items():
        arguments.extend((f"--{name}", str(value)))
    generated = CliRunner().invoke(app, arguments)
    assert generated.exit_code == 0, generated.output
    verified = CliRunner().invoke(app, ["verify-dogfood-evidence", "--artifact-dir", str(output)])
    assert verified.exit_code == 0, verified.output


def test_rejects_old_protocol_trace_secret_and_canonical_drift(
    tmp_path: Path, dogfood_artifacts
) -> None:
    inputs, capture = dogfood_artifacts
    value = json.loads(capture.read_text())
    value["protocol"] = "plico.personal.v1"
    capture.write_bytes(canonical_json(value))
    with pytest.raises(ValueError, match="typed schema"):
        generate_dogfood_evidence(
            capture_path=capture, inputs=inputs, output_directory=tmp_path / "old"
        )

    value["protocol"] = "plico.personal.v2"
    capture.write_bytes(canonical_json(value))
    inputs.daemon_trace.write_bytes(inputs.daemon_trace.read_bytes() + b"PRIVATE-CANARY-1234\n")
    with pytest.raises(ValueError, match="secret or privacy"):
        generate_dogfood_evidence(
            capture_path=capture, inputs=inputs, output_directory=tmp_path / "secret"
        )

    inputs.daemon_trace.write_text("{}\n", encoding="utf-8")
    checkpoint = json.loads(inputs.canonical_before_restart.read_text())
    checkpoint["entries"][0]["path"] = "tampered"
    inputs.canonical_before_restart.write_bytes(canonical_json(checkpoint))
    with pytest.raises(ValueError, match="canonical inventor"):
        generate_dogfood_evidence(
            capture_path=capture, inputs=inputs, output_directory=tmp_path / "drift"
        )


def test_fifo_symlink_and_output_overlap_fail_closed(tmp_path: Path, dogfood_artifacts) -> None:
    inputs, capture = dogfood_artifacts
    fifo = tmp_path / "trace-fifo"
    os.mkfifo(fifo, 0o600)
    with pytest.raises(ValueError, match="invalid type"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=replace(inputs, daemon_trace=fifo),
            output_directory=tmp_path / "fifo-output",
        )
    link = tmp_path / "trace-link"
    link.symlink_to(inputs.daemon_trace)
    with pytest.raises(ValueError, match="safely opened"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=replace(inputs, daemon_trace=link),
            output_directory=tmp_path / "link-output",
        )
    with pytest.raises(ValueError, match="overlap"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=inputs,
            output_directory=inputs.canonical_vault / "evidence",
        )
    inputs.uds_socket.chmod(0o666)
    with pytest.raises(ValueError, match="UDS artifact"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=inputs,
            output_directory=tmp_path / "uds-mode-output",
        )


def test_v1_zero_state_and_trace_order_are_artifact_bound(
    tmp_path: Path, dogfood_artifacts
) -> None:
    inputs, capture = dogfood_artifacts
    lines = inputs.daemon_trace.read_text().splitlines()
    lines[0], lines[1] = lines[1], lines[0]
    reordered = tmp_path / "reordered.jsonl"
    reordered.write_text("\n".join(lines) + "\n")
    reordered.chmod(0o600)
    with pytest.raises(ValueError, match="sequence|order"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=replace(inputs, daemon_trace=reordered),
            output_directory=tmp_path / "trace-reordered",
        )
    value = json.loads(inputs.v1_zero_after.read_text())
    value["projection_entries"].append(
        {
            "path": "projection-store/tamper",
            "kind": "file",
            "mode": "0600",
            "bytes": 1,
            "sha256": "a" * 64,
        }
    )
    inputs.v1_zero_after.write_bytes(canonical_json(value))
    with pytest.raises(ValueError, match="zero-state"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=inputs,
            output_directory=tmp_path / "v1-state-tamper",
        )

    inputs = replace(inputs, v1_zero_after=inputs.v1_zero_before)
    with pytest.raises(ValueError, match="binding|distinct"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=inputs,
            output_directory=tmp_path / "v1-rebound",
        )


@pytest.mark.parametrize("tamper", ["empty", "rebound_revision"])
def test_post_restart_responses_bind_the_seeded_revision(
    tmp_path: Path, dogfood_artifacts, tamper: str
) -> None:
    inputs, capture = dogfood_artifacts
    records = [json.loads(line) for line in inputs.daemon_trace.read_text().splitlines()]
    completed = next(
        record
        for record in records
        if record.get("event") == "auxiliary_request"
        and record.get("category") == "post_restart_verification"
        and record.get("wire_operation") == "memory.get"
        and record.get("phase") == "completed"
    )
    if tamper == "empty":
        completed.pop("typed_result_evidence")
    else:
        completed["typed_result_evidence"]["revision_id"] = str(UUID(int=8888))
    inputs.daemon_trace.write_bytes(b"".join(canonical_json(record) for record in records))
    inputs.daemon_trace.chmod(0o600)

    with pytest.raises(ValueError, match="post-restart"):
        generate_dogfood_evidence(
            capture_path=capture,
            inputs=inputs,
            output_directory=tmp_path / tamper,
        )


def test_concurrent_commit_has_one_winner(tmp_path: Path, dogfood_artifacts) -> None:
    inputs, capture = dogfood_artifacts
    output = tmp_path / "one-output"
    outcomes: list[str] = []

    def run() -> None:
        try:
            generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=output)
            outcomes.append("ok")
        except ValueError:
            outcomes.append("rejected")

    threads = [threading.Thread(target=run) for _ in range(2)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join(timeout=5)
    assert outcomes.count("ok") == 1
    assert outcomes.count("rejected") == 1
    verify_dogfood_evidence(output)


def test_incomplete_or_rebound_directory_is_rejected(
    tmp_path: Path, dogfood_artifacts, monkeypatch
) -> None:
    inputs, capture = dogfood_artifacts
    output = tmp_path / "crashed"
    original = dogfood_io._atomic_write_at
    calls = 0

    def fail_second(directory_fd: int, name: str, payload: bytes) -> None:
        nonlocal calls
        calls += 1
        if calls == 2:
            raise OSError("injected crash")
        original(directory_fd, name, payload)

    monkeypatch.setattr(dogfood_io, "_atomic_write_at", fail_second)
    with pytest.raises(OSError, match="injected crash"):
        generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=output)
    with pytest.raises(ValueError, match="incomplete"):
        verify_dogfood_evidence(output)

    complete = tmp_path / "complete"
    monkeypatch.setattr(dogfood_io, "_atomic_write_at", original)
    generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=complete)
    evidence = json.loads((complete / EVIDENCE_FILE).read_text())
    evidence["embedding_provider"]["provider_compatibility_id"] = "0" * 64
    payload = canonical_json(evidence)
    sidecar = canonical_json(
        {
            "schema": DOGFOOD_DIGEST_SCHEMA,
            "file_name": EVIDENCE_FILE,
            "bytes": len(payload),
            "sha256": sha256(payload),
        }
    )
    committed = canonical_json(
        {
            "schema": "plico.p3a.dogfood-evidence-commit/v1",
            "artifact_file": EVIDENCE_FILE,
            "artifact_sha256": sha256(payload),
            "sidecar_file": SIDECAR_FILE,
            "sidecar_sha256": sha256(sidecar),
        }
    )
    for name, value in (
        (EVIDENCE_FILE, payload),
        (SIDECAR_FILE, sidecar),
        (COMMITTED_FILE, committed),
    ):
        (complete / name).write_bytes(value)
        (complete / name).chmod(0o600)
    with pytest.raises(ValueError, match="typed schema"):
        verify_dogfood_evidence(complete)


@pytest.mark.parametrize(
    ("section", "field"),
    (
        ("daemon_trace", "auxiliary_request_count"),
        ("daemon_trace", "records"),
        ("real_llm_reader", "trace_records"),
    ),
)
def test_deep_verifier_rejects_rebound_trace_metadata(
    tmp_path: Path, dogfood_artifacts, section: str, field: str
) -> None:
    inputs, capture = dogfood_artifacts
    output = tmp_path / f"rebound-{section}-{field}"
    generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=output)
    value = json.loads((output / EVIDENCE_FILE).read_text())
    value[section][field] += 1
    _rewrite_committed_evidence(output, value)

    with pytest.raises(ValueError, match="typed schema"):
        verify_dogfood_evidence(output)


@pytest.mark.parametrize("drift", ["version", "model", "digest"])
def test_ollama_collector_matches_rust_probe_and_rejects_drift(
    tmp_path: Path, monkeypatch, drift: str
) -> None:
    calls = 0

    def get_json(url: str) -> dict:
        nonlocal calls
        calls += 1
        if url.endswith("/api/version"):
            version = "0.11.5" if not (drift == "version" and calls > 2) else "0.11.6"
            return {"version": version}
        digest = "a" * 64 if not (drift == "digest" and calls > 2) else "b" * 64
        return {"models": [{"name": "model:latest", "digest": digest}]}

    def post_json(_url: str, request: dict) -> dict:
        assert request["input"] == "plico document identity probe v1"
        model = "other:latest" if drift == "model" else "model:latest"
        return {"model": model, "embeddings": [[1.0, 2.0]]}

    monkeypatch.setattr("plico_benchmarks.core.dogfood_collectors._get_json", get_json)
    monkeypatch.setattr("plico_benchmarks.core.dogfood_collectors._post_json", post_json)
    with pytest.raises(ValueError):
        collect_ollama_probe(
            base_url="http://127.0.0.1:11434",
            configured_tag="model:latest",
            output=tmp_path / "probe.json",
            requested_target_dimension=None,
            adaptive_prefix_contract_id="provider-native-input-v1",
        )


def test_ollama_collector_accepts_explicit_latest_and_writes_redacted_typed_evidence(
    tmp_path: Path, monkeypatch
) -> None:
    def get_json(url: str) -> dict:
        if url.endswith("/api/version"):
            return {"version": "0.11.5"}
        return {"models": [{"name": "model:latest", "digest": "a" * 64}]}

    def post_json(_url: str, request: dict) -> dict:
        assert request == {
            "model": "model:latest",
            "input": "plico document identity probe v1",
            "truncate": False,
        }
        return {
            "model": "model:latest",
            "embeddings": [[3.0, 4.0]],
            "prompt_eval_count": 5,
            "total_duration": 10,
            "load_duration": 2,
        }

    monkeypatch.setattr("plico_benchmarks.core.dogfood_collectors._get_json", get_json)
    monkeypatch.setattr("plico_benchmarks.core.dogfood_collectors._post_json", post_json)
    output = tmp_path / "probe.json"
    collect_ollama_probe(
        base_url="http://127.0.0.1:11434",
        configured_tag="model:latest",
        output=output,
        requested_target_dimension=1,
        adaptive_prefix_contract_id="provider-native-input-v1",
    )
    value = json.loads(output.read_text())
    assert value["configured_exact_tag"] == "model:latest"
    assert value["normalization"] == "l2_after_matryoshka_truncation_v1"
    assert value["effective_dimension"] == 1
    assert output.stat().st_mode & 0o777 == 0o600
    assert b"127.0.0.1" not in output.read_bytes()


@pytest.mark.parametrize(
    "response",
    [
        {"model": "model:latest", "embeddings": [[1.0]], "prompt_eval_count": -1},
        {"model": "model:latest", "embeddings": [[1.0]], "total_duration": 2**64},
        {"model": "model:latest", "embeddings": [[1e40]]},
        {"model": "model:latest", "embeddings": [[1.0]], "load_duration": True},
    ],
)
def test_ollama_collector_enforces_rust_numeric_types(
    tmp_path: Path, monkeypatch, response: dict
) -> None:
    def get_json(url: str) -> dict:
        if url.endswith("/api/version"):
            return {"version": "0.11.5"}
        return {"models": [{"name": "model:latest", "digest": "a" * 64}]}

    monkeypatch.setattr("plico_benchmarks.core.dogfood_collectors._get_json", get_json)
    monkeypatch.setattr(
        "plico_benchmarks.core.dogfood_collectors._post_json", lambda _url, _value: response
    )
    with pytest.raises(ValueError):
        collect_ollama_probe(
            base_url="http://127.0.0.1:11434",
            configured_tag="model:latest",
            output=tmp_path / "probe.json",
            requested_target_dimension=None,
            adaptive_prefix_contract_id="provider-native-input-v1",
        )


def test_evidence_extra_entry_and_sidecar_tamper_are_rejected(
    tmp_path: Path, dogfood_artifacts
) -> None:
    inputs, capture = dogfood_artifacts
    output = tmp_path / "evidence"
    generate_dogfood_evidence(capture_path=capture, inputs=inputs, output_directory=output)
    extra = output / "extra"
    extra.write_text("x")
    extra.chmod(0o600)
    with pytest.raises(ValueError, match="mixed-run"):
        verify_dogfood_evidence(output)
    extra.unlink()
    (output / SIDECAR_FILE).write_text("{}\n")
    (output / SIDECAR_FILE).chmod(0o600)
    with pytest.raises(ValueError):
        verify_dogfood_evidence(output)
