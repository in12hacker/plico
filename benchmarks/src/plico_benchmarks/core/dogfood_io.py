"""Bounded same-descriptor I/O and committed evidence-directory protocol."""

from __future__ import annotations

import errno
import hashlib
import json
import os
import stat
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any

MAX_EVIDENCE_BYTES = 4 * 1024 * 1024
EVIDENCE_FILE = "evidence.json"
SIDECAR_FILE = "evidence.sha256.json"
LOCK_FILE = "LOCK"
COMMITTED_FILE = "COMMITTED"


@dataclass(frozen=True)
class ReadArtifact:
    payload: bytes
    size: int
    mode: int
    device: int
    inode: int


def read_regular(path: Path, *, private: bool, maximum: int = MAX_EVIDENCE_BYTES) -> bytes:
    return read_regular_artifact(path, private=private, maximum=maximum).payload


def read_regular_artifact(
    path: Path,
    *,
    private: bool,
    maximum: int = MAX_EVIDENCE_BYTES,
    required_mode: int | None = None,
) -> ReadArtifact:
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    noatime = getattr(os, "O_NOATIME", 0)
    parent_fd, name = _open_parent_no_follow(path)
    try:
        descriptor = os.open(name, flags | noatime, dir_fd=parent_fd)
    except OSError as error:
        if noatime and error.errno == errno.EPERM:
            descriptor = os.open(name, flags, dir_fd=parent_fd)
        else:
            os.close(parent_fd)
            raise ValueError("evidence input cannot be safely opened") from error
    os.close(parent_fd)
    try:
        before = os.fstat(descriptor)
        mode = stat.S_IMODE(before.st_mode)
        if not stat.S_ISREG(before.st_mode) or before.st_uid != os.geteuid():
            raise ValueError("evidence input has an invalid type or owner")
        expected_mode = required_mode if required_mode is not None else (0o600 if private else None)
        if expected_mode is not None and mode != expected_mode:
            raise ValueError(f"evidence input must have mode {expected_mode:04o}")
        if not private and mode & 0o002:
            raise ValueError("live source input must not be world writable")
        if before.st_size <= 0 or before.st_size > maximum:
            raise ValueError("evidence input is outside its byte limit")
        payload = _read_fd(descriptor, before.st_size)
        after = os.fstat(descriptor)
        if len(payload) != before.st_size or _stable_stat(after) != _stable_stat(before):
            raise ValueError("evidence input changed while being read")
        return ReadArtifact(
            payload=payload,
            size=before.st_size,
            mode=mode,
            device=before.st_dev,
            inode=before.st_ino,
        )
    finally:
        os.close(descriptor)


def strict_json_object(payload: bytes) -> dict[str, Any]:
    value = json.loads(
        payload,
        object_pairs_hook=_reject_duplicates,
        parse_constant=_reject_nonfinite_json_number,
    )
    if not isinstance(value, dict):
        raise ValueError("evidence JSON must be an object")
    return value


def strict_json_lines(payload: bytes) -> list[dict[str, Any]]:
    try:
        text = payload.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ValueError("trace must be UTF-8 JSONL") from error
    if not text.endswith("\n"):
        raise ValueError("trace JSONL must end with a newline")
    records = []
    for line in text.splitlines():
        if not line:
            raise ValueError("trace JSONL contains an empty record")
        records.append(strict_json_object(line.encode()))
    if not records:
        raise ValueError("trace JSONL must not be empty")
    return records


def canonical_json(value: Any) -> bytes:
    return (
        json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        + "\n"
    ).encode()


def _reject_nonfinite_json_number(value: str) -> None:
    raise ValueError(f"non-finite JSON number is forbidden: {value}")


def sha256(payload: bytes) -> str:
    return hashlib.sha256(payload).hexdigest()


def commit_evidence_directory(output: Path, evidence: bytes, sidecar: bytes) -> None:
    commit_artifact_directory(
        output,
        artifact_name=EVIDENCE_FILE,
        sidecar_name=SIDECAR_FILE,
        artifact=evidence,
        sidecar=sidecar,
        commit_schema="plico.p3a.dogfood-evidence-commit/v1",
    )


def commit_artifact_directory(
    output: Path,
    *,
    artifact_name: str,
    sidecar_name: str,
    artifact: bytes,
    sidecar: bytes,
    commit_schema: str,
) -> None:
    parent = output.parent
    parent_fd = open_directory_no_follow(parent)
    try:
        os.mkdir(output.name, 0o700, dir_fd=parent_fd)
        directory_fd = os.open(
            output.name,
            os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_fd,
        )
        try:
            _assert_directory(directory_fd)
            lock_fd = os.open(
                LOCK_FILE,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
                0o600,
                dir_fd=directory_fd,
            )
            os.fsync(lock_fd)
            os.close(lock_fd)
            _atomic_write_at(directory_fd, artifact_name, artifact)
            _atomic_write_at(directory_fd, sidecar_name, sidecar)
            committed = canonical_json(
                {
                    "schema": commit_schema,
                    "artifact_file": artifact_name,
                    "artifact_sha256": sha256(artifact),
                    "sidecar_file": sidecar_name,
                    "sidecar_sha256": sha256(sidecar),
                }
            )
            marker_fd = os.open(
                COMMITTED_FILE,
                os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
                0o600,
                dir_fd=directory_fd,
            )
            try:
                _write_all(marker_fd, committed)
                os.fsync(marker_fd)
            finally:
                os.close(marker_fd)
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)
        os.fsync(parent_fd)
    except FileExistsError as error:
        raise ValueError("evidence output directory already exists") from error
    finally:
        os.close(parent_fd)


