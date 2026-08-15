"""Shared deterministic benchmark evidence fixtures."""

from __future__ import annotations

import json
import os
import stat
from pathlib import Path
from uuid import UUID

import pytest

from plico_benchmarks.core.client import PUBLIC_OPERATION_CATALOG
from plico_benchmarks.core.dogfood_artifacts import ArtifactInputs
from plico_benchmarks.core.dogfood_collectors import (
    collect_canonical_checkpoint,
    collect_v1_zero_state_checkpoint,
)
from plico_benchmarks.core.dogfood_io import canonical_json
from plico_benchmarks.core.dogfood_schema import (
    MUTATING_OPERATIONS,
    builder_spec_hash,
    provider_compatibility_id,
)

_ASSERTIONS = {
    "capabilities.describe": "catalog_exact_14",
    "runtime.readiness": "canonical_and_projection_ready",
    "object.put": "canonical_commit",
    "object.get": "target_content_verified",
    "object.search": "target_object_hit",
    "memory.create": "canonical_commit",
    "memory.get": "target_revision_verified",
    "memory.recall": "target_revision_hit_lexical",
    "projection.status": "ready_observed",
    "projection.rebuild": "durable_receipt_canonical_unchanged",
    "memory.update": "replacement_head_verified",
    "memory.delete": "tombstone_and_recall_absence_verified",
    "session.start": "session_started",
    "session.end": "watermark_monotonic",
}


def _uuid(index: int) -> str:
    return str(UUID(int=index))


def _watermark() -> dict:
    return {
        "generation": 3,
        "revision_watermark": 4,
        "policy_watermark": 2,
        "relation_watermark": 0,
    }


