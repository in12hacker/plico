"""V1-B release evidence over the real daemon and offline migrator."""

from __future__ import annotations

import base64
import hashlib
import json
import os
import signal
import socket
import stat
import subprocess
import tempfile
import threading
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

from plico_benchmarks.core.client import (
    PROTOCOL,
    PUBLIC_OPERATION_CATALOG,
    PlicoClient,
    PlicoProtocolError,
)
from plico_benchmarks.core.metrics import latency_percentiles
from plico_benchmarks.core.reporter import Report
from plico_benchmarks.suites.base import SuiteBase

PROJECT_ROOT = Path(__file__).resolve().parents[4]
OWNER_ROLE = "personal-owner"


class V1BReleaseSuite(SuiteBase):
    """One fail-closed V1-B run; it makes no cross-run superiority claim."""

    name = "v1b-release"
    description = "Canonical ledger, restart, policy, migration, and projection boundary"

    def setup(self) -> None:
        if self.samples not in (None, 1):
            raise ValueError("v1b-release is one destructive lifecycle run; --samples must be 1")
        self._plicod = _required_executable("PLICO_BENCH_PLICOD", "target/debug/plicod")
        self._migrator = _required_executable(
            "PLICO_BENCH_MIGRATOR", "target/debug/plico-memory-migrate"
        )
        self._workspace = tempfile.TemporaryDirectory(prefix="plico-v1b-release-")
        workspace = Path(self._workspace.name)
        self._canonical_root = workspace / "canonical-vault"
        self._migration_root = workspace / "migration-vault"
        self._daemon: subprocess.Popen[bytes] | None = None
        self._daemon_log_handle: Any | None = None
        self._daemon_logs: list[Path] = []
        self._input_artifacts = [
            _file_artifact(self._plicod, "plicod_binary"),
            _file_artifact(self._migrator, "offline_migrator_binary"),
            _file_artifact(Path(__file__), "v1b_release_suite_source"),
            _file_artifact(PROJECT_ROOT / "benchmarks/configs/benchmark.yaml", "benchmark_config"),
        ]
        self._external_evidence: list[dict[str, Any]] = []
        self._load_external_reader_evidence()
        self._source_state: dict[str, Any] = {"kind": "not_observed"}
        self._start_daemon(self._canonical_root)

    def run(self) -> list[dict[str, Any]]:
        results: list[dict[str, Any]] = []
        try:
            results.extend(self._canonical_protocol_run())
            self._stop_daemon()
            results.extend(self._migration_policy_run())
        finally:
            self._stop_daemon()
        results.append(self._trace_evidence_result())
        return results

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        phases: dict[str, dict[str, int | float]] = {}
        for result in raw:
            phase = str(result["phase"])
            aggregate = phases.setdefault(
                phase,
                {
                    "samples": 0,
                    "latency_ms": 0.0,
                    "request_bytes": 0,
                    "response_bytes": 0,
                    "failures": 0,
                },
            )
            aggregate["samples"] += int(result.get("count", 0))
            aggregate["latency_ms"] += float(result.get("phase_elapsed_ms", 0.0))
            aggregate["request_bytes"] += int(result.get("request_bytes", 0))
            aggregate["response_bytes"] += int(result.get("response_bytes", 0))
            aggregate["failures"] += int(result.get("failure_count", 0))
        return {
            "overall": {
                "single_run_only": True,
                "comparative_inference": "not_available_single_run",
                "operations_observed": len(raw),
                "samples_observed": self.evaluated_sample_count(),
                "failure_count": sum(int(item.get("failure_count", 0)) for item in raw),
            },
            "phases": phases,
        }

    def report(self, metrics: dict[str, Any]) -> Report:
        evidence_ledger = [_redacted_evidence(item) for item in self._raw_results]
        evidence_bytes = json.dumps(evidence_ledger, sort_keys=True, separators=(",", ":")).encode()
        self._input_artifacts.append(
            {
                "role": "embedded_v1b_evidence_ledger",
                "file_name": "embedded:evidence_ledger",
                "bytes": len(evidence_bytes),
                "sha256": hashlib.sha256(evidence_bytes).hexdigest(),
            }
        )
        return Report(
            {
                "metadata": {
                    "suite": self.name,
                    "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                    "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                },
                "config": {
                    "run_kind": "single_local_release_evidence",
                    "protocol": PROTOCOL,
                    "canonical_transport": "uds",
                    "policy_transports": ["uds", "tcp"],
                    "canonical_embedding_backend": os.environ.get("EMBEDDING_BACKEND", "stub"),
                    "latency_boundaries": {
                        "public": "length-prefixed daemon request/response",
                        "migration": "offline process start through verified JSON report",
                        "restart": "SIGTERM flush through restarted public replay response",
                    },
                    "projection_claim": (
                        "The stub provider is reported as identity-unavailable by "
                        "projection.status; "
                        "canonical reads remain valid and no vector or thermal claim is made"
                    ),
                },
                "metrics": metrics,
                "evidence_ledger": evidence_ledger,
                "raw_results": self._raw_results,
            }
        )

    def evaluated_sample_count(self) -> int:
        return sum(int(item.get("count", 0)) for item in self._raw_results)

    def evaluated_samples_by_operation(self) -> dict[str, int]:
        return {str(item["operation"]): int(item.get("count", 0)) for item in self._raw_results}

    def input_artifacts(self) -> list[dict[str, Any]]:
        return list(self._input_artifacts)

    def source_watermark(self) -> dict[str, Any] | str:
        return self._source_state

    def external_evidence(self) -> list[dict[str, Any]]:
        return list(self._external_evidence)

    def _load_external_reader_evidence(self) -> None:
        configured = os.environ.get("PLICO_BENCH_EXTERNAL_READER_TRACE")
        if configured is None:
            return
        trace_path = Path(configured)
        run_id = os.environ.get("PLICO_BENCH_EXTERNAL_READER_RUN_ID")
        backend = os.environ.get("PLICO_BENCH_EXTERNAL_READER_BACKEND")
        model = os.environ.get("PLICO_BENCH_EXTERNAL_READER_MODEL")
        if not run_id or not backend or not model:
            raise ValueError("external reader trace requires run_id, backend, and model metadata")
        metadata = trace_path.stat()
        if stat.S_IMODE(metadata.st_mode) != 0o600:
            raise ValueError("external reader trace must be owner-only")
        records = []
        for line in trace_path.read_text(encoding="utf-8").splitlines():
            record = json.loads(line)
            if not isinstance(record, dict):
                raise ValueError("external reader trace contains a non-object event")
            records.append(record)
        workflow_run_ids = {record["run_id"] for record in records if "run_id" in record}
        if workflow_run_ids != {run_id}:
            raise ValueError("external reader trace run_id does not match its binding")
        completed_workflow = {
            record.get("event") for record in records if record.get("phase") == "completed"
        }
        if not {"workflow.analyst", "workflow.reporter"}.issubset(completed_workflow):
            raise ValueError("external reader trace did not complete reader/report phases")
        successful_operations = {
            record.get("operation")
            for record in records
            if record.get("event") == "transport.domain_result" and record.get("ok") is True
        }
        required_operations = {
            "object.put",
            "memory.create",
            "object.search",
            "memory.recall",
            "session.start",
            "session.end",
            "memory.delete",
        }
        if not required_operations.issubset(successful_operations):
            raise ValueError("external reader trace is missing a successful public operation")
        artifact = _file_artifact(trace_path, "external_real_llm_trace")
        self._input_artifacts.append(artifact)
        self._external_evidence.append(
            {
                "relationship": "linked_not_scored_in_v1b_release_run",
                "workflow_run_id": run_id,
                "outcome": "pytest_1_of_1_pass",
                "backend": backend,
                "model": model,
                "transport": "uds",
                "trace_sha256": artifact["sha256"],
                "trace_bytes": artifact["bytes"],
                "independent_runs_observed": 1,
                "comparative_inference": "not_available_single_run",
            }
        )

    def _canonical_protocol_run(self) -> list[dict[str, Any]]:
        results: list[dict[str, Any]] = []
        initial = _ledger_observation(self._canonical_root)

        catalog, measured = self._public_call(
            "protocol.capability_catalog",
            "protocol",
            self.client.capabilities_describe,
            initial,
        )
        operations = catalog.get("operations")
        if operations != list(PUBLIC_OPERATION_CATALOG) or len(operations) != len(set(operations)):
            raise RuntimeError("daemon capability catalog is not the exact personal protocol")
        measured["catalog_operation_count"] = len(operations)
        results.append(measured)
        results.append(_mutating_disconnect_probe())

        _, readiness = self._public_call(
            "runtime.readiness", "protocol", self.client.runtime_readiness
        )
        results.append(readiness)

        object_result, object_put = self._public_call(
            "object.put", "protocol", lambda: self.client.object_put("v1b object sentinel")
        )
        object_put["client_attempt_limit"] = 1
        results.append(object_put)
        cid = _required_string(object_result, "cid", "object.put")
        fetched, object_get = self._public_call(
            "object.get", "protocol", lambda: self.client.object_get(cid)
        )
        encoded_content = fetched.get("content_base64")
        if (
            not isinstance(encoded_content, str)
            or base64.b64decode(encoded_content, validate=True) != b"v1b object sentinel"
        ):
            raise RuntimeError("object.get did not return the seeded object")
        results.append(object_get)
        searched, object_search = self._public_call(
            "object.search", "protocol", lambda: self.client.object_search("v1b object sentinel")
        )
        if not any(hit.get("cid") == cid for hit in searched.get("hits", [])):
            raise RuntimeError("object.search did not return the seeded object")
        object_search["seeded_hit_verified"] = True
        results.append(object_search)

        created, create = self._public_call(
            "memory.create_ack",
            "canonical",
            lambda: self.client.memory_create(
                "v1b canonical original sentinel", ["v1b", "canonical"]
            ),
        )
        create["client_attempt_limit"] = 1
        results.append(create)
        original_id = _entry_id(created, "memory.create")

        _, memory_get = self._public_call(
            "memory.get", "canonical", lambda: self.client.memory_get(original_id)
        )
        results.append(memory_get)
        recalled, memory_recall = self._public_call(
            "memory.recall",
            "canonical",
            lambda: self.client.memory_recall("v1b canonical original sentinel"),
        )
        if not any(
            hit.get("entry", {}).get("entry_id") == original_id for hit in recalled.get("hits", [])
        ):
            raise RuntimeError("memory.recall did not return the seeded canonical memory")
        memory_recall["seeded_hit_verified"] = True
        results.append(memory_recall)

        status, projection = self._public_call(
            "projection.status_identity_unavailable",
            "projection",
            lambda: self.client.projection_status(original_id),
        )
        observation = status.get("status")
        if (
            status.get("kind") != "memory_embedding"
            or not isinstance(observation, dict)
            or observation.get("observation") != "unavailable"
            or observation.get("reason") != "identity_unavailable"
        ):
            raise RuntimeError(
                "stub-backed run did not report typed identity-unavailable projection"
            )
        projection["projection_observation"] = "unavailable"
        projection["projection_unavailable_reason"] = "identity_unavailable"
        projection["projection_completion_claimed"] = False
        results.append(projection)

        canonical_during_unavailable, canonical_read = self._public_call(
            "memory.get_during_projection_unavailable",
            "canonical",
            lambda: self.client.memory_get(original_id),
        )
        if _entry_id({"entry": canonical_during_unavailable}, "memory.get") != original_id:
            raise RuntimeError("canonical memory changed while projection was unavailable")
        canonical_read["canonical_read_succeeded"] = True
        results.append(canonical_read)

        rebuild_before = _ledger_observation(self._canonical_root)
        rebuild_started = time.perf_counter()
        try:
            self.client.projection_rebuild_current(original_id)
        except PlicoProtocolError as error:
            if error.code != "DEPENDENCY_UNAVAILABLE":
                raise RuntimeError(
                    "identity-unavailable rebuild returned the wrong typed error"
                ) from error
        else:
            raise RuntimeError("identity-unavailable rebuild unexpectedly succeeded")
        rebuild_after = _ledger_observation(self._canonical_root)
        if rebuild_after != rebuild_before:
            raise RuntimeError("rejected projection rebuild changed the canonical ledger")
        results.append(
            _operation_result(
                "projection.rebuild_identity_unavailable",
                "projection",
                (time.perf_counter() - rebuild_started) * 1000,
                _exchange_bytes(self.client),
                rebuild_before,
                rebuild_after,
                observed_error_code="DEPENDENCY_UNAVAILABLE",
                client_attempt_limit=1,
                projection_unavailable_reason="identity_unavailable",
            )
        )

        updated, update = self._public_call(
            "memory.update_ack",
            "canonical",
            lambda: self.client.memory_update(original_id, "v1b canonical corrected sentinel"),
        )
        update["client_attempt_limit"] = 1
        results.append(update)
        updated_id = _entry_id(updated, "memory.update")
        if updated_id == original_id:
            raise RuntimeError("memory.update did not append a new revision")

        conflict_before = _ledger_observation(self._canonical_root)
        conflict_started = time.perf_counter()
        try:
            self.client.memory_update(original_id, "stale rewrite must conflict")
        except PlicoProtocolError as error:
            if error.code != "CONFLICT":
                raise RuntimeError("stale update did not return typed CONFLICT") from error
        else:
            raise RuntimeError("stale update unexpectedly succeeded")
        conflict_after = _ledger_observation(self._canonical_root)
        if conflict_after != conflict_before:
            raise RuntimeError("stale expected-head conflict mutated the canonical ledger")
        results.append(
            _operation_result(
                "memory.expected_head_conflict",
                "canonical",
                (time.perf_counter() - conflict_started) * 1000,
                _exchange_bytes(self.client),
                conflict_before,
                conflict_after,
                observed_error_code="CONFLICT",
                client_attempt_limit=1,
            )
        )

        replay_created, replay_create = self._public_call(
            "memory.create_replay_anchor",
            "canonical",
            lambda: self.client.memory_create("v1b restart replay sentinel", ["v1b"]),
        )
        replay_create["client_attempt_limit"] = 1
        results.append(replay_create)
        replay_id = _entry_id(replay_created, "memory.create")

        deleted, delete = self._public_call(
            "memory.delete_ack",
            "canonical",
            lambda: self.client.memory_delete(updated_id),
        )
        delete["client_attempt_limit"] = 1
        results.append(delete)
        delete_generation = delete["generation_after"]

        repeated, idempotent = self._public_call(
            "memory.delete_idempotent",
            "canonical",
            lambda: self.client.memory_delete(updated_id),
        )
        if repeated != deleted or idempotent["generation_after"] != delete_generation:
            raise RuntimeError("repeated delete appended a second tombstone")
        idempotent["generation_delta"] = 0
        idempotent["client_attempt_limit"] = 1
        results.append(idempotent)

        session, session_start = self._public_call(
            "session.start", "protocol", self.client.session_start
        )
        session_start["client_attempt_limit"] = 1
        results.append(session_start)
        session_id = _required_string(session, "session_id", "session.start")
        _, session_end = self._public_call(
            "session.end", "protocol", lambda: self.client.session_end(session_id)
        )
        session_end["client_attempt_limit"] = 1
        results.append(session_end)

        before_restart = _ledger_observation(self._canonical_root)
        restart_started = time.perf_counter()
        self._stop_daemon()
        self._start_daemon(self._canonical_root)
        replayed = self.client.memory_get(replay_id)
        if _entry_id({"entry": replayed}, "memory.get") != replay_id:
            raise RuntimeError("restart did not replay the live canonical revision")
        try:
            self.client.memory_get(updated_id)
        except PlicoProtocolError as error:
            if error.code != "NOT_FOUND":
                raise RuntimeError("restart did not preserve the tombstone") from error
        else:
            raise RuntimeError("restart resurrected a deleted revision")
        after_restart = _ledger_observation(self._canonical_root)
        if after_restart != before_restart:
            raise RuntimeError("restart changed the canonical ledger watermark")
        exchange = _exchange_bytes(self.client)
        results.append(
            _operation_result(
                "memory.restart_replay",
                "restart",
                (time.perf_counter() - restart_started) * 1000,
                exchange,
                before_restart,
                after_restart,
                count=2,
                live_revision_replayed=True,
                tombstone_preserved=True,
            )
        )
        self._canonical_watermark = after_restart
        return results

    def _migration_policy_run(self) -> list[dict[str, Any]]:
        fixture = _write_legacy_fixture(self._migration_root, self.seed)
        self._input_artifacts.extend(fixture["artifacts"])
        authorization = fixture["authorization"]
        revision_ids = fixture["revision_ids"]
        results = []
        migrate_report: dict[str, Any] | None = None
        for command, expected_status in (
            ("inspect", "verified"),
            ("dry-run", "verified"),
            ("migrate", "published"),
        ):
            before_bytes = _tree_bytes(self._migration_root)
            started = time.perf_counter()
            report, request_bytes, response_bytes = self._run_migrator(command, authorization)
            elapsed = (time.perf_counter() - started) * 1000
            if report.get("status") != expected_status:
                raise RuntimeError(f"migration {command} did not report {expected_status}")
            if report.get("source_entries") != 3 or report.get("source_streams") != 3:
                raise RuntimeError(f"migration {command} reported wrong source counts")
            if command == "inspect":
                if report.get("target_revisions") is not None:
                    raise RuntimeError("migration inspect unexpectedly constructed a target")
            elif (
                report.get("target_revisions") != 3
                or report.get("target_policies") != 3
                or report.get("target_relations") != 0
            ):
                raise RuntimeError(f"migration {command} reported wrong target counts")
            if command == "migrate":
                migrate_report = report
                if report.get("rollback_backup_created") is not True:
                    raise RuntimeError("migration did not preserve a rollback backup")
            result = _operation_result(
                f"migration.{command.replace('-', '_')}",
                "migration",
                elapsed,
                {"request": request_bytes, "response": response_bytes},
                None,
                None,
                source_entries=report.get("source_entries"),
                source_streams=report.get("source_streams"),
                target_revisions=report.get("target_revisions"),
                target_policies=report.get("target_policies"),
                target_relations=report.get("target_relations"),
                target_root_hash=report.get("target_root_hash"),
                vault_bytes_before=before_bytes,
                vault_bytes_after=_tree_bytes(self._migration_root),
            )
            results.append(result)

        migration_watermark = _ledger_observation(self._migration_root)
        if migration_watermark["generation"] != 1:
            raise RuntimeError("offline migration did not publish generation 1")
        if (
            migrate_report is None
            or migrate_report.get("target_root_hash") != migration_watermark["root_hash"]
        ):
            raise RuntimeError("migration report root hash did not bind the published target")

        self._start_daemon(self._migration_root)
        owner = self.client
        port = self._daemon_port
        role_a = PlicoClient(host="127.0.0.1", port=port, bearer_token="role-a-secret")
        role_b = PlicoClient(host="127.0.0.1", port=port, bearer_token="role-b-secret")
        role_c = PlicoClient(host="127.0.0.1", port=port, bearer_token="role-c-secret")
        try:
            results.append(
                self._policy_recall_result(
                    "policy.private_recall",
                    "orchidprivatev1b",
                    revision_ids["private"],
                    ((role_a, True), (role_b, False), (owner, True)),
                )
            )
            results.append(
                self._policy_recall_result(
                    "policy.shared_recall",
                    "saffronsharedv1b",
                    revision_ids["shared"],
                    ((role_b, True),),
                )
            )
            results.append(
                self._policy_recall_result(
                    "policy.group_recall",
                    "cobaltgroupv1b",
                    revision_ids["group"],
                    ((role_c, True), (role_b, False), (owner, True)),
                )
            )
        finally:
            role_a.close()
            role_b.close()
            role_c.close()
        self._stop_daemon()

        self._start_daemon(self._migration_root)
        restarted = self._policy_recall_result(
            "policy.recall_after_restart",
            "orchidprivatev1b",
            revision_ids["private"],
            ((self.client, True),),
        )
        restarted["restart_consistent"] = True
        results.append(restarted)
        final_migration = _ledger_observation(self._migration_root)
        if final_migration != migration_watermark:
            raise RuntimeError("policy-only recall or restart changed the canonical generation")
        self._source_state = {
            "kind": "v1b_canonical_ledger/v1",
            "canonical_run": self._canonical_watermark,
            "migrated_run": final_migration,
        }
        return results

    def _policy_recall_result(
        self,
        operation: str,
        query: str,
        target_revision_id: str,
        cases: tuple[tuple[PlicoClient, bool], ...],
    ) -> dict[str, Any]:
        before = _ledger_observation(self._migration_root)
        started = time.perf_counter()
        request_bytes = 0
        response_bytes = 0
        for case_index, (client, expected_visible) in enumerate(cases):
            result = client.memory_recall(query, limit=10)
            actual_visible = any(
                hit.get("entry", {}).get("entry_id") == target_revision_id
                for hit in result.get("hits", [])
            )
            if actual_visible is not expected_visible:
                raise RuntimeError(
                    f"{operation} case {case_index} expected target visibility "
                    f"{expected_visible}, observed {actual_visible}"
                )
            exchange = _exchange_bytes(client)
            request_bytes += exchange["request"]
            response_bytes += exchange["response"]
        after = _ledger_observation(self._migration_root)
        if after != before:
            raise RuntimeError(f"{operation} mutated the canonical ledger")
        return _operation_result(
            operation,
            "policy",
            (time.perf_counter() - started) * 1000,
            {"request": request_bytes, "response": response_bytes},
            before,
            after,
            count=len(cases),
            policy_assertions=len(cases),
        )

    def _public_call(
        self,
        operation: str,
        phase: str,
        call: Callable[[], dict[str, Any]],
        before: dict[str, Any] | None = None,
    ) -> tuple[dict[str, Any], dict[str, Any]]:
        before = before or _ledger_observation(self._canonical_root)
        started = time.perf_counter()
        response = call()
        elapsed = (time.perf_counter() - started) * 1000
        after = _ledger_observation(self._canonical_root)
        return response, _operation_result(
            operation,
            phase,
            elapsed,
            _exchange_bytes(self.client),
            before,
            after,
        )

    def _start_daemon(self, root: Path) -> None:
        if self._daemon is not None:
            raise RuntimeError("daemon already started")
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        self._daemon_port = _free_tcp_port()
        environment = os.environ.copy()
        environment.update(
            {
                "EMBEDDING_BACKEND": "stub",
                "LLM_BACKEND": "stub",
                "PLICO_KG_AUTO_EXTRACT": "false",
                "PLICO_AGENT_AUTH_MODE": "required",
                "RUST_LOG": (
                    "warn,plicod=debug,plico::memory=debug,"
                    "plico::kernel::public_service=debug,"
                    "plico::kernel::ops::memory=debug,"
                    "plico::kernel::ops::projection_runtime=debug,"
                    "plico::kernel::ops::projection_controller=debug,"
                    "plico::memory::projection=debug"
                ),
            }
        )
        self._daemon = subprocess.Popen(
            [
                str(self._plicod),
                "start",
                "--root",
                str(root),
                "--host",
                "127.0.0.1",
                "--port",
                str(self._daemon_port),
            ],
            cwd=PROJECT_ROOT,
            env=environment,
            stdout=self._open_daemon_log(),
            stderr=subprocess.STDOUT,
        )
        uds_path = root / "plico.sock"
        client = PlicoClient(uds_path=str(uds_path), timeout=10.0, max_retries=2)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self._daemon.poll() is not None:
                raise RuntimeError("plicod exited before readiness")
            try:
                readiness = client.runtime_readiness()
                if isinstance(readiness.get("ready"), bool):
                    self.client.close()
                    self.client = client
                    return
            except (OSError, PlicoProtocolError):
                client.close()
            time.sleep(0.05)
        client.close()
        raise TimeoutError("local UDS daemon did not become ready")

    def _stop_daemon(self) -> None:
        self.client.close()
        daemon = getattr(self, "_daemon", None)
        if daemon is None:
            return
        if daemon.poll() is None:
            daemon.send_signal(signal.SIGTERM)
            try:
                daemon.wait(timeout=15)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=5)
        self._daemon = None
        if self._daemon_log_handle is not None:
            self._daemon_log_handle.close()
            self._daemon_log_handle = None

    def _open_daemon_log(self) -> Any:
        path = Path(self._workspace.name) / f"daemon-{len(self._daemon_logs) + 1}.log"
        descriptor = os.open(path, os.O_CREAT | os.O_APPEND | os.O_WRONLY, 0o600)
        self._daemon_logs.append(path)
        self._daemon_log_handle = os.fdopen(descriptor, "ab", buffering=0)
        return self._daemon_log_handle

    def _trace_evidence_result(self) -> dict[str, Any]:
        payload = b"".join(path.read_bytes() for path in self._daemon_logs)
        forbidden = [
            b"v1b canonical original sentinel",
            b"v1b canonical corrected sentinel",
            b"v1b restart replay sentinel",
            b"private orchid v1b",
            b"shared saffron v1b",
            b"group cobalt v1b",
            b"orchidprivatev1b",
            b"saffronsharedv1b",
            b"cobaltgroupv1b",
            b"owner-secret",
            b"role-a-secret",
            b"role-b-secret",
            b"role-c-secret",
            os.fsencode(self._canonical_root),
            os.fsencode(self._migration_root),
            self._canonical_watermark["root_hash"].encode(),
            self._source_state["migrated_run"]["root_hash"].encode(),
        ]
        if any(canary in payload for canary in forbidden):
            raise RuntimeError("daemon trace leaked a V1-B privacy canary")
        for required in (b"operation", b"phase", b"outcome"):
            if required not in payload:
                raise RuntimeError("daemon trace omitted required structured flow fields")
        configured = os.environ.get("PLICO_BENCH_TRACE_OUTPUT")
        trace_path = (
            Path(configured)
            if configured
            else Path(self._workspace.name) / f"v1b_release_{self.run_id}.daemon.log"
        )
        _write_private_bytes(trace_path, payload)
        self._input_artifacts.append(_file_artifact(trace_path, "daemon_trace_evidence"))
        return _operation_result(
            "observability.trace_canary",
            "observability",
            0.0,
            {"request": 0, "response": 0},
            None,
            None,
            count=len(self._daemon_logs),
            trace_file_name=trace_path.name,
            trace_bytes=len(payload),
            trace_sha256=hashlib.sha256(payload).hexdigest(),
            privacy_canaries_absent=True,
            structured_fields_observed=["operation", "phase", "outcome"],
        )

    def _run_migrator(
        self, command: str, authorization: dict[str, Any]
    ) -> tuple[dict[str, Any], int, int]:
        payload = json.dumps(authorization, separators=(",", ":")).encode()
        completed = subprocess.run(
            [str(self._migrator), command, "--root", str(self._migration_root)],
            cwd=PROJECT_ROOT,
            input=payload,
            capture_output=True,
            check=False,
            timeout=30,
        )
        if completed.returncode != 0:
            try:
                category = json.loads(completed.stderr).get("category", "migration_rejected")
            except (json.JSONDecodeError, UnicodeDecodeError):
                category = "migration_process_failure"
            raise RuntimeError(f"migration {command} failed [{category}]")
        try:
            report = json.loads(completed.stdout)
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise RuntimeError(f"migration {command} returned invalid JSON") from error
        if not isinstance(report, dict) or report.get("operation") != command.replace("-", "_"):
            raise RuntimeError(f"migration {command} returned a mismatched report")
        return report, len(payload), len(completed.stdout)


