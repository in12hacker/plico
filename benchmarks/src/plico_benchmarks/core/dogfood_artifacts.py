"""Artifact-backed inputs for deterministic P3-A dogfood evidence."""

from __future__ import annotations

import json
import math
import os
import re
import stat
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

from plico_benchmarks.core.dogfood_io import (
    canonical_json,
    open_directory_no_follow,
    read_regular,
    read_regular_artifact,
    sha256,
    strict_json_lines,
    strict_json_object,
)
from plico_benchmarks.core.dogfood_schema import (
    BinaryCapture,
    CanonicalFingerprintCapture,
    DogfoodCapture,
    FileCapture,
    MemoryGetEvidence,
    MemoryRecallEvidence,
    PostRestartVerification,
    SourceManifest,
    StatusEvidence,
    TraceCapture,
    canonical_uuid,
    provider_compatibility_id,
)
from plico_benchmarks.core.dogfood_schema import (
    sha256 as validate_sha256,
)

MAX_SOURCE_FILE_BYTES = 16 * 1024 * 1024
MAX_SOURCE_TOTAL_BYTES = 256 * 1024 * 1024
MAX_SOURCE_FILES = 4096
TRACE_SCHEMA = "plico.p3a.dogfood-trace-record/v1"
READER_TRACE_SCHEMA = "plico.p3a.reader-trace-record/v1"
INVENTORY_SCHEMA = "plico.p3a.canonical-inventory/v1"
CANARY_SCHEMA = "plico.p3a.privacy-canary/v1"
_CACHE_PARTS = {".git", ".venv", "__pycache__", ".pytest_cache", ".ruff_cache"}
_SECRET_PATTERN = re.compile(rb"(?i)(?:sk-[A-Za-z0-9_-]{8,}|bearer\s+[A-Za-z0-9._~+/-]{4,})")


@dataclass(frozen=True)
class ArtifactInputs:
    plicod_binary: Path
    uds_socket: Path
    plico_root: Path
    plico_agents_root: Path
    uv_lock: Path
    daemon_trace: Path
    reader_trace: Path
    canonical_before_rebuild: Path
    canonical_after_rebuild: Path
    canonical_before_restart: Path
    canonical_after_restart: Path
    canary: Path
    ollama_probe: Path
    canonical_vault: Path
    v1_zero_before: Path
    v1_zero_after: Path

    def paths(self) -> tuple[Path, ...]:
        return (
            self.plicod_binary,
            self.uds_socket,
            self.plico_root,
            self.plico_agents_root,
            self.uv_lock,
            self.daemon_trace,
            self.reader_trace,
            self.canonical_before_rebuild,
            self.canonical_after_rebuild,
            self.canonical_before_restart,
            self.canonical_after_restart,
            self.canary,
            self.ollama_probe,
            self.canonical_vault,
            self.v1_zero_before,
            self.v1_zero_after,
        )


@dataclass(frozen=True)
class VerifiedArtifacts:
    plicod: BinaryCapture
    plico_source: SourceManifest
    agents_source: SourceManifest
    agents_uv_lock: FileCapture
    ollama_probe: FileCapture
    canonical_fingerprint: CanonicalFingerprintCapture
    daemon_trace: TraceCapture
    auxiliary_request_ids: tuple[str, ...]
    reader_trace_sha256: str
    reader_trace_bytes: int
    reader_trace_records: int
    reader_trace_request_count: int
    canaries: tuple[bytes, ...]
    v1_zero_state: dict[str, Any]
    post_restart_verification: PostRestartVerification


def verify_artifact_inputs(capture: DogfoodCapture, inputs: ArtifactInputs) -> VerifiedArtifacts:
    _verify_uds_socket(inputs.uds_socket)
    canaries = _read_canaries(inputs.canary)
    ollama_probe = _read_ollama_probe(capture, inputs.ollama_probe)
    binary = read_regular_artifact(
        inputs.plicod_binary,
        private=False,
        maximum=256 * 1024 * 1024,
        required_mode=0o700,
    )
    if not binary.mode & stat.S_IXUSR:
        raise ValueError("plicod binary is not owner-executable")
    plicod = BinaryCapture.model_validate(
        {
            "role": "plicod_binary",
            "file_name": "plicod",
            "bytes": binary.size,
            "sha256": sha256(binary.payload),
            "trust": "sealed_owner_only_executable_0700",
        }
    )
    plico_source = _scan_plico(inputs.plico_root)
    agents_source = _scan_agents(inputs.plico_agents_root)
    uv_artifact = read_regular_artifact(
        inputs.uv_lock, private=False, maximum=MAX_SOURCE_FILE_BYTES
    )
    expected_uv = next((item for item in agents_source.files if item.path == "uv.lock"), None)
    if (
        expected_uv is None
        or expected_uv.bytes != uv_artifact.size
        or expected_uv.sha256 != sha256(uv_artifact.payload)
    ):
        raise ValueError("explicit uv.lock does not match the enumerated plico-agents lockfile")

    checkpoint_paths = (
        inputs.canonical_before_rebuild,
        inputs.canonical_after_rebuild,
        inputs.canonical_before_restart,
        inputs.canonical_after_restart,
    )
    phases = ("before_rebuild", "after_rebuild", "before_restart", "after_restart")
    daemon_ids = (
        capture.restart_replay.pre_restart.daemon_instance_id,
        capture.restart_replay.pre_restart.daemon_instance_id,
        capture.restart_replay.pre_restart.daemon_instance_id,
        capture.restart_replay.post_restart.daemon_instance_id,
    )
    checkpoints = tuple(
        _read_checkpoint(path, capture.bundle_run_id, phase, daemon_id, sequence)
        for sequence, (path, phase, daemon_id) in enumerate(
            zip(checkpoint_paths, phases, daemon_ids, strict=True), start=1
        )
    )
    identities = {(item[1], item[2]) for item in checkpoints}
    if len(identities) != 4:
        raise ValueError("canonical checkpoint inputs must be four distinct files")
    inventory_payloads = tuple(item[0] for item in checkpoints)
    if len(set(inventory_payloads)) != 1:
        raise ValueError("canonical inventories differ across projection and restart checkpoints")
    if collect_canonical_inventory(inputs.canonical_vault) != inventory_payloads[3]:
        raise ValueError("post-restart live canonical tree differs from its checkpoint inventory")
    inventory_hash = sha256(inventory_payloads[0])
    fingerprint = CanonicalFingerprintCapture.model_validate(
        {
            "schema": "plico.canonical-tree-fingerprint/v1",
            "before_projection_rebuild_sha256": inventory_hash,
            "after_projection_rebuild_sha256": inventory_hash,
            "before_restart_sha256": inventory_hash,
            "after_restart_sha256": inventory_hash,
            "unchanged_across_projection_rebuild": True,
            "unchanged_across_restart": True,
        }
    )

    daemon_payload = read_regular(inputs.daemon_trace, private=True)
    reader_payload = read_regular(inputs.reader_trace, private=True)
    _scan_secrets(daemon_payload, canaries)
    _scan_secrets(reader_payload, canaries)
    daemon_trace, auxiliary, post_restart = _verify_daemon_trace(capture, daemon_payload)
    reader_requests = _verify_reader_trace(capture, reader_payload)
    v1_zero_state = _verify_v1_zero_state(capture, inputs.v1_zero_before, inputs.v1_zero_after)
    return VerifiedArtifacts(
        plicod=plicod,
        plico_source=plico_source,
        agents_source=agents_source,
        agents_uv_lock=FileCapture(
            path="uv.lock", bytes=uv_artifact.size, sha256=sha256(uv_artifact.payload)
        ),
        ollama_probe=ollama_probe,
        canonical_fingerprint=fingerprint,
        daemon_trace=daemon_trace,
        auxiliary_request_ids=auxiliary,
        reader_trace_sha256=sha256(reader_payload),
        reader_trace_bytes=len(reader_payload),
        reader_trace_records=len(strict_json_lines(reader_payload)),
        reader_trace_request_count=reader_requests,
        canaries=canaries,
        v1_zero_state=v1_zero_state,
        post_restart_verification=post_restart,
    )