@pytest.fixture
def valid_dogfood_capture() -> dict:
    probe_identity = {
        "configured_exact_tag": "nomic-embed-text:latest",
        "model_digest_before": "a" * 64,
        "server_version": "0.11.5",
        "api_contract": "ollama-api-embed-truncate-false/v1",
        "raw_dimension": 768,
        "effective_dimension": 768,
        "requested_target_dimension": None,
        "adaptive_prefix_contract_id": "provider-native-input-v1",
        "normalization": "provider_native",
    }
    compatibility_id = provider_compatibility_id(
        {"exact_model_tag": "nomic-embed-text:latest", **probe_identity}
    )
    memory_id = _uuid(3999)
    revision_id = _uuid(4000)
    content_evidence_ref = "seed-memory-v1"
    builder_id = builder_spec_hash(
        {
            "provider_compatibility_id": compatibility_id,
            "exact_model_tag": "nomic-embed-text:latest",
            "raw_dimension": 768,
            "effective_dimension": 768,
            "normalization": "provider_native",
            "transform_contract_id": "provider-native-document-v1",
        }
    )
    receipt_id = _uuid(4001)
    request_ledger = []
    for index, operation in enumerate(PUBLIC_OPERATION_CATALOG, start=1):
        item = {
            "wire_operation": operation,
            "request_id": _uuid(index),
            "attempt_count": 1,
            "frame_count": 1,
            "typed_response_ok": True,
            "result_assertion": _ASSERTIONS[operation],
            "typed_result_evidence": None,
        }
        if operation == "memory.get":
            item["typed_result_evidence"] = {
                "result": "found",
                "memory_id": memory_id,
                "revision_id": revision_id,
                "content_evidence_ref": content_evidence_ref,
            }
        if operation == "memory.recall":
            item["typed_result_evidence"] = {
                "strategy": "lexical_overlap",
                "target_memory_id": memory_id,
                "target_revision_id": revision_id,
                "content_evidence_ref": content_evidence_ref,
                "match_count": 1,
                "target_found": True,
            }
        if operation == "projection.status":
            item["typed_result_evidence"] = {
                "kind": "memory_embedding",
                "observation": "observed",
                "state": "ready",
                "event_watermark": 7,
                "reconciled_source": _watermark(),
                "memory_id": memory_id,
                "revision_id": revision_id,
                "content_evidence_ref": content_evidence_ref,
                "builder_compatibility_id": builder_id,
            }
        if operation == "projection.rebuild":
            item["typed_result_evidence"] = {
                "kind": "memory_embedding",
                "selected_count": 1,
                "manifest_generation": 4,
                "event_watermark": 8,
                "reconciled_source": _watermark(),
                "canonical_ledger_unchanged": True,
                "memory_id": memory_id,
                "revision_id": revision_id,
                "builder_compatibility_id": builder_id,
                "receipt_id": receipt_id,
            }
        request_ledger.append(item)

    restart_state = {
        "projection_observation": "observed",
        "projection_state": "ready",
        "canonical_store_ready": True,
        "canonical_memory_persistence_ready": True,
        "projection_control_plane_ready": True,
        "projection_worker_ready": True,
        "revision_id": revision_id,
        "builder_compatibility_id": builder_id,
        "event_watermark": 8,
        "reconciled_source": _watermark(),
        "rebuild_receipt_id": receipt_id,
    }
    return {
        "schema": "plico.p3a.dogfood-capture/v1",
        "protocol": "plico.personal.v2",
        "bundle_run_id": _uuid(1000),
        "captured_at_utc": "2026-08-14T12:00:00Z",
        "transport": {
            "kind": "uds",
            "file_type": "socket",
            "mode": "0600",
            "owner_matches_effective_user": True,
        },
        "embedding_provider": {
            "family": "ollama",
            "evidence_schema": "plico.embedding.ollama-evidence/v1",
            "exact_model_tag": "nomic-embed-text:latest",
            "exact_tag_match_count": 1,
            "model_digest_before": "a" * 64,
            "model_digest_after": "a" * 64,
            "server_version": "0.11.5",
            "api_contract": "ollama-api-embed-truncate-false/v1",
            "provider_compatibility_id": compatibility_id,
            "raw_dimension": 768,
            "effective_dimension": 768,
            "requested_target_dimension": None,
            "adaptive_prefix_contract_id": "provider-native-input-v1",
            "input_contract": "memory_text_utf8_v1",
            "operation_contract": "document_v1",
            "normalization": "provider_native",
            "transform_contract_id": "provider-native-document-v1",
            "identity_verified_before_and_after": True,
            "fallback_used": False,
        },
        "seed_evidence": {
            "memory_id": memory_id,
            "revision_id": revision_id,
            "content_evidence_ref": content_evidence_ref,
        },
        "request_ledger": request_ledger,
        "disconnect_cases": [
            {
                "wire_operation": operation,
                "request_id": _uuid(100 + index),
                "attempt_count": 1,
                "frame_count": 1,
                "response_observed": False,
                "outcome": "ambiguous_commit_no_retry",
            }
            for index, operation in enumerate(MUTATING_OPERATIONS)
        ],
        "v1_reject": {
            "protocol": "plico.personal.v1",
            "wire_operation": "memory.get",
            "request_id": _uuid(7000),
            "attempt_count": 1,
            "frame_count": 1,
            "response_category": "unsupported_protocol",
            "predispatch_rejected": True,
            "authentication_invoked": False,
            "daemon_instance_id": _uuid(3000),
        },
        "restart_replay": {
            "status": "passed",
            "shutdown_flush_observed": True,
            "new_daemon_process_started": True,
            "pre_restart": {**restart_state, "daemon_instance_id": _uuid(3000)},
            "post_restart": {**restart_state, "daemon_instance_id": _uuid(3001)},
        },
        "real_llm_reader": {
            "status": "passed",
            "workflow_run_id": _uuid(2000),
            "backend": "openai-compatible",
            "model": "deepseek-v4-flash",
            "fallback_used": False,
            "seeded_object_present_in_evidence_ids": True,
            "seeded_memory_present_in_evidence_ids": True,
            "reported_citations_subset_of_evidence_ids": True,
            "workflow_performed_no_object_or_memory_writeback": True,
            "model_answer_body_recorded": False,
        },
    }


def _write_private(path: Path, value: object) -> None:
    path.write_bytes(canonical_json(value))
    path.chmod(0o600)