def _operation_result(
    operation: str,
    phase: str,
    elapsed_ms: float,
    exchange: dict[str, Any],
    before: dict[str, Any] | None,
    after: dict[str, Any] | None,
    *,
    count: int = 1,
    **metadata: Any,
) -> dict[str, Any]:
    percentiles = latency_percentiles([elapsed_ms])
    result: dict[str, Any] = {
        "operation": operation,
        "phase": phase,
        "count": count,
        "phase_elapsed_ms": round(elapsed_ms, 3),
        "p50_ms": percentiles["p50"],
        "p95_ms": percentiles["p95"],
        "p99_ms": percentiles["p99"],
        "request_bytes": exchange["request"],
        "response_bytes": exchange["response"],
        "status": "measured",
        "failure_count": 0,
        **metadata,
    }
    for key in (
        "request_id",
        "wire_operation",
        "attempt_count",
        "frame_sent",
        "response_observed",
    ):
        if key in exchange:
            result[key] = exchange[key]
    if before is not None and after is not None:
        result.update(
            {
                "generation_before": before["generation"],
                "generation_after": after["generation"],
                "generation_delta": after["generation"] - before["generation"],
                "revision_watermark_before": before["revision_watermark"],
                "revision_watermark_after": after["revision_watermark"],
                "ledger_bytes_before": before["ledger_storage_bytes"],
                "ledger_bytes_after": after["ledger_storage_bytes"],
                "ledger_bytes_delta": (
                    after["ledger_storage_bytes"] - before["ledger_storage_bytes"]
                ),
            }
        )
    return result