def _verify_uds_socket(path: Path) -> None:
    if getattr(os, "O_PATH", 0) == 0:
        raise ValueError("UDS evidence requires Linux O_PATH identity verification")
    parent_fd = open_directory_no_follow(path.parent)
    descriptor = -1
    try:
        expected = os.stat(path.name, dir_fd=parent_fd, follow_symlinks=False)
        descriptor = os.open(
            path.name,
            os.O_PATH | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_fd,
        )
        before = os.fstat(descriptor)
        if (
            not stat.S_ISSOCK(before.st_mode)
            or before.st_uid != os.geteuid()
            or stat.S_IMODE(before.st_mode) != 0o600
            or _stable_stat(before) != _stable_stat(expected)
            or _stable_stat(os.fstat(descriptor)) != _stable_stat(before)
        ):
            raise ValueError("UDS artifact is not one stable owner-only socket")
    except OSError as error:
        raise ValueError("UDS artifact could not be safely opened") from error
    finally:
        if descriptor >= 0:
            os.close(descriptor)
        os.close(parent_fd)


def reject_output_overlap(output: Path, capture: Path, inputs: ArtifactInputs) -> None:
    output_absolute = os.path.abspath(output)
    guarded = (capture, *inputs.paths())
    for value in guarded:
        candidate = os.path.abspath(value)
        if output_absolute == candidate:
            raise ValueError("evidence output collides with an input")
    for root in (inputs.plico_root, inputs.plico_agents_root, inputs.canonical_vault):
        root_absolute = os.path.abspath(root)
        common = os.path.commonpath((output_absolute, root_absolute))
        if common in {root_absolute, output_absolute}:
            raise ValueError("evidence output must not overlap an input root")


def _scan_plico(root: Path) -> SourceManifest:
    required = ("Cargo.toml", "Cargo.lock", "benchmarks/pyproject.toml", "benchmarks/README.md")
    optional = ("build.rs", "benchmarks/uv.lock")
    directories = (
        "src",
        "tests",
        "benchmarks/src",
        "benchmarks/tests",
        "benchmarks/scripts",
        "benchmarks/configs",
    )
    return _scan_project(root, required=required, optional=optional, directories=directories)


def _scan_agents(root: Path) -> SourceManifest:
    return _scan_project(
        root,
        required=("pyproject.toml", "uv.lock", "README.md"),
        optional=(),
        directories=("plico_api", "tests", "agents"),
        top_level_python=True,
    )


def _scan_project(
    root: Path,
    *,
    required: tuple[str, ...],
    optional: tuple[str, ...],
    directories: tuple[str, ...],
    top_level_python: bool = False,
) -> SourceManifest:
    root_fd = _open_directory(root)
    rows: dict[str, FileCapture] = {}
    root_before = os.fstat(root_fd)
    try:
        for relative in required:
            rows[relative] = _read_source_relative(root_fd, relative, required=True)
        for relative in optional:
            item = _read_source_relative(root_fd, relative, required=False)
            if item is not None:
                rows[relative] = item
        for relative in directories:
            directory_fd = _open_relative_directory(root_fd, relative)
            try:
                _walk_source_dir(directory_fd, PurePosixPath(relative), rows)
            finally:
                os.close(directory_fd)
        if top_level_python:
            initial_entries = list(os.scandir(root_fd))
            for entry in initial_entries:
                if entry.name.endswith(".py"):
                    rows[entry.name] = _read_source_at(root_fd, entry.name, entry.name)
            if [entry.name for entry in os.scandir(root_fd)] != [
                entry.name for entry in initial_entries
            ]:
                raise ValueError("source root changed during enumeration")
        if _stable_stat(os.fstat(root_fd)) != _stable_stat(root_before):
            raise ValueError("source root metadata changed during enumeration")
    finally:
        os.close(root_fd)
    ordered = [rows[path] for path in sorted(rows)]
    if not ordered or len(ordered) > MAX_SOURCE_FILES:
        raise ValueError("source inventory has an invalid file count")
    total = sum(item.bytes for item in ordered)
    if total > MAX_SOURCE_TOTAL_BYTES:
        raise ValueError("source inventory exceeds its aggregate byte limit")
    row_bytes = b"".join(
        canonical_json(item.model_dump(mode="json")).rstrip(b"\n") + b"\n" for item in ordered
    )
    return SourceManifest.model_validate(
        {
            "schema": "plico.source-file-manifest/v1",
            "file_count": len(ordered),
            "total_bytes": total,
            "aggregate_sha256": sha256(row_bytes),
            "row_format": "canonical_jcs_file_capture_jsonl_v1",
            "inventory_rule": "fixed_execution_source_selection_v1",
            "trust": "live_same_euid_non_world_writable",
            "files": [item.model_dump(mode="json") for item in ordered],
        }
    )