def _daemon_records(capture: dict) -> list[dict]:
    schema = "plico.p3a.dogfood-trace-record/v1"
    run_id = capture["bundle_run_id"]
    v1 = capture["v1_reject"]
    pre_daemon = capture["restart_replay"]["pre_restart"]["daemon_instance_id"]
    post_daemon = capture["restart_replay"]["post_restart"]["daemon_instance_id"]
    records = [
        {
            "schema": schema,
            "run_id": run_id,
            "event": "provider_identity",
            "phase": "verified",
            "identity": capture["embedding_provider"],
        },
        {
            "schema": schema,
            "run_id": run_id,
            "event": "v1_protocol_reject",
            "phase": "rejected",
            **v1,
        },
    ]
    for item in capture["request_ledger"]:
        started = {
            "schema": schema,
            "run_id": run_id,
            "event": "request",
            "phase": "started",
            "request_id": item["request_id"],
            "wire_operation": item["wire_operation"],
            "transport": "uds",
        }
        completed = {
            **started,
            "phase": "completed",
            "attempt_count": 1,
            "frame_count": 1,
            "typed_response_ok": True,
            "result_assertion": item["result_assertion"],
        }
        if item.get("typed_result_evidence") is not None:
            completed["typed_result_evidence"] = item["typed_result_evidence"]
        records.extend((started, completed))
    for item in capture["disconnect_cases"]:
        started = {
            "schema": schema,
            "run_id": run_id,
            "event": "disconnect",
            "phase": "started",
            "request_id": item["request_id"],
            "wire_operation": item["wire_operation"],
            "transport": "uds",
        }
        records.extend(
            (
                started,
                {
                    **started,
                    "phase": "ambiguous",
                    "attempt_count": 1,
                    "frame_count": 1,
                    "response_observed": False,
                    "outcome": "ambiguous_commit_no_retry",
                },
            )
        )
    auxiliary_id = _uuid(5000)
    auxiliary = {
        "schema": schema,
        "run_id": run_id,
        "event": "auxiliary_request",
        "phase": "started",
        "request_id": auxiliary_id,
        "category": "projection_poll",
        "transport": "uds",
    }
    records.extend((auxiliary, {**auxiliary, "phase": "completed", "typed_response_ok": True}))
    for phase, key in (("before", "pre_restart"), ("after", "post_restart")):
        records.append(
            {
                "schema": schema,
                "run_id": run_id,
                "event": "restart_checkpoint",
                "phase": phase,
                "state": capture["restart_replay"][key],
            }
        )
    for offset, operation in enumerate(("memory.get", "memory.recall", "projection.status")):
        auxiliary = {
            "schema": schema,
            "run_id": run_id,
            "event": "auxiliary_request",
            "phase": "started",
            "request_id": _uuid(5100 + offset),
            "category": "post_restart_verification",
            "wire_operation": operation,
            "transport": "uds",
        }
        if operation == "memory.get":
            typed_result = {
                "result": "found",
                **capture["seed_evidence"],
            }
        elif operation == "memory.recall":
            typed_result = {
                "strategy": "lexical_overlap",
                "target_memory_id": capture["seed_evidence"]["memory_id"],
                "target_revision_id": capture["seed_evidence"]["revision_id"],
                "content_evidence_ref": capture["seed_evidence"]["content_evidence_ref"],
                "match_count": 1,
                "target_found": True,
            }
        else:
            typed_result = {
                "kind": "memory_embedding",
                "observation": "observed",
                "state": "ready",
                "event_watermark": capture["restart_replay"]["post_restart"]["event_watermark"],
                "reconciled_source": capture["restart_replay"]["post_restart"]["reconciled_source"],
                "memory_id": capture["seed_evidence"]["memory_id"],
                "revision_id": capture["seed_evidence"]["revision_id"],
                "content_evidence_ref": capture["seed_evidence"]["content_evidence_ref"],
                "builder_compatibility_id": capture["restart_replay"]["post_restart"][
                    "builder_compatibility_id"
                ],
            }
        records.extend(
            (
                auxiliary,
                {
                    **auxiliary,
                    "phase": "completed",
                    "typed_response_ok": True,
                    "typed_result_evidence": typed_result,
                },
            )
        )
    after_restart = False
    for sequence, record in enumerate(records, start=1):
        if record["event"] == "restart_checkpoint" and record["phase"] == "after":
            after_restart = True
        record["sequence"] = sequence
        record["daemon_instance_id"] = post_daemon if after_restart else pre_daemon
    return records