def _redacted_evidence(result: dict[str, Any]) -> dict[str, Any]:
    """Keep release evidence even when personal raw benchmark rows are omitted."""
    allowed = (
        "operation",
        "wire_operation",
        "phase",
        "count",
        "phase_elapsed_ms",
        "p50_ms",
        "p95_ms",
        "p99_ms",
        "request_bytes",
        "response_bytes",
        "status",
        "failure_count",
        "request_id",
        "attempt_count",
        "frame_sent",
        "response_observed",
        "observed_error_code",
        "client_attempt_limit",
        "generation_before",
        "generation_after",
        "generation_delta",
        "revision_watermark_before",
        "revision_watermark_after",
        "ledger_bytes_before",
        "ledger_bytes_after",
        "ledger_bytes_delta",
        "source_entries",
        "source_streams",
        "target_revisions",
        "target_policies",
        "target_relations",
        "vault_bytes_before",
        "vault_bytes_after",
        "policy_assertions",
        "seeded_hit_verified",
        "canonical_read_succeeded",
        "projection_observation",
        "projection_unavailable_reason",
        "projection_completion_claimed",
        "live_revision_replayed",
        "tombstone_preserved",
        "restart_consistent",
        "privacy_canaries_absent",
        "structured_fields_observed",
        "trace_file_name",
        "trace_bytes",
        "fault_injection",
        "injected_fault",
    )
    evidence = {key: result[key] for key in allowed if key in result}
    evidence["typed_outcome"] = result.get("observed_error_code", result.get("status"))
    if result.get("fault_injection") is True:
        evidence["fault_cases"] = [
            {
                key: case[key]
                for key in (
                    "operation",
                    "request_id",
                    "attempt_count",
                    "frame_sent",
                    "response_observed",
                    "outcome",
                )
            }
            for case in result.get("fault_cases", [])
        ]
    return evidence