def _walk_source_dir(
    directory_fd: int, prefix: PurePosixPath, rows: dict[str, FileCapture]
) -> None:
    before = os.fstat(directory_fd)
    initial_entries = sorted(os.scandir(directory_fd), key=lambda item: item.name)
    for entry in initial_entries:
        if entry.name in _CACHE_PARTS or entry.name.endswith((".pyc", ".pyo")):
            continue
        relative = str(prefix / entry.name)
        if entry.name == ".env" or entry.name.startswith(".env."):
            raise ValueError("source tree contains an excluded secret file")
        metadata = entry.stat(follow_symlinks=False)
        if stat.S_ISDIR(metadata.st_mode):
            child_fd = _open_child_directory(directory_fd, entry.name, metadata)
            try:
                _walk_source_dir(child_fd, prefix / entry.name, rows)
            finally:
                os.close(child_fd)
        elif stat.S_ISREG(metadata.st_mode):
            rows[relative] = _read_source_at(directory_fd, entry.name, relative, metadata)
        else:
            raise ValueError("source tree contains a symlink or special file")
    if sorted(entry.name for entry in os.scandir(directory_fd)) != [
        entry.name for entry in initial_entries
    ] or _stable_stat(os.fstat(directory_fd)) != _stable_stat(before):
        raise ValueError("source directory changed during enumeration")


def _read_source_relative(root_fd: int, relative: str, *, required: bool) -> FileCapture | None:
    parts = PurePosixPath(relative).parts
    directory_fd = os.dup(root_fd)
    try:
        for part in parts[:-1]:
            next_fd = _open_child_directory(directory_fd, part)
            os.close(directory_fd)
            directory_fd = next_fd
        try:
            return _read_source_at(directory_fd, parts[-1], relative)
        except FileNotFoundError:
            if required:
                raise ValueError("required source input is missing") from None
            return None
    finally:
        os.close(directory_fd)


def _read_source_at(
    directory_fd: int, name: str, relative: str, expected: os.stat_result | None = None
) -> FileCapture:
    _validate_relative(relative)
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, dir_fd=directory_fd)
    try:
        before = os.fstat(descriptor)
        if expected is not None and (before.st_dev, before.st_ino) != (
            expected.st_dev,
            expected.st_ino,
        ):
            raise ValueError("source entry changed during enumeration")
        mode = stat.S_IMODE(before.st_mode)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_uid != os.geteuid()
            or mode & 0o002
            or before.st_size < 0
            or before.st_size > MAX_SOURCE_FILE_BYTES
        ):
            raise ValueError("source entry has an invalid type, owner, mode, or size")
        payload = _read_exact(descriptor, before.st_size)
        after = os.fstat(descriptor)
        if len(payload) != before.st_size or _stable_stat(after) != _stable_stat(before):
            raise ValueError("source entry changed while being read")
        return FileCapture(path=relative, bytes=len(payload), sha256=sha256(payload))
    finally:
        os.close(descriptor)


def _open_directory(path: Path) -> int:
    descriptor = open_directory_no_follow(path)
    _validate_directory_fd(descriptor)
    return descriptor


def _open_relative_directory(root_fd: int, relative: str) -> int:
    descriptor = os.dup(root_fd)
    try:
        for part in PurePosixPath(relative).parts:
            next_fd = _open_child_directory(descriptor, part)
            os.close(descriptor)
            descriptor = next_fd
        return descriptor
    except Exception:
        os.close(descriptor)
        raise


def _open_child_directory(parent_fd: int, name: str, expected: os.stat_result | None = None) -> int:
    flags = (
        os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    )
    descriptor = os.open(name, flags, dir_fd=parent_fd)
    try:
        metadata = os.fstat(descriptor)
        _validate_directory_metadata(metadata)
        if expected is not None and (metadata.st_dev, metadata.st_ino) != (
            expected.st_dev,
            expected.st_ino,
        ):
            raise ValueError("source directory changed during enumeration")
        return descriptor
    except Exception:
        os.close(descriptor)
        raise


def _validate_directory_fd(descriptor: int) -> None:
    _validate_directory_metadata(os.fstat(descriptor))


def _validate_directory_metadata(metadata: os.stat_result) -> None:
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) & 0o002
    ):
        raise ValueError("source root has an invalid type, owner, or writable mode")


def _validate_relative(value: str) -> None:
    path = PurePosixPath(value)
    if (
        not value
        or value.startswith(("/", "~"))
        or "\\" in value
        or ".." in path.parts
        or str(path) != value
        or any(ord(character) < 32 or ord(character) == 127 for character in value)
    ):
        raise ValueError("source manifest contains an unsafe relative path")