def write_private_exclusive(path: Path, payload: bytes) -> None:
    """Create one bounded owner-only artifact without replacing existing evidence."""
    if not payload or len(payload) > MAX_EVIDENCE_BYTES:
        raise ValueError("private artifact is outside its byte limit")
    parent_fd = open_directory_no_follow(path.parent)
    try:
        descriptor = os.open(
            path.name,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
            0o600,
            dir_fd=parent_fd,
        )
        try:
            _write_all(descriptor, payload)
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)


def verify_evidence_directory(output: Path) -> tuple[bytes, bytes]:
    return verify_artifact_directory(
        output,
        artifact_name=EVIDENCE_FILE,
        sidecar_name=SIDECAR_FILE,
        commit_schema="plico.p3a.dogfood-evidence-commit/v1",
    )


def verify_artifact_directory(
    output: Path, *, artifact_name: str, sidecar_name: str, commit_schema: str
) -> tuple[bytes, bytes]:
    parent_fd = open_directory_no_follow(output.parent)
    try:
        directory_fd = os.open(
            output.name,
            os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_fd,
        )
        try:
            _assert_directory(directory_fd)
            directory_before = os.fstat(directory_fd)
            entries = {entry.name for entry in os.scandir(directory_fd)}
            if entries != {LOCK_FILE, artifact_name, sidecar_name, COMMITTED_FILE}:
                raise ValueError("evidence directory is incomplete or contains mixed-run files")
            _assert_private_entry(directory_fd, LOCK_FILE, allow_empty=True)
            artifact = _read_private_at(directory_fd, artifact_name)
            sidecar = _read_private_at(directory_fd, sidecar_name)
            committed = _read_private_at(directory_fd, COMMITTED_FILE)
            marker = strict_json_object(committed)
            expected = {
                "schema": commit_schema,
                "artifact_file": artifact_name,
                "artifact_sha256": sha256(artifact),
                "sidecar_file": sidecar_name,
                "sidecar_sha256": sha256(sidecar),
            }
            if marker != expected or committed != canonical_json(expected):
                raise ValueError("evidence COMMITTED marker does not bind the complete pair")
            if {entry.name for entry in os.scandir(directory_fd)} != entries or _stable_stat(
                os.fstat(directory_fd)
            ) != _stable_stat(directory_before):
                raise ValueError("evidence directory changed during verification")
            return artifact, sidecar
        finally:
            os.close(directory_fd)
    finally:
        os.close(parent_fd)


def _read_private_at(directory_fd: int, name: str) -> bytes:
    flags = os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, dir_fd=directory_fd)
    try:
        before = os.fstat(descriptor)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_uid != os.geteuid()
            or stat.S_IMODE(before.st_mode) != 0o600
            or before.st_size <= 0
            or before.st_size > MAX_EVIDENCE_BYTES
        ):
            raise ValueError("evidence directory entry has an invalid type, owner, mode, or size")
        payload = _read_fd(descriptor, before.st_size)
        after = os.fstat(descriptor)
        if len(payload) != before.st_size or _stable_stat(after) != _stable_stat(before):
            raise ValueError("evidence directory entry changed while being read")
        return payload
    finally:
        os.close(descriptor)


def _assert_private_entry(directory_fd: int, name: str, *, allow_empty: bool) -> None:
    descriptor = os.open(
        name,
        os.O_RDONLY | os.O_NONBLOCK | getattr(os, "O_NOFOLLOW", 0),
        dir_fd=directory_fd,
    )
    try:
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o600
            or (not allow_empty and metadata.st_size == 0)
        ):
            raise ValueError("evidence directory lock has an invalid type, owner, or mode")
    finally:
        os.close(descriptor)


def _assert_directory(directory_fd: int) -> None:
    metadata = os.fstat(directory_fd)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise ValueError("evidence output directory must be owner-only mode 0700")


def _atomic_write_at(directory_fd: int, name: str, payload: bytes) -> None:
    temporary = f".{name}.{uuid.uuid4().hex}.tmp"
    descriptor = os.open(
        temporary,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
        0o600,
        dir_fd=directory_fd,
    )
    try:
        _write_all(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)
    os.rename(temporary, name, src_dir_fd=directory_fd, dst_dir_fd=directory_fd)


def _read_fd(descriptor: int, size: int) -> bytes:
    chunks = []
    remaining = size
    while remaining:
        chunk = os.read(descriptor, min(remaining, 64 * 1024))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _write_all(descriptor: int, payload: bytes) -> None:
    view = memoryview(payload)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("evidence write made no progress")
        view = view[written:]


def _reject_duplicates(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("evidence JSON contains duplicate keys")
        value[key] = item
    return value


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


def _open_parent_no_follow(path: Path) -> tuple[int, str]:
    if not path.name or path.name in {".", ".."}:
        raise ValueError("evidence input has an invalid basename")
    return open_directory_no_follow(path.parent), path.name


def open_directory_no_follow(path: Path) -> int:
    descriptor = os.open(
        "/" if path.is_absolute() else ".",
        os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0),
    )
    try:
        for part in path.parts:
            if part in {"", "/", "."}:
                continue
            if part == "..":
                raise ValueError("parent traversal is not allowed in evidence paths")
            next_fd = os.open(
                part,
                os.O_RDONLY
                | os.O_DIRECTORY
                | getattr(os, "O_CLOEXEC", 0)
                | getattr(os, "O_NOFOLLOW", 0),
                dir_fd=descriptor,
            )
            os.close(descriptor)
            descriptor = next_fd
        return descriptor
    except Exception:
        os.close(descriptor)
        raise