def _ledger_observation(vault_root: Path) -> dict[str, Any]:
    ledger = vault_root / "memory-ledger"
    active = ledger / "roots/active"
    pointer_bytes = active.read_bytes()
    pointer = json.loads(pointer_bytes)
    if pointer.get("schema") != "plico.memory.root-pointer/v1":
        raise RuntimeError("active pointer has an unexpected schema")
    root_hash = _required_string(pointer, "root_hash", "active pointer")
    if len(root_hash) != 64 or any(character not in "0123456789abcdef" for character in root_hash):
        raise RuntimeError("active pointer contains a non-canonical root hash")
    root_bytes = (ledger / "objects" / root_hash).read_bytes()
    root = json.loads(root_bytes)
    if root.get("schema") != "plico.memory.root/v1":
        raise RuntimeError("ledger root has an unexpected schema")
    return {
        "generation": int(root["generation"]),
        "revision_watermark": int(root["revision_watermark"]),
        "policy_watermark": int(root["policy_watermark"]),
        "relation_watermark": int(root["relation_watermark"]),
        "root_hash": root_hash,
        "ledger_storage_bytes": _tree_bytes(ledger),
    }


def _write_legacy_fixture(root: Path, seed: int) -> dict[str, Any]:
    root.mkdir(mode=0o700, parents=True)
    tokens = {
        role: {
            "agent_id": role,
            "token": token,
            "issued_at": 1,
            "expires_at": None,
            "capabilities": [],
        }
        for role, token in (
            (OWNER_ROLE, "owner-secret"),
            ("role-a", "role-a-secret"),
            ("role-b", "role-b-secret"),
            ("role-c", "role-c-secret"),
        )
    }
    token_path = root / "agent_tokens.json"
    token_path.write_bytes(json.dumps(tokens, separators=(",", ":")).encode())
    token_path.chmod(0o600)
    scopes: tuple[tuple[str, str, Any], ...] = (
        ("private", "orchidprivatev1b private orchid v1b", "Private"),
        ("shared", "saffronsharedv1b shared saffron v1b", "Shared"),
        ("group", "cobaltgroupv1b group cobalt v1b", {"Group": "research"}),
    )
    entries = []
    revision_ids = {}
    for index, (policy_kind, content, scope) in enumerate(scopes):
        revision_id = uuid.uuid5(uuid.NAMESPACE_URL, f"plico-v1b-release:{seed}:legacy:{index}")
        revision_ids[policy_kind] = str(revision_id)
        entries.append(
            {
                "id": str(revision_id),
                "agent_id": "legacy-agent",
                "tenant_id": "default",
                "tier": "Working",
                "content": {"Text": content},
                "importance": 50,
                "access_count": 0,
                "last_accessed": 1,
                "created_at": 1,
                "tags": ["v1b", "legacy"],
                "embedding": None,
                "ttl_ms": None,
                "original_ttl_ms": None,
                "scope": scope,
                "memory_type": "Semantic",
                "causal_parent": None,
                "supersedes": None,
                "superseded_by": None,
                "deleted_at": None,
            }
        )
    entry_bytes = json.dumps(entries, ensure_ascii=False, separators=(",", ":")).encode()
    cid = hashlib.sha256(entry_bytes).hexdigest()
    envelope_bytes = json.dumps(
        {
            "cid": cid,
            "data": list(entry_bytes),
            "meta": {
                "content_type": "Structured",
                "tags": ["memory"],
                "created_by": "plico:memory-persister",
                "created_at": 1,
                "intent": None,
                "tenant_id": "default",
                "scope": "private",
            },
        },
        separators=(",", ":"),
    ).encode()
    shard = root / "cas" / cid[:2]
    shard.mkdir(mode=0o700, parents=True)
    object_path = shard / cid[2:]
    object_path.write_bytes(envelope_bytes)
    index_path = root / "memory_index.json"
    index_path.write_bytes(
        json.dumps(
            {
                "agents": {
                    "legacy-agent": [{"tier": "working", "cid": cid, "entry_count": len(entries)}]
                }
            },
            separators=(",", ":"),
        ).encode()
    )
    return {
        "authorization": {
            "owner_bearer": "owner-secret",
            "role_mappings": [{"legacy_agent_id": "legacy-agent", "target_role_id": "role-a"}],
            "group_mappings": [{"legacy_group_id": "research", "target_role_ids": ["role-c"]}],
        },
        "revision_ids": revision_ids,
        "artifacts": [
            _file_artifact(index_path, "legacy_memory_index"),
            _file_artifact(object_path, "legacy_memory_snapshot_envelope"),
        ],
    }