def _read_ollama_probe(capture: DogfoodCapture, path: Path) -> FileCapture:
    payload = read_regular(path, private=True, maximum=64 * 1024)
    value = strict_json_object(payload)
    fields = {
        "schema",
        "configured_exact_tag",
        "exact_tag_match_count",
        "model_digest_before",
        "model_digest_after",
        "server_version",
        "api_contract",
        "raw_dimension",
        "effective_dimension",
        "requested_target_dimension",
        "adaptive_prefix_contract_id",
        "normalization",
        "transform_contract_id",
        "probe_vector_count",
        "probe_dimension",
        "probe_all_finite",
        "probe_nonzero",
        "probe_l2_norm",
    }
    if set(value) != fields or value.get("schema") != "plico.p3a.ollama-probe/v1":
        raise ValueError("Ollama probe artifact has an invalid schema")
    provider = capture.embedding_provider
    expected = {
        "configured_exact_tag": provider.exact_model_tag,
        "exact_tag_match_count": provider.exact_tag_match_count,
        "model_digest_before": provider.model_digest_before,
        "model_digest_after": provider.model_digest_after,
        "server_version": provider.server_version,
        "api_contract": provider.api_contract,
        "raw_dimension": provider.raw_dimension,
        "effective_dimension": provider.effective_dimension,
        "requested_target_dimension": provider.requested_target_dimension,
        "adaptive_prefix_contract_id": provider.adaptive_prefix_contract_id,
        "normalization": provider.normalization,
        "transform_contract_id": provider.transform_contract_id,
    }
    if any(value.get(key) != expected_value for key, expected_value in expected.items()):
        raise ValueError("Ollama probe artifact differs from the typed capture")
    norm = value.get("probe_l2_norm")
    if (
        value.get("probe_vector_count") != 1
        or value.get("probe_dimension") != provider.raw_dimension
        or value.get("probe_all_finite") is not True
        or value.get("probe_nonzero") is not True
        or not isinstance(norm, (int, float))
        or isinstance(norm, bool)
        or not math.isfinite(norm)
        or norm <= 0
    ):
        raise ValueError("Ollama probe did not establish a finite nonzero embedding shape")
    if provider.normalization == "l2_after_matryoshka_truncation_v1" and abs(norm - 1.0) > 1e-4:
        raise ValueError("Ollama adaptive probe does not satisfy the frozen L2 tolerance")
    if payload != canonical_json(value):
        raise ValueError("Ollama probe artifact is not canonical JSON")
    compatibility = provider_compatibility_id(
        {
            "exact_model_tag": value["configured_exact_tag"],
            "model_digest_before": value["model_digest_before"],
            "server_version": value["server_version"],
            "api_contract": value["api_contract"],
            "raw_dimension": value["raw_dimension"],
            "effective_dimension": value["effective_dimension"],
            "requested_target_dimension": value["requested_target_dimension"],
            "adaptive_prefix_contract_id": value["adaptive_prefix_contract_id"],
            "normalization": value["normalization"],
        }
    )
    if compatibility != provider.provider_compatibility_id:
        raise ValueError("Ollama probe does not produce the captured compatibility identity")
    return FileCapture(path="ollama-probe.json", bytes=len(payload), sha256=sha256(payload))


def collect_canonical_inventory(vault: Path) -> bytes:
    """Collect the exact private memory-ledger tree without following links."""
    vault_fd = _open_directory(vault)
    rows: list[dict[str, Any]] = []
    try:
        if stat.S_IMODE(os.fstat(vault_fd).st_mode) != 0o700:
            raise ValueError("canonical vault root is not mode 0700")
        ledger_fd = _open_child_directory(vault_fd, "memory-ledger")
        try:
            if stat.S_IMODE(os.fstat(ledger_fd).st_mode) != 0o700:
                raise ValueError("canonical ledger directory is not mode 0700")
            rows.append(
                {
                    "path": "memory-ledger",
                    "kind": "directory",
                    "mode": "0700",
                    "bytes": 0,
                    "sha256": None,
                }
            )
            _walk_canonical(ledger_fd, PurePosixPath("memory-ledger"), rows)
        finally:
            os.close(ledger_fd)
    finally:
        os.close(vault_fd)
    rows.sort(key=lambda item: item["path"])
    if not rows or len(rows) > MAX_SOURCE_FILES:
        raise ValueError("canonical tree inventory has an invalid entry count")
    return canonical_json({"schema": INVENTORY_SCHEMA, "entries": rows})


def collect_projection_inventory(vault: Path) -> bytes:
    """Collect the exact private projection-store tree without following links."""
    vault_fd = _open_directory(vault)
    rows: list[dict[str, Any]] = []
    try:
        if stat.S_IMODE(os.fstat(vault_fd).st_mode) != 0o700:
            raise ValueError("projection vault root is not mode 0700")
        projection_fd = _open_child_directory(vault_fd, "projection-store")
        try:
            if stat.S_IMODE(os.fstat(projection_fd).st_mode) != 0o700:
                raise ValueError("projection store directory is not mode 0700")
            rows.append(
                {
                    "path": "projection-store",
                    "kind": "directory",
                    "mode": "0700",
                    "bytes": 0,
                    "sha256": None,
                }
            )
            _walk_canonical(projection_fd, PurePosixPath("projection-store"), rows)
        finally:
            os.close(projection_fd)
    finally:
        os.close(vault_fd)
    rows.sort(key=lambda item: item["path"])
    return canonical_json({"schema": INVENTORY_SCHEMA, "entries": rows})


def _verify_v1_zero_state(capture: DogfoodCapture, before: Path, after: Path) -> dict[str, Any]:
    expected_daemon = capture.v1_reject.daemon_instance_id
    values = []
    identities = []
    for sequence, (path, phase) in enumerate(
        ((before, "before_v1_reject"), (after, "after_v1_reject")), start=1
    ):
        artifact = read_regular_artifact(path, private=True)
        identities.append((artifact.device, artifact.inode))
        value = strict_json_object(artifact.payload)
        if (
            set(value)
            != {
                "schema",
                "bundle_run_id",
                "phase",
                "daemon_instance_id",
                "sequence",
                "canonical_entries",
                "projection_entries",
            }
            or value.get("schema") != "plico.p3a.v1-zero-state-checkpoint/v1"
        ):
            raise ValueError("v1 zero-state checkpoint schema is invalid")
        if (
            value.get("bundle_run_id") != capture.bundle_run_id
            or value.get("phase") != phase
            or value.get("daemon_instance_id") != expected_daemon
            or value.get("sequence") != sequence
            or artifact.payload != canonical_json(value)
        ):
            raise ValueError("v1 zero-state checkpoint binding is invalid")
        _validate_inventory_entries(value["canonical_entries"])
        _validate_inventory_entries(value["projection_entries"])
        values.append(value)
    if len(set(identities)) != 2:
        raise ValueError("v1 zero-state checkpoints must be distinct files")
    state_before = canonical_json(
        {
            "canonical_entries": values[0]["canonical_entries"],
            "projection_entries": values[0]["projection_entries"],
        }
    )
    state_after = canonical_json(
        {
            "canonical_entries": values[1]["canonical_entries"],
            "projection_entries": values[1]["projection_entries"],
        }
    )
    if state_before != state_after:
        raise ValueError("v1 rejected request changed canonical or projection zero-state")
    return {
        "schema": "plico.p3a.v1-zero-state-evidence/v1",
        "before_sha256": sha256(state_before),
        "after_sha256": sha256(state_after),
        "canonical_entry_count": len(values[0]["canonical_entries"]),
        "projection_entry_count": len(values[0]["projection_entries"]),
        "unchanged": True,
    }