def _reader_records(capture: dict) -> list[dict]:
    schema = "plico.p3a.reader-trace-record/v1"
    reader = capture["real_llm_reader"]
    run_id = reader["workflow_run_id"]
    records = [
        {
            "schema": schema,
            "run_id": run_id,
            "event": "workflow",
            "phase": "completed",
            "role": role,
        }
        for role in ("analyst", "reporter")
    ]
    for index, (operation, evidence_ref) in enumerate(
        (
            ("object.get", "seed-object-v1"),
            ("object.search", "seed-object-v1"),
            ("memory.get", "seed-memory-v1"),
            ("memory.recall", "seed-memory-v1"),
        ),
        start=6000,
    ):
        started = {
            "schema": schema,
            "run_id": run_id,
            "event": "request",
            "phase": "started",
            "request_id": _uuid(index),
            "wire_operation": operation,
            "transport": "uds",
        }
        records.extend(
            (
                started,
                {
                    **started,
                    "phase": "completed",
                    "typed_response_ok": True,
                    "evidence_ref": evidence_ref,
                    "match_count": 1,
                },
            )
        )
    records.extend(
        (
            {
                "schema": schema,
                "run_id": run_id,
                "event": "assertions",
                "phase": "completed",
                "seeded_object_present_in_evidence_ids": True,
                "seeded_memory_present_in_evidence_ids": True,
                "reported_citations_subset_of_evidence_ids": True,
                "workflow_performed_no_object_or_memory_writeback": True,
                "model_answer_body_recorded": False,
                "seeded_object_evidence_ref": "seed-object-v1",
                "seeded_memory_evidence_ref": "seed-memory-v1",
            },
            {
                "schema": schema,
                "run_id": run_id,
                "event": "provider",
                "phase": "completed",
                "backend": reader["backend"],
                "model": reader["model"],
                "fallback_used": False,
            },
        )
    )
    return records