def _required_executable(environment_key: str, relative_default: str) -> Path:
    path = Path(os.environ.get(environment_key, PROJECT_ROOT / relative_default)).resolve()
    if not path.is_file() or not os.access(path, os.X_OK):
        raise FileNotFoundError(f"required executable is unavailable: {environment_key}")
    return path


def _file_artifact(path: Path, role: str) -> dict[str, Any]:
    payload = path.read_bytes()
    return {
        "role": role,
        "file_name": path.name,
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def _write_private_bytes(path: Path, payload: bytes) -> None:
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_CREAT | os.O_TRUNC | os.O_WRONLY, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
    finally:
        path.chmod(0o600)


def _tree_bytes(root: Path) -> int:
    if not root.exists():
        return 0
    total = 0
    for path in root.rglob("*"):
        metadata = path.lstat()
        if stat.S_ISREG(metadata.st_mode):
            total += metadata.st_size
    return total


def _free_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def _mutating_disconnect_probe() -> dict[str, Any]:
    """Prove that each public mutation sends once and never retries after disconnect."""
    mutation_calls: tuple[tuple[str, Callable[[PlicoClient], Any]], ...] = (
        ("object.put", lambda client: client.object_put("disconnect probe")),
        ("memory.create", lambda client: client.memory_create("disconnect probe")),
        (
            "projection.rebuild",
            lambda client: client.projection_rebuild_current(
                "00000000-0000-4000-8000-000000000003"
            ),
        ),
        (
            "memory.update",
            lambda client: client.memory_update(
                "00000000-0000-4000-8000-000000000001", "disconnect probe"
            ),
        ),
        (
            "memory.delete",
            lambda client: client.memory_delete("00000000-0000-4000-8000-000000000001"),
        ),
        ("session.start", lambda client: client.session_start()),
        (
            "session.end",
            lambda client: client.session_end("00000000-0000-4000-8000-000000000002"),
        ),
    )
    started = time.perf_counter()
    observations = []
    total_request_bytes = 0
    for operation, call in mutation_calls:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(0.3)
        port = int(listener.getsockname()[1])
        frames: list[tuple[int, str]] = []

        def disconnect_after_frame() -> None:
            try:
                while True:
                    connection, _ = listener.accept()
                    with connection:
                        header = _recv_exact(connection, 4)
                        length = int.from_bytes(header, "big")
                        request = json.loads(_recv_exact(connection, length))
                        frames.append((length + 4, str(request["request_id"])))
            except (TimeoutError, OSError, ConnectionError):
                return

        server = threading.Thread(target=disconnect_after_frame, daemon=True)
        server.start()
        client = PlicoClient(
            host="127.0.0.1",
            port=port,
            bearer_token="fault-probe-bearer",
            timeout=1,
            max_retries=9,
        )
        try:
            call(client)
        except (ConnectionError, OSError, TimeoutError):
            pass
        else:
            raise RuntimeError(f"{operation} disconnect probe unexpectedly returned a response")
        finally:
            client.close()
        server.join(timeout=2)
        listener.close()
        if len(frames) != 1:
            raise RuntimeError(f"{operation} attempted {len(frames)} frames after disconnect")
        total_request_bytes += frames[0][0]
        observations.append(
            {
                "operation": operation,
                "request_id": frames[0][1],
                "attempt_count": 1,
                "frame_sent": True,
                "response_observed": False,
                "outcome": "connection_closed_after_frame",
            }
        )
    return _operation_result(
        "protocol.mutating_disconnect_no_retry",
        "protocol_fault",
        (time.perf_counter() - started) * 1000,
        {"request": total_request_bytes, "response": 0},
        None,
        None,
        count=len(observations),
        fault_injection=True,
        injected_fault="disconnect_after_complete_request_frame",
        fault_cases=observations,
    )


def _recv_exact(connection: socket.socket, size: int) -> bytes:
    payload = bytearray()
    while len(payload) < size:
        chunk = connection.recv(size - len(payload))
        if not chunk:
            raise ConnectionError("fault probe connection ended before a complete frame")
        payload.extend(chunk)
    return bytes(payload)


def _entry_id(result: dict[str, Any], operation: str) -> str:
    entry = result.get("entry")
    if not isinstance(entry, dict):
        raise RuntimeError(f"{operation} returned no entry")
    return _required_string(entry, "entry_id", operation)


def _required_string(value: dict[str, Any], key: str, operation: str) -> str:
    result = value.get(key)
    if not isinstance(result, str) or not result:
        raise RuntimeError(f"{operation} returned no {key}")
    return result


def _exchange_bytes(client: PlicoClient) -> dict[str, Any]:
    if client.last_exchange is None:
        raise RuntimeError("client did not record request/response bytes")
    return dict(client.last_exchange)