def _walk_canonical(directory_fd: int, prefix: PurePosixPath, rows: list[dict[str, Any]]) -> None:
    before = os.fstat(directory_fd)
    entries = sorted(os.scandir(directory_fd), key=lambda item: item.name)
    for entry in entries:
        relative = str(prefix / entry.name)
        _validate_relative(relative)
        metadata = entry.stat(follow_symlinks=False)
        if stat.S_ISDIR(metadata.st_mode):
            child_fd = _open_child_directory(directory_fd, entry.name, metadata)
            try:
                if stat.S_IMODE(os.fstat(child_fd).st_mode) != 0o700:
                    raise ValueError("canonical directory is not mode 0700")
                rows.append(
                    {
                        "path": relative,
                        "kind": "directory",
                        "mode": "0700",
                        "bytes": 0,
                        "sha256": None,
                    }
                )
                _walk_canonical(child_fd, prefix / entry.name, rows)
            finally:
                os.close(child_fd)
        elif stat.S_ISREG(metadata.st_mode):
            used_bytes = sum(item["bytes"] for item in rows if item["kind"] == "file")
            if metadata.st_size < 0 or used_bytes + metadata.st_size > MAX_SOURCE_TOTAL_BYTES:
                raise ValueError("canonical inventory resource exhausted")
            item = _read_canonical_file(directory_fd, entry.name, relative, metadata)
            rows.append(item)
        else:
            raise ValueError("canonical tree contains a symlink or special file")
    if sorted(entry.name for entry in os.scandir(directory_fd)) != [
        entry.name for entry in entries
    ] or _stable_stat(os.fstat(directory_fd)) != _stable_stat(before):
        raise ValueError("canonical directory changed during collection")
    if len(rows) > MAX_SOURCE_FILES:
        raise ValueError("canonical inventory resource exhausted")


def _read_canonical_file(
    directory_fd: int, name: str, relative: str, expected: os.stat_result
) -> dict[str, Any]:
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    noatime = getattr(os, "O_NOATIME", 0)
    try:
        descriptor = os.open(name, flags | noatime, dir_fd=directory_fd)
    except PermissionError:
        descriptor = os.open(name, flags, dir_fd=directory_fd)
    try:
        before = os.fstat(descriptor)
        if (
            (before.st_dev, before.st_ino) != (expected.st_dev, expected.st_ino)
            or not stat.S_ISREG(before.st_mode)
            or before.st_uid != os.geteuid()
            or stat.S_IMODE(before.st_mode) != 0o600
            or before.st_size < 0
            or before.st_size > MAX_SOURCE_FILE_BYTES
        ):
            raise ValueError("canonical file has an invalid identity, mode, or size")
        payload = _read_exact(descriptor, before.st_size)
        if len(payload) != before.st_size or _stable_stat(os.fstat(descriptor)) != _stable_stat(
            before
        ):
            raise ValueError("canonical file changed during collection")
        return {
            "path": relative,
            "kind": "file",
            "mode": "0600",
            "bytes": len(payload),
            "sha256": sha256(payload),
        }
    finally:
        os.close(descriptor)


def _read_checkpoint(
    path: Path, run_id: str, phase: str, daemon_instance_id: str, sequence: int
) -> tuple[bytes, int, int]:
    artifact = read_regular_artifact(path, private=True)
    value = strict_json_object(artifact.payload)
    if (
        set(value)
        != {
            "schema",
            "bundle_run_id",
            "phase",
            "daemon_instance_id",
            "sequence",
            "entries",
        }
        or value.get("schema") != "plico.p3a.canonical-checkpoint/v1"
    ):
        raise ValueError("canonical inventory has an invalid schema")
    if (
        value.get("bundle_run_id") != run_id
        or value.get("phase") != phase
        or value.get("daemon_instance_id") != daemon_instance_id
        or value.get("sequence") != sequence
    ):
        raise ValueError("canonical checkpoint has an invalid run, phase, daemon, or sequence")
    entries = value.get("entries")
    _validate_inventory_entries(entries)
    if artifact.payload != canonical_json(value):
        raise ValueError("canonical inventory is not canonical JSON")
    inventory = canonical_json({"schema": INVENTORY_SCHEMA, "entries": entries})
    return inventory, artifact.device, artifact.inode


def _validate_inventory_entries(entries: Any) -> None:
    if not isinstance(entries, list) or not entries or len(entries) > MAX_SOURCE_FILES:
        raise ValueError("canonical inventory has an invalid entry count")
    previous = ""
    for item in entries:
        if not isinstance(item, dict) or set(item) != {"path", "kind", "mode", "bytes", "sha256"}:
            raise ValueError("canonical inventory row has unexpected fields")
        relative = item.get("path")
        if not isinstance(relative, str):
            raise ValueError("canonical inventory path is invalid")
        _validate_relative(relative)
        if relative <= previous:
            raise ValueError("canonical inventory paths must be strict sorted unique")
        previous = relative
        if item.get("kind") not in {"file", "directory"}:
            raise ValueError("canonical inventory kind is invalid")
        expected_mode = "0600" if item["kind"] == "file" else "0700"
        if item.get("mode") != expected_mode:
            raise ValueError("canonical inventory mode is not owner-only")
        size = item.get("bytes")
        digest = item.get("sha256")
        if not isinstance(size, int) or isinstance(size, bool) or size < 0:
            raise ValueError("canonical inventory byte count is invalid")
        if item["kind"] == "directory" and (size != 0 or digest is not None):
            raise ValueError("canonical directory inventory row is invalid")
        if item["kind"] == "file" and not isinstance(digest, str):
            raise ValueError("canonical file inventory digest is invalid")
        if isinstance(digest, str):
            validate_sha256(digest)