@pytest.fixture
def dogfood_artifacts(tmp_path: Path, valid_dogfood_capture: dict) -> tuple[ArtifactInputs, Path]:
    plico = tmp_path / "plico-source"
    agents = tmp_path / "agents-source"
    for root in (plico, agents):
        root.mkdir(mode=0o775)
        root.chmod(0o775)
    for relative in (
        "src",
        "tests",
        "benchmarks/src",
        "benchmarks/tests",
        "benchmarks/scripts",
        "benchmarks/configs",
    ):
        (plico / relative).mkdir(parents=True, mode=0o775)
        (plico / relative).chmod(0o775)
    for relative, content in {
        "Cargo.toml": "[package]\nname='fixture'\n",
        "Cargo.lock": "# lock\n",
        "benchmarks/pyproject.toml": "[project]\nname='fixture-bench'\n",
        "benchmarks/README.md": "fixture\n",
        "src/lib.rs": "pub fn fixture() {}\n",
        "tests/empty.rs": "",
        "benchmarks/src/collector.py": "VALUE = 1\n",
    }.items():
        path = plico / relative
        path.write_text(content, encoding="utf-8")
        path.chmod(0o664)
    for relative in ("plico_api", "tests", "agents"):
        (agents / relative).mkdir(mode=0o775)
        (agents / relative).chmod(0o775)
    for relative, content in {
        "pyproject.toml": "[project]\nname='fixture-agents'\n",
        "uv.lock": "version = 1\n",
        "README.md": "fixture agents\n",
        "workflow.py": "VALUE = 1\n",
        "plico_api/__init__.py": "",
        "tests/test_live.py": "def test_live(): pass\n",
        "agents/config.py": "VALUE = 2\n",
    }.items():
        path = agents / relative
        path.write_text(content, encoding="utf-8")
        path.chmod(0o664)
    binary = tmp_path / "plicod"
    binary.write_bytes(b"#!/bin/sh\nexit 0\n")
    binary.chmod(0o700)
    uds_socket = tmp_path / "plico.sock"
    os.mknod(uds_socket, stat.S_IFSOCK | 0o600)
    daemon_trace = tmp_path / "daemon.jsonl"
    daemon_trace.write_text(
        "".join(
            json.dumps(item, sort_keys=True, separators=(",", ":")) + "\n"
            for item in _daemon_records(valid_dogfood_capture)
        ),
        encoding="utf-8",
    )
    daemon_trace.chmod(0o600)
    reader_trace = tmp_path / "reader.jsonl"
    reader_trace.write_text(
        "".join(
            json.dumps(item, sort_keys=True, separators=(",", ":")) + "\n"
            for item in _reader_records(valid_dogfood_capture)
        ),
        encoding="utf-8",
    )
    reader_trace.chmod(0o600)
    canary = tmp_path / "canary.json"
    _write_private(
        canary, {"schema": "plico.p3a.privacy-canary/v1", "values": ["PRIVATE-CANARY-1234"]}
    )
    provider = valid_dogfood_capture["embedding_provider"]
    probe = tmp_path / "ollama-probe.json"
    _write_private(
        probe,
        {
            "schema": "plico.p3a.ollama-probe/v1",
            "configured_exact_tag": provider["exact_model_tag"],
            "exact_tag_match_count": 1,
            "model_digest_before": provider["model_digest_before"],
            "model_digest_after": provider["model_digest_after"],
            "server_version": provider["server_version"],
            "api_contract": provider["api_contract"],
            "raw_dimension": 768,
            "effective_dimension": 768,
            "requested_target_dimension": None,
            "adaptive_prefix_contract_id": "provider-native-input-v1",
            "normalization": "provider_native",
            "transform_contract_id": "provider-native-document-v1",
            "probe_vector_count": 1,
            "probe_dimension": 768,
            "probe_all_finite": True,
            "probe_nonzero": True,
            "probe_l2_norm": 12.5,
        },
    )
    vault = tmp_path / "vault"
    (vault / "memory-ledger" / "objects").mkdir(parents=True, mode=0o700)
    (vault / "memory-ledger" / "roots").mkdir(mode=0o700)
    (vault / "projection-store" / "manifest").mkdir(parents=True, mode=0o700)
    (vault / "projection-store" / "artifacts").mkdir(mode=0o700)
    for directory in (
        vault,
        vault / "memory-ledger",
        vault / "memory-ledger" / "objects",
        vault / "memory-ledger" / "roots",
        vault / "projection-store",
        vault / "projection-store" / "manifest",
        vault / "projection-store" / "artifacts",
    ):
        directory.chmod(0o700)
    for relative, content in (("objects/" + "a" * 64, b"{}"), ("roots/active", b"{}")):
        path = vault / "memory-ledger" / relative
        path.write_bytes(content)
        path.chmod(0o600)
    checkpoints = [tmp_path / f"canonical-{index}.json" for index in range(4)]
    pre_daemon = valid_dogfood_capture["restart_replay"]["pre_restart"]["daemon_instance_id"]
    post_daemon = valid_dogfood_capture["restart_replay"]["post_restart"]["daemon_instance_id"]
    for sequence, (checkpoint, phase, daemon_id) in enumerate(
        zip(
            checkpoints,
            ("before_rebuild", "after_rebuild", "before_restart", "after_restart"),
            (pre_daemon, pre_daemon, pre_daemon, post_daemon),
            strict=True,
        ),
        start=1,
    ):
        collect_canonical_checkpoint(
            vault=vault,
            output=checkpoint,
            bundle_run_id=valid_dogfood_capture["bundle_run_id"],
            phase=phase,
            daemon_instance_id=daemon_id,
            sequence=sequence,
        )
    zero_checkpoints = [tmp_path / f"v1-zero-{index}.json" for index in range(2)]
    for sequence, (checkpoint, phase) in enumerate(
        zip(
            zero_checkpoints,
            ("before_v1_reject", "after_v1_reject"),
            strict=True,
        ),
        start=1,
    ):
        collect_v1_zero_state_checkpoint(
            vault=vault,
            output=checkpoint,
            bundle_run_id=valid_dogfood_capture["bundle_run_id"],
            phase=phase,
            daemon_instance_id=pre_daemon,
            sequence=sequence,
        )
    capture = tmp_path / "capture.json"
    _write_private(capture, valid_dogfood_capture)
    return (
        ArtifactInputs(
            plicod_binary=binary,
            uds_socket=uds_socket,
            plico_root=plico,
            plico_agents_root=agents,
            uv_lock=agents / "uv.lock",
            daemon_trace=daemon_trace,
            reader_trace=reader_trace,
            canonical_before_rebuild=checkpoints[0],
            canonical_after_rebuild=checkpoints[1],
            canonical_before_restart=checkpoints[2],
            canonical_after_restart=checkpoints[3],
            canary=canary,
            ollama_probe=probe,
            canonical_vault=vault,
            v1_zero_before=zero_checkpoints[0],
            v1_zero_after=zero_checkpoints[1],
        ),
        capture,
    )