def _read_canaries(path: Path) -> tuple[bytes, ...]:
    payload = read_regular(path, private=True, maximum=64 * 1024)
    value = strict_json_object(payload)
    if set(value) != {"schema", "values"} or value.get("schema") != CANARY_SCHEMA:
        raise ValueError("privacy canary file has an invalid schema")
    values = value.get("values")
    if not isinstance(values, list) or not values or len(values) > 32:
        raise ValueError("privacy canary set is invalid")
    if payload != canonical_json(value):
        raise ValueError("privacy canary file is not canonical JSON")
    encoded: list[bytes] = []
    for item in values:
        if (
            not isinstance(item, str)
            or len(item) < 8
            or len(item) > 256
            or not item.isascii()
            or any(ord(character) < 33 or ord(character) == 127 for character in item)
        ):
            raise ValueError("privacy canary value is invalid")
        encoded.append(item.encode())
    if len(encoded) != len(set(encoded)):
        raise ValueError("privacy canary values must be unique")
    return tuple(encoded)


def _scan_secrets(payload: bytes, canaries: tuple[bytes, ...]) -> None:
    escaped = tuple(
        json.dumps(canary.decode(), ensure_ascii=True)[1:-1].encode() for canary in canaries
    )
    if _SECRET_PATTERN.search(payload) or any(
        canary in payload or escaped_value in payload
        for canary, escaped_value in zip(canaries, escaped, strict=True)
    ):
        raise ValueError("trace contains secret or privacy canary material")


def _verify_daemon_trace(
    capture: DogfoodCapture, payload: bytes
) -> tuple[TraceCapture, tuple[str, ...], PostRestartVerification]:
    records = strict_json_lines(payload)
    pairs: dict[str, list[dict[str, Any]]] = {}
    provider_count = 0
    v1_reject_count = 0
    restart_records: dict[str, dict[str, Any]] = {}
    after_restart = False
    for expected_sequence, record in enumerate(records, start=1):
        if record.get("event") == "restart_checkpoint" and record.get("phase") == "after":
            after_restart = True
        expected_daemon = (
            capture.restart_replay.post_restart.daemon_instance_id
            if after_restart
            else capture.restart_replay.pre_restart.daemon_instance_id
        )
        if record.get("schema") != TRACE_SCHEMA or record.get("run_id") != capture.bundle_run_id:
            raise ValueError("daemon trace record has an invalid schema or run binding")
        if (
            record.get("sequence") != expected_sequence
            or record.get("daemon_instance_id") != expected_daemon
        ):
            raise ValueError("daemon trace sequence or daemon instance is invalid")
        event = record.get("event")
        if event in {"request", "disconnect", "auxiliary_request"}:
            request_id = record.get("request_id")
            if not isinstance(request_id, str):
                raise ValueError("daemon trace request ID is missing")
            canonical_uuid(request_id)
            pairs.setdefault(request_id, []).append(record)
        elif event == "provider_identity":
            if (
                set(record)
                != {
                    "schema",
                    "run_id",
                    "event",
                    "phase",
                    "identity",
                    "sequence",
                    "daemon_instance_id",
                }
                or record.get("phase") != "verified"
            ):
                raise ValueError("provider trace record has unexpected fields")
            if record.get("identity") != capture.embedding_provider.model_dump(mode="json"):
                raise ValueError("daemon provider trace differs from the typed capture")
            provider_count += 1
        elif event == "restart_checkpoint":
            if set(record) != {
                "schema",
                "run_id",
                "event",
                "phase",
                "state",
                "sequence",
                "daemon_instance_id",
            }:
                raise ValueError("restart trace record has unexpected fields")
            phase = record.get("phase")
            if phase not in {"before", "after"} or phase in restart_records:
                raise ValueError("restart trace checkpoints are incomplete or duplicated")
            restart_records[phase] = record
        elif event == "v1_protocol_reject":
            expected = {
                "schema": TRACE_SCHEMA,
                "run_id": capture.bundle_run_id,
                "event": "v1_protocol_reject",
                "phase": "rejected",
                "sequence": record["sequence"],
                **capture.v1_reject.model_dump(mode="json"),
            }
            if record != expected:
                raise ValueError("v1 protocol rejection trace differs from the typed capture")
            v1_reject_count += 1
        else:
            raise ValueError("daemon trace contains an unsupported event")
    if provider_count != 1 or v1_reject_count != 1:
        raise ValueError("daemon trace must contain one verified provider identity")
    if [item.get("event") for item in records[:2]] != [
        "provider_identity",
        "v1_protocol_reject",
    ]:
        raise ValueError("daemon trace provider and v1 rejection order is invalid")
    expected_restart = {
        "before": capture.restart_replay.pre_restart.model_dump(mode="json"),
        "after": capture.restart_replay.post_restart.model_dump(mode="json"),
    }
    if {phase: item["state"] for phase, item in restart_records.items()} != expected_restart:
        raise ValueError("daemon trace restart checkpoints differ from the typed capture")

    public_ids = []
    disconnect_ids = []
    auxiliary_ids = []
    post_restart_evidence: dict[str, MemoryGetEvidence | MemoryRecallEvidence | StatusEvidence] = {}
    for request_id, chain in pairs.items():
        if (
            len(chain) != 2
            or chain[0].get("phase") != "started"
            or chain[1].get("sequence") != chain[0].get("sequence") + 1
            or chain[1].get("daemon_instance_id") != chain[0].get("daemon_instance_id")
        ):
            raise ValueError("daemon trace request phase chain is invalid")
        event = chain[0].get("event")
        if any(item.get("event") != event for item in chain):
            raise ValueError("daemon trace request phase chain changes event type")
        if event == "request":
            _verify_public_pair(chain, capture)
            public_ids.append(request_id)
        elif event == "disconnect":
            _verify_disconnect_pair(chain, capture)
            disconnect_ids.append(request_id)
        else:
            post_restart = _verify_auxiliary_pair(chain, capture)
            if post_restart is not None:
                operation, evidence = post_restart
                if operation in post_restart_evidence:
                    raise ValueError("post-restart response evidence is duplicated")
                post_restart_evidence[operation] = evidence
            auxiliary_ids.append(request_id)
    if public_ids != [item.request_id for item in capture.request_ledger]:
        raise ValueError("daemon trace does not contain the exact ordered public request ledger")
    if disconnect_ids != [item.request_id for item in capture.disconnect_cases]:
        raise ValueError("daemon trace does not contain the exact ordered disconnect ledger")
    all_ids = (*public_ids, *disconnect_ids, *auxiliary_ids, capture.v1_reject.request_id)
    if len(set(all_ids)) != len(all_ids):
        raise ValueError("daemon trace request IDs are not globally unique")
    restart_before = restart_records["before"]["sequence"]
    restart_after = restart_records["after"]["sequence"]
    public_sequences = [
        chain[0]["sequence"] for chain in pairs.values() if chain[0]["event"] == "request"
    ]
    post_restart_operations = [
        chain[0].get("wire_operation")
        for chain in pairs.values()
        if chain[0].get("category") == "post_restart_verification"
    ]
    if (
        not public_sequences
        or max(public_sequences) >= restart_before
        or restart_before >= restart_after
        or post_restart_operations != ["memory.get", "memory.recall", "projection.status"]
        or any(
            chain[0]["sequence"] <= restart_after
            for chain in pairs.values()
            if chain[0].get("category") == "post_restart_verification"
        )
    ):
        raise ValueError("daemon restart and post-restart verification order is invalid")
    if set(post_restart_evidence) != {"memory.get", "memory.recall", "projection.status"}:
        raise ValueError("daemon trace lacks exact post-restart response evidence")
    verified_post_restart = PostRestartVerification.model_validate(
        {
            "memory_get": post_restart_evidence["memory.get"].model_dump(mode="json"),
            "memory_recall": post_restart_evidence["memory.recall"].model_dump(mode="json"),
            "projection_status": post_restart_evidence["projection.status"].model_dump(mode="json"),
        }
    )
    trace = TraceCapture.model_validate(
        {
            "schema": "plico.p3a.dogfood-trace/v1",
            "bytes": len(payload),
            "records": len(records),
            "public_request_count": len(public_ids),
            "disconnect_request_count": len(disconnect_ids),
            "auxiliary_request_count": len(auxiliary_ids),
            "sha256": sha256(payload),
            "privacy_canary_scan_passed": True,
        }
    )
    return trace, tuple(auxiliary_ids), verified_post_restart


def _verify_public_pair(chain: list[dict[str, Any]], capture: DogfoodCapture) -> None:
    started, completed = chain
    common = {
        "schema",
        "run_id",
        "event",
        "phase",
        "request_id",
        "wire_operation",
        "transport",
        "sequence",
        "daemon_instance_id",
    }
    if (
        set(started) != common
        or started.get("transport") != "uds"
        or completed.get("phase") != "completed"
    ):
        raise ValueError("public request trace has unexpected started fields")
    operation = started.get("wire_operation")
    request_id = started.get("request_id")
    expected = next(
        (item for item in capture.request_ledger if item.request_id == request_id), None
    )
    if expected is None or operation != expected.wire_operation:
        raise ValueError("public request trace does not bind the typed capture")
    expected_completed = {
        **{key: started[key] for key in common if key not in {"phase", "sequence"}},
        "phase": "completed",
        "sequence": completed["sequence"],
        "attempt_count": expected.attempt_count,
        "frame_count": expected.frame_count,
        "typed_response_ok": expected.typed_response_ok,
        "result_assertion": expected.result_assertion,
    }
    if expected.typed_result_evidence is not None:
        expected_completed["typed_result_evidence"] = expected.typed_result_evidence.model_dump(
            mode="json"
        )
    if completed != expected_completed:
        raise ValueError("public request completion differs from the typed capture")


def _verify_disconnect_pair(chain: list[dict[str, Any]], capture: DogfoodCapture) -> None:
    started, failed = chain
    common = {
        "schema",
        "run_id",
        "event",
        "phase",
        "request_id",
        "wire_operation",
        "transport",
        "sequence",
        "daemon_instance_id",
    }
    if (
        set(started) != common
        or started.get("transport") != "uds"
        or failed.get("phase") != "ambiguous"
    ):
        raise ValueError("disconnect trace has unexpected started fields")
    request_id = started.get("request_id")
    expected = next(
        (item for item in capture.disconnect_cases if item.request_id == request_id), None
    )
    if expected is None or started.get("wire_operation") != expected.wire_operation:
        raise ValueError("disconnect trace does not bind the typed capture")
    expected_failed = {
        **{key: started[key] for key in common if key not in {"phase", "sequence"}},
        "phase": "ambiguous",
        "sequence": failed["sequence"],
        "attempt_count": expected.attempt_count,
        "frame_count": expected.frame_count,
        "response_observed": expected.response_observed,
        "outcome": expected.outcome,
    }
    if failed != expected_failed:
        raise ValueError("disconnect trace differs from the typed capture")


def _verify_auxiliary_pair(
    chain: list[dict[str, Any]], capture: DogfoodCapture
) -> tuple[str, MemoryGetEvidence | MemoryRecallEvidence | StatusEvidence] | None:
    started, completed = chain
    common = {
        "schema",
        "run_id",
        "event",
        "phase",
        "request_id",
        "category",
        "transport",
        "sequence",
        "daemon_instance_id",
    }
    if started.get("category") == "post_restart_verification":
        common.add("wire_operation")
    if (
        set(started) != common
        or started.get("transport") != "uds"
        or started.get("category")
        not in {"projection_poll", "restart_probe", "post_restart_verification"}
    ):
        raise ValueError("auxiliary trace request phase chain is invalid")
    expected_completed = {
        **started,
        "phase": "completed",
        "sequence": completed["sequence"],
        "typed_response_ok": True,
    }
    if started.get("category") != "post_restart_verification":
        if completed != expected_completed:
            raise ValueError("auxiliary trace request phase chain is invalid")
        return None
    seed = capture.seed_evidence
    restart = capture.restart_replay.post_restart
    operation = started.get("wire_operation")
    if operation == "memory.get":
        evidence: MemoryGetEvidence | MemoryRecallEvidence | StatusEvidence = (
            MemoryGetEvidence.model_validate(
                {
                    "result": "found",
                    "memory_id": seed.memory_id,
                    "revision_id": seed.revision_id,
                    "content_evidence_ref": seed.content_evidence_ref,
                }
            )
        )
    elif operation == "memory.recall":
        evidence = MemoryRecallEvidence.model_validate(
            {
                "strategy": "lexical_overlap",
                "target_memory_id": seed.memory_id,
                "target_revision_id": seed.revision_id,
                "content_evidence_ref": seed.content_evidence_ref,
                "match_count": completed.get("typed_result_evidence", {}).get("match_count"),
                "target_found": True,
            }
        )
    elif operation == "projection.status":
        evidence = StatusEvidence.model_validate(
            {
                "kind": "memory_embedding",
                "observation": restart.projection_observation,
                "state": restart.projection_state,
                "event_watermark": restart.event_watermark,
                "reconciled_source": restart.reconciled_source.model_dump(mode="json"),
                "memory_id": seed.memory_id,
                "revision_id": seed.revision_id,
                "content_evidence_ref": seed.content_evidence_ref,
                "builder_compatibility_id": restart.builder_compatibility_id,
            }
        )
    else:
        raise ValueError("post-restart verification operation is invalid")
    expected_completed["typed_result_evidence"] = evidence.model_dump(mode="json")
    if completed != expected_completed:
        raise ValueError("post-restart response evidence differs from the seeded revision")
    return operation, evidence


def _verify_reader_trace(capture: DogfoodCapture, payload: bytes) -> int:
    records = strict_json_lines(payload)
    workflow = set()
    pairs: dict[str, list[dict[str, Any]]] = {}
    assertion_seen = False
    provider_seen = False
    expected_reader = capture.real_llm_reader
    for record in records:
        if (
            record.get("schema") != READER_TRACE_SCHEMA
            or record.get("run_id") != expected_reader.workflow_run_id
        ):
            raise ValueError("reader trace record has an invalid schema or run binding")
        event = record.get("event")
        if event == "workflow":
            if (
                set(record) != {"schema", "run_id", "event", "phase", "role"}
                or record.get("phase") != "completed"
                or record.get("role") not in {"analyst", "reporter"}
            ):
                raise ValueError("reader workflow trace record is invalid")
            workflow.add(record["role"])
        elif event == "request":
            request_id = record.get("request_id")
            if not isinstance(request_id, str):
                raise ValueError("reader trace request ID is missing")
            canonical_uuid(request_id)
            pairs.setdefault(request_id, []).append(record)
        elif event == "assertions":
            expected = {
                "schema": READER_TRACE_SCHEMA,
                "run_id": expected_reader.workflow_run_id,
                "event": "assertions",
                "phase": "completed",
                "seeded_object_present_in_evidence_ids": True,
                "seeded_memory_present_in_evidence_ids": True,
                "reported_citations_subset_of_evidence_ids": True,
                "workflow_performed_no_object_or_memory_writeback": True,
                "model_answer_body_recorded": False,
                "seeded_object_evidence_ref": "seed-object-v1",
                "seeded_memory_evidence_ref": "seed-memory-v1",
            }
            if record != expected or assertion_seen:
                raise ValueError("reader trace assertions are missing or duplicated")
            assertion_seen = True
        elif event == "provider":
            expected = {
                "schema": READER_TRACE_SCHEMA,
                "run_id": expected_reader.workflow_run_id,
                "event": "provider",
                "phase": "completed",
                "backend": expected_reader.backend,
                "model": expected_reader.model,
                "fallback_used": False,
            }
            if record != expected or provider_seen:
                raise ValueError("reader provider trace differs from the typed capture")
            provider_seen = True
        else:
            raise ValueError("reader trace contains an unsupported event")
    if workflow != {"analyst", "reporter"} or not assertion_seen or not provider_seen:
        raise ValueError("reader trace is missing required workflow evidence")
    observed_operations = set()
    for chain in pairs.values():
        if len(chain) != 2:
            raise ValueError("reader request trace chain is incomplete")
        started, completed = chain
        common = {"schema", "run_id", "event", "phase", "request_id", "wire_operation", "transport"}
        if (
            set(started) != common
            or started.get("phase") != "started"
            or started.get("transport") != "uds"
            or started.get("wire_operation")
            not in {
                "object.get",
                "object.search",
                "memory.get",
                "memory.recall",
                "projection.status",
            }
        ):
            raise ValueError("reader trace request phase chain is invalid")
        operation = started["wire_operation"]
        observed_operations.add(operation)
        expected_completed = {**started, "phase": "completed", "typed_response_ok": True}
        evidence_ref = {
            "object.get": "seed-object-v1",
            "object.search": "seed-object-v1",
            "memory.get": "seed-memory-v1",
            "memory.recall": "seed-memory-v1",
        }.get(operation)
        if evidence_ref is not None:
            expected_completed.update({"evidence_ref": evidence_ref, "match_count": 1})
        if completed != expected_completed:
            raise ValueError("reader trace completion does not bind seeded evidence")
    if not pairs:
        raise ValueError("reader trace contains no verified read request")
    if not {"object.get", "object.search", "memory.get", "memory.recall"}.issubset(
        observed_operations
    ):
        raise ValueError("reader trace lacks required object and memory retrieval evidence")
    return len(pairs)


def _read_exact(descriptor: int, size: int) -> bytes:
    chunks = []
    remaining = size
    while remaining:
        chunk = os.read(descriptor, min(remaining, 64 * 1024))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _stable_stat(metadata: os.stat_result) -> tuple[int, ...]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        stat.S_IFMT(metadata.st_mode),
        stat.S_IMODE(metadata.st_mode),
        metadata.st_uid,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )
