#!/usr/bin/env python3
"""Authorize a verified v53 R0 packet using only local Git objects.

The packet proves integrity but carries no authority.  Authorization is a
separate, single-parent Git commit which only adds one canonical approval
record.  A deterministic tag must point at that commit.  This is an unsigned,
procedural repository-control record; it is not a cryptographic identity proof
and does not resist a repository administrator or a compromised local UID.
"""

from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

import verify


APPROVAL_SCHEMA = "plico.v53.r0-approval/v1"
RESULT_SCHEMA = "plico.v53.r0-authorization-result/v1"
APPROVAL_PATH = "docs/milestones/v53-r0-approval.json"
DEFAULT_APPROVAL_REVISION = "refs/remotes/origin/v53-integration"
ALLOWED_APPROVAL_REFS = frozenset(
    {
        DEFAULT_APPROVAL_REVISION,
        "refs/heads/v53-integration",
    }
)
SPEC_PATH = "scripts/milestones/v53/r0_spec.json"
MAX_APPROVAL_BYTES = 64 * 1024
MAX_GIT_METADATA_BYTES = 4 * 1024 * 1024
MAX_GIT_OUTPUT_BYTES = 1024 * 1024
GIT_TIMEOUT_SECONDS = 30
MAX_CLOCK_SKEW = dt.timedelta(minutes=5)
OBJECT_ID = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
LIMITATIONS = [
    "procedural and unsigned; no cryptographic reviewer identity",
    "not resistant to repository administrator rewriting refs or history",
    "not resistant to compromise of the local verifier UID or toolchain",
]


class AuthorizationError(RuntimeError):
    """A fail-closed authorization failure safe to show to the caller."""


def _object(value: Any, location: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AuthorizationError(f"{location} must be an object")
    return value


def _string(value: Any, location: str, maximum_bytes: int = 4096) -> str:
    try:
        return verify.require_string(value, location, maximum_bytes)
    except verify.VerificationError as error:
        raise AuthorizationError(str(error)) from error


def _utc(value: Any, location: str) -> dt.datetime:
    try:
        return verify.parse_utc(value, location)
    except verify.VerificationError as error:
        raise AuthorizationError(str(error)) from error


def _exact_keys(value: dict[str, Any], expected: set[str], location: str) -> None:
    try:
        verify.require_exact_keys(value, expected, location)
    except verify.VerificationError as error:
        raise AuthorizationError(str(error)) from error


def _object_id(value: Any, location: str) -> str:
    value = _string(value, location, 64)
    if not OBJECT_ID.fullmatch(value):
        raise AuthorizationError(f"{location} is not a canonical Git object id")
    return value


def packet_integrity_status(handoff: dict[str, Any]) -> dict[str, str]:
    """Describe packet-only authority without trusting self-reported approval."""

    authorization = _object(handoff.get("authorization"), "handoff.authorization")
    if authorization.get("state") != "unverified":
        raise AuthorizationError("packet authorization must remain unverified")
    return {
        "authorization": "unverified",
        "integrity": "verified",
        "packet_id": _string(handoff.get("packet_id"), "handoff.packet_id", 64),
    }


def packet_digest_bindings(packet_files: dict[str, bytes]) -> dict[str, str]:
    """Return exact packet digests which the approval record and tag bind."""

    try:
        committed = packet_files["COMMITTED"]
        handoff = packet_files["handoff.json"]
    except KeyError as error:
        raise AuthorizationError(
            f"verified packet bytes are missing {error.args[0]}"
        ) from error
    return {
        "committed_sha256": verify.sha256_bytes(committed),
        "handoff_sha256": verify.sha256_bytes(handoff),
    }


def approval_tag_name(packet_files: dict[str, bytes]) -> str:
    return f"v53-r0-{packet_digest_bindings(packet_files)['committed_sha256']}"


def _binding_sha256(handoff: dict[str, Any], path: str) -> str:
    bindings = handoff.get("bindings")
    if not isinstance(bindings, list):
        raise AuthorizationError("handoff.bindings must be a list")
    matches = [
        item for item in bindings if isinstance(item, dict) and item.get("path") == path
    ]
    if len(matches) != 1:
        raise AuthorizationError(f"handoff does not contain one binding for {path}")
    digest = _string(matches[0].get("sha256"), f"binding sha256 for {path}", 64)
    if not verify.HEX_SHA256.fullmatch(digest):
        raise AuthorizationError(f"binding sha256 for {path} is not canonical")
    return digest


def expected_record_bindings(
    handoff: dict[str, Any], packet_files: dict[str, bytes]
) -> dict[str, Any]:
    """Derive all immutable approval fields from the verified packet."""

    spec = _object(handoff.get("spec"), "handoff.spec")
    contract = _object(spec.get("contract"), "handoff.spec.contract")
    accepted_adr = _object(spec.get("accepted_adr"), "handoff.spec.accepted_adr")
    digests = packet_digest_bindings(packet_files)
    return {
        "accepted_adr_sha256": _binding_sha256(handoff, accepted_adr.get("path")),
        "committed_sha256": digests["committed_sha256"],
        "contract_sha256": _binding_sha256(handoff, contract.get("path")),
        "contract_version": handoff.get("contract_version"),
        "expires_at_utc": handoff.get("expires_at_utc"),
        "handoff_sha256": digests["handoff_sha256"],
        "implementation_base_sha": handoff.get("implementation_base_sha"),
        "implementation_base_tree": handoff.get("implementation_base_tree"),
        "packet_generated_at_utc": handoff.get("generated_at_utc"),
        "packet_id": handoff.get("packet_id"),
        "spec_sha256": _binding_sha256(handoff, SPEC_PATH),
        "tag": approval_tag_name(packet_files),
    }


def parse_approval_record(data: bytes) -> dict[str, Any]:
    """Parse one bounded, strict canonical approval JSON blob."""

    if not data or len(data) > MAX_APPROVAL_BYTES:
        raise AuthorizationError("approval record is empty or exceeds its byte limit")
    try:
        record = _object(
            verify.strict_json_loads(data, APPROVAL_PATH),
            APPROVAL_PATH,
        )
        if verify.canonical_json(record) != data:
            raise AuthorizationError("approval record is not canonical JSON")
    except verify.VerificationError as error:
        raise AuthorizationError(str(error)) from error
    return record


def validate_approval_record(
    record: dict[str, Any],
    *,
    handoff: dict[str, Any],
    packet_files: dict[str, bytes],
    approval_commit_time: dt.datetime,
    now: dt.datetime,
) -> dict[str, Any]:
    """Validate the closed schema, review semantics, bindings, and lifetime."""

    _exact_keys(
        record,
        {
            "accepted_adr_sha256",
            "approved_at_utc",
            "attestation",
            "authority_limitations",
            "committed_sha256",
            "contract_sha256",
            "contract_version",
            "decision",
            "expires_at_utc",
            "handoff_sha256",
            "implementation_base_sha",
            "implementation_base_tree",
            "manual_reviewers",
            "packet_authorization",
            "packet_generated_at_utc",
            "packet_id",
            "review_method",
            "schema",
            "spec_sha256",
            "tag",
        },
        "approval",
    )
    if record["schema"] != APPROVAL_SCHEMA:
        raise AuthorizationError("unsupported approval schema")
    if record["decision"] != "GO":
        raise AuthorizationError("approval decision is not GO")
    if record["attestation"] != "unsigned_repository_control":
        raise AuthorizationError(
            "approval attestation is not unsigned repository control"
        )
    if record["review_method"] != "manual_review":
        raise AuthorizationError("approval review method is not manual review")
    if record["packet_authorization"] != "unverified":
        raise AuthorizationError(
            "approval falsely grants authority to the packet itself"
        )
    if record["authority_limitations"] != LIMITATIONS:
        raise AuthorizationError("approval authority limitations differ")

    reviewers = record["manual_reviewers"]
    if not isinstance(reviewers, list) or not 1 <= len(reviewers) <= 16:
        raise AuthorizationError(
            "approval must name between one and sixteen manual reviewers"
        )
    normalized = [
        _string(value, f"approval.manual_reviewers[{index}]", 128)
        for index, value in enumerate(reviewers)
    ]
    if normalized != sorted(set(normalized)):
        raise AuthorizationError("manual reviewers must be sorted and unique")

    for field, expected in expected_record_bindings(handoff, packet_files).items():
        if record[field] != expected:
            raise AuthorizationError(f"approval {field} binding differs")

    approved_at = _utc(record["approved_at_utc"], "approval.approved_at_utc")
    generated_at = _utc(
        record["packet_generated_at_utc"], "approval.packet_generated_at_utc"
    )
    expires_at = _utc(record["expires_at_utc"], "approval.expires_at_utc")
    if approval_commit_time.microsecond or approval_commit_time.tzinfo is None:
        raise AuthorizationError(
            "approval commit time is not second-precision aware time"
        )
    approval_commit_time = approval_commit_time.astimezone(dt.timezone.utc)
    if approved_at != approval_commit_time:
        raise AuthorizationError("approval time differs from Git commit time")
    if approved_at < generated_at:
        raise AuthorizationError("approval predates packet generation")
    if approved_at > now + MAX_CLOCK_SKEW:
        raise AuthorizationError("approval timestamp is unacceptably in the future")
    if now >= expires_at or approved_at >= expires_at:
        raise AuthorizationError("approval has expired or was created after expiry")
    return record


def bound_git_executable(handoff: dict[str, Any]) -> Path:
    """Resolve and re-hash the packet-sealed Git before any Git operation."""

    spec = _object(handoff.get("spec"), "handoff.spec")
    toolchain = _object(spec.get("toolchain"), "handoff.spec.toolchain")
    git_contract = _object(toolchain.get("git"), "handoff.spec.toolchain.git")
    command = git_contract.get("command")
    if not isinstance(command, list) or len(command) < 1:
        raise AuthorizationError("sealed Git command is missing")
    expected_path = Path(_string(command[0], "sealed Git command", 4096))
    observed = _object(
        _object(handoff.get("toolchain_observed"), "handoff.toolchain_observed").get(
            "git"
        ),
        "handoff.toolchain_observed.git",
    )
    sealed_realpath = Path(
        _string(observed.get("launcher_realpath"), "sealed Git realpath", 4096)
    )
    try:
        expected_realpath = expected_path.resolve(strict=True)
        realpath = sealed_realpath.resolve(strict=True)
        info = realpath.stat()
        data = realpath.read_bytes()
    except OSError as error:
        raise AuthorizationError(
            f"packet-bound Git executable identity cannot be read: {error}"
        ) from error
    if (
        not expected_path.is_absolute()
        or not sealed_realpath.is_absolute()
        or expected_realpath != realpath
        or not stat.S_ISREG(info.st_mode)
        or not os.access(realpath, os.X_OK)
    ):
        raise AuthorizationError(
            "packet-bound Git is not the expected executable regular file"
        )
    if observed.get("launcher_path") != os.fspath(expected_path):
        raise AuthorizationError("sealed Git launcher path differs from the contract")
    digest = _string(observed.get("launcher_sha256"), "sealed Git sha256", 64)
    if not verify.HEX_SHA256.fullmatch(digest) or verify.sha256_bytes(data) != digest:
        raise AuthorizationError("packet-bound Git executable digest differs")
    located = shutil.which("git")
    try:
        path_git = Path(located).resolve(strict=True) if located is not None else None
    except OSError as error:
        raise AuthorizationError("PATH Git identity cannot be resolved") from error
    if path_git != realpath:
        raise AuthorizationError("PATH Git differs from the packet-bound executable")
    return realpath


def _git_environment() -> dict[str, str]:
    return {
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_NO_LAZY_FETCH": "1",
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_PAGER": "cat",
        "GIT_TERMINAL_PROMPT": "0",
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": os.defpath,
    }


def run_git(
    repo: Path,
    args: list[str],
    *,
    git_executable: Path,
    allow_failure: bool = False,
) -> bytes:
    """Run an absolute Git executable with a small sanitized environment."""

    command = [
        os.fspath(git_executable),
        "--no-pager",
        "--no-replace-objects",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.preloadIndex=false",
        "-C",
        os.fspath(repo),
        *args,
    ]
    try:
        result = subprocess.run(
            command,
            env=_git_environment(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
            timeout=GIT_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise AuthorizationError(f"sanitized Git execution failed: {error}") from error
    if (
        len(result.stdout) > MAX_GIT_OUTPUT_BYTES
        or len(result.stderr) > MAX_GIT_OUTPUT_BYTES
    ):
        raise AuthorizationError("Git output exceeds the authorization verifier limit")
    if result.returncode != 0 and not allow_failure:
        detail = result.stderr.decode("utf-8", errors="replace").strip().splitlines()
        suffix = detail[-1] if detail else "unknown Git failure"
        raise AuthorizationError(f"Git {args[0]} failed: {suffix}")
    return result.stdout if result.returncode == 0 else b""


def _read_metadata_file(path: Path, label: str) -> bytes | None:
    """Read one bounded regular metadata file without following its final link."""

    try:
        before = path.lstat()
    except FileNotFoundError:
        return None
    except OSError as error:
        raise AuthorizationError(f"cannot inspect {label}: {error}") from error
    if not stat.S_ISREG(before.st_mode) or before.st_nlink != 1:
        raise AuthorizationError(f"{label} must be a non-linked regular file")
    if before.st_size > MAX_GIT_METADATA_BYTES:
        raise AuthorizationError(f"{label} exceeds the metadata size limit")
    try:
        flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0)
        fd = os.open(path, flags)
        try:
            data = b""
            remaining = before.st_size
            while remaining:
                chunk = os.read(fd, min(remaining, 1024 * 1024))
                if not chunk:
                    raise AuthorizationError(f"{label} changed while reading")
                data += chunk
                remaining -= len(chunk)
            if os.read(fd, 1):
                raise AuthorizationError(f"{label} grew while reading")
            after = os.fstat(fd)
        finally:
            os.close(fd)
    except OSError as error:
        raise AuthorizationError(f"cannot read {label}: {error}") from error
    if (
        before.st_dev,
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    ) != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
        raise AuthorizationError(f"{label} changed while reading")
    return data


def _metadata_directory(path: Path, label: str) -> Path:
    try:
        info = path.lstat()
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise AuthorizationError(f"cannot resolve {label}: {error}") from error
    if not stat.S_ISDIR(info.st_mode) or not resolved.is_dir():
        raise AuthorizationError(f"{label} must be a non-symlink directory")
    return resolved


def _git_directories(repo: Path) -> tuple[Path, Path, bytes]:
    marker = repo / ".git"
    try:
        marker_info = marker.lstat()
    except OSError as error:
        raise AuthorizationError(
            f"cannot inspect repository .git marker: {error}"
        ) from error
    marker_bytes = b"directory"
    if stat.S_ISDIR(marker_info.st_mode):
        git_dir = _metadata_directory(marker, "repository Git directory")
    elif stat.S_ISREG(marker_info.st_mode) and marker_info.st_nlink == 1:
        marker_data = _read_metadata_file(marker, "repository .git file")
        if marker_data is None or len(marker_data) > 4096:
            raise AuthorizationError("repository .git file is empty or oversized")
        try:
            marker_text = marker_data.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise AuthorizationError("repository .git file is not UTF-8") from error
        match = re.fullmatch(r"gitdir: ([^\x00-\x1f\x7f]+)\n?", marker_text)
        if match is None:
            raise AuthorizationError("repository .git file is not canonical")
        raw_git_dir = Path(match.group(1))
        if not raw_git_dir.is_absolute():
            raw_git_dir = repo / raw_git_dir
        git_dir = _metadata_directory(raw_git_dir, "linked Git directory")
        marker_bytes = marker_data
    else:
        raise AuthorizationError("repository .git marker has a forbidden file type")

    common_file = _read_metadata_file(git_dir / "commondir", "Git commondir")
    if common_file is None:
        common_dir = git_dir
    else:
        try:
            common_text = common_file.decode("utf-8", errors="strict").rstrip("\n")
        except UnicodeDecodeError as error:
            raise AuthorizationError("Git commondir is not UTF-8") from error
        if not common_text or "\n" in common_text or "\r" in common_text:
            raise AuthorizationError("Git commondir is not canonical")
        raw_common = Path(common_text)
        if not raw_common.is_absolute():
            raw_common = git_dir / raw_common
        common_dir = _metadata_directory(raw_common, "Git common directory")
    return git_dir, common_dir, marker_bytes


def _reject_nonempty_metadata(path: Path, label: str) -> bytes:
    data = _read_metadata_file(path, label)
    if data:
        raise AuthorizationError(f"nonempty {label} is forbidden")
    return b"" if data is None else data


def _reject_directory_entries(path: Path, label: str) -> list[str]:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return []
    except OSError as error:
        raise AuthorizationError(f"cannot inspect {label}: {error}") from error
    if not stat.S_ISDIR(info.st_mode):
        raise AuthorizationError(f"{label} must be an ordinary directory")
    try:
        entries = sorted(entry.name for entry in os.scandir(path))
    except OSError as error:
        raise AuthorizationError(f"cannot list {label}: {error}") from error
    if entries:
        raise AuthorizationError(f"nonempty {label} is forbidden")
    return entries


def git_metadata_seal(repo: Path) -> str:
    """Reject history overlays/partial stores and seal relevant Git metadata."""

    try:
        repo = Path(repo).resolve(strict=True)
    except OSError as error:
        raise AuthorizationError(
            f"repository path cannot be resolved: {error}"
        ) from error
    git_dir, common_dir, marker = _git_directories(repo)
    roots = sorted({git_dir, common_dir}, key=os.fspath)
    sealed: dict[str, str] = {"git_marker": verify.sha256_bytes(marker)}
    for index, root in enumerate(roots):
        prefix = f"root{index}"
        for relative, label in (
            ("info/grafts", "Git grafts"),
            ("shallow", "Git shallow boundary"),
            ("objects/info/alternates", "Git object alternates"),
            ("objects/info/http-alternates", "Git HTTP object alternates"),
        ):
            data = _reject_nonempty_metadata(root / relative, label)
            sealed[f"{prefix}:{relative}"] = verify.sha256_bytes(data)
        _reject_directory_entries(root / "refs/replace", "Git replacement refs")
        packed_refs = (
            _read_metadata_file(root / "packed-refs", "Git packed refs") or b""
        )
        if b"refs/replace/" in packed_refs:
            raise AuthorizationError("packed Git replacement refs are forbidden")
        sealed[f"{prefix}:packed-refs"] = verify.sha256_bytes(packed_refs)
        for config_name in ("config", "config.worktree"):
            config = (
                _read_metadata_file(root / config_name, f"Git {config_name}") or b""
            )
            lowered = config.lower()
            if b"partialclone" in lowered or b"promisor" in lowered:
                raise AuthorizationError(
                    "partial/promisor Git configuration is forbidden"
                )
            if re.search(rb"(?im)^\s*\[\s*include(?:if)?\b", config):
                raise AuthorizationError("included Git configuration is forbidden")
            sealed[f"{prefix}:{config_name}"] = verify.sha256_bytes(config)
        pack_directory = root / "objects/pack"
        try:
            pack_info = pack_directory.lstat()
        except FileNotFoundError:
            promisor_entries: list[str] = []
        except OSError as error:
            raise AuthorizationError(
                f"cannot inspect Git pack directory: {error}"
            ) from error
        else:
            if not stat.S_ISDIR(pack_info.st_mode):
                raise AuthorizationError("Git pack path must be an ordinary directory")
            try:
                promisor_entries = sorted(
                    entry.name
                    for entry in os.scandir(pack_directory)
                    if entry.name.endswith(".promisor")
                )
            except OSError as error:
                raise AuthorizationError(
                    f"cannot list Git pack directory: {error}"
                ) from error
        if promisor_entries:
            raise AuthorizationError("Git promisor pack markers are forbidden")
        sealed[f"{prefix}:promisor"] = "absent"
    return verify.sha256_bytes(verify.canonical_json(sealed))


def canonical_repo(repo: Path, *, git_executable: Path) -> Path:
    try:
        repo = Path(repo).resolve(strict=True)
    except OSError as error:
        raise AuthorizationError(
            f"repository path cannot be resolved: {error}"
        ) from error
    if not repo.is_dir():
        raise AuthorizationError("repository path is not a directory")
    root = run_git(
        repo,
        ["rev-parse", "--show-toplevel"],
        git_executable=git_executable,
    )
    try:
        reported = Path(root.decode("utf-8", errors="strict").strip()).resolve(
            strict=True
        )
    except (OSError, UnicodeDecodeError) as error:
        raise AuthorizationError("Git returned an invalid repository root") from error
    if reported != repo:
        raise AuthorizationError("--repo must name the canonical repository root")
    return repo


def _validate_revision(revision: str) -> str:
    if revision in ALLOWED_APPROVAL_REFS:
        return revision
    if OBJECT_ID.fullmatch(revision):
        return revision
    raise AuthorizationError("approval revision is not an allowed v53 ref or object id")


def resolve_commit(repo: Path, revision: str, *, git_executable: Path) -> str:
    revision = _validate_revision(revision)
    output = run_git(
        repo,
        ["rev-parse", "--verify", f"{revision}^{{commit}}"],
        git_executable=git_executable,
    )
    value = output.decode("ascii", errors="strict").strip()
    return _object_id(value, "resolved approval commit")


def _commit_parent(repo: Path, commit: str, *, git_executable: Path) -> str:
    output = run_git(
        repo,
        ["cat-file", "commit", commit],
        git_executable=git_executable,
    )
    header = output.split(b"\n\n", 1)[0]
    parents = [line[7:] for line in header.splitlines() if line.startswith(b"parent ")]
    if len(parents) != 1:
        raise AuthorizationError("approval commit must have exactly one parent")
    try:
        parent = parents[0].decode("ascii", errors="strict")
    except UnicodeDecodeError as error:
        raise AuthorizationError("approval commit parent is not ASCII") from error
    return _object_id(parent, "approval commit parent")


def _commit_time(repo: Path, commit: str, *, git_executable: Path) -> dt.datetime:
    output = run_git(
        repo,
        ["show", "-s", "--format=%ct", commit],
        git_executable=git_executable,
    )
    text = output.decode("ascii", errors="strict").strip()
    if not text.isascii() or not text.isdigit():
        raise AuthorizationError("approval commit has an invalid committer timestamp")
    try:
        return dt.datetime.fromtimestamp(int(text), tz=dt.timezone.utc)
    except (OverflowError, OSError, ValueError) as error:
        raise AuthorizationError("approval commit timestamp is out of range") from error


def _approval_blob(
    repo: Path, base: str, approval: str, *, git_executable: Path
) -> bytes:
    diff = run_git(
        repo,
        [
            "diff-tree",
            "--no-commit-id",
            "-r",
            "--name-status",
            "-z",
            "--no-renames",
            base,
            approval,
        ],
        git_executable=git_executable,
    )
    if diff != b"A\0" + APPROVAL_PATH.encode("utf-8") + b"\0":
        raise AuthorizationError(
            "approval commit must only add the fixed approval record"
        )

    base_entry = run_git(
        repo,
        ["ls-tree", "-z", base, "--", APPROVAL_PATH],
        git_executable=git_executable,
        allow_failure=True,
    )
    if base_entry:
        raise AuthorizationError("approval record already exists in the packet base")
    entry = run_git(
        repo,
        ["ls-tree", "-z", approval, "--", APPROVAL_PATH],
        git_executable=git_executable,
    )
    records = [record for record in entry.split(b"\0") if record]
    if len(records) != 1:
        raise AuthorizationError("approval record is missing or ambiguous")
    try:
        header, listed_path = records[0].split(b"\t", 1)
        mode, kind, object_id = header.decode("ascii").split(" ", 2)
        listed = listed_path.decode("utf-8", errors="strict")
    except (ValueError, UnicodeDecodeError) as error:
        raise AuthorizationError(
            "approval record has an invalid Git tree entry"
        ) from error
    if listed != APPROVAL_PATH or mode != "100644" or kind != "blob":
        raise AuthorizationError("approval record must be one regular 100644 Git blob")
    object_id = _object_id(object_id, "approval record blob")
    size = run_git(
        repo,
        ["cat-file", "-s", object_id],
        git_executable=git_executable,
    )
    try:
        byte_count = int(size.decode("ascii", errors="strict").strip())
    except (UnicodeDecodeError, ValueError) as error:
        raise AuthorizationError("approval blob has an invalid byte count") from error
    if not 0 < byte_count <= MAX_APPROVAL_BYTES:
        raise AuthorizationError("approval blob is empty or exceeds its byte limit")
    data = run_git(
        repo,
        ["cat-file", "blob", object_id],
        git_executable=git_executable,
    )
    if len(data) != byte_count:
        raise AuthorizationError("approval blob byte count changed while reading")
    return data


def validate_approval_commit(
    repo: Path,
    handoff: dict[str, Any],
    packet_files: dict[str, bytes],
    *,
    approval_revision: str = DEFAULT_APPROVAL_REVISION,
    now: dt.datetime | None = None,
) -> dict[str, Any]:
    """Validate the local approval commit and return the candidate scope base."""

    git_executable = bound_git_executable(handoff)
    metadata_before = git_metadata_seal(repo)
    repo = canonical_repo(repo, git_executable=git_executable)
    shallow_state = run_git(
        repo,
        ["rev-parse", "--is-shallow-repository"],
        git_executable=git_executable,
    )
    if shallow_state != b"false\n":
        raise AuthorizationError("Git repository must be complete and non-shallow")
    replacement_refs = run_git(
        repo,
        ["for-each-ref", "--format=%(refname)", "refs/replace"],
        git_executable=git_executable,
    )
    if replacement_refs.strip():
        raise AuthorizationError(
            "Git replacement refs are forbidden during authorization"
        )
    base = _object_id(handoff.get("implementation_base_sha"), "implementation base")
    expected_tree = _object_id(
        handoff.get("implementation_base_tree"), "implementation base tree"
    )
    observed_tree = run_git(
        repo,
        ["rev-parse", "--verify", f"{base}^{{tree}}"],
        git_executable=git_executable,
    )
    if observed_tree.decode("ascii", errors="strict").strip() != expected_tree:
        raise AuthorizationError("packet base tree differs from local Git")

    approval = resolve_commit(
        repo,
        approval_revision,
        git_executable=git_executable,
    )
    run_git(
        repo,
        ["fsck", "--strict", "--no-dangling", "--no-reflogs", base, approval],
        git_executable=git_executable,
    )
    if (
        approval == base
        or _commit_parent(repo, approval, git_executable=git_executable) != base
    ):
        raise AuthorizationError(
            "approval commit is not a direct single-parent child of packet base"
        )
    record = parse_approval_record(
        _approval_blob(repo, base, approval, git_executable=git_executable)
    )
    validate_approval_record(
        record,
        handoff=handoff,
        packet_files=packet_files,
        approval_commit_time=_commit_time(
            repo, approval, git_executable=git_executable
        ),
        now=now or dt.datetime.now(dt.timezone.utc),
    )

    tag = approval_tag_name(packet_files)
    tagged = run_git(
        repo,
        ["rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"],
        git_executable=git_executable,
    )
    if tagged.decode("ascii", errors="strict").strip() != approval:
        raise AuthorizationError(
            "derived approval tag does not point at approval commit"
        )
    run_git(
        repo,
        ["fsck", "--strict", "--no-dangling", "--no-reflogs", base, approval],
        git_executable=git_executable,
    )
    metadata_after = git_metadata_seal(repo)
    if metadata_after != metadata_before:
        raise AuthorizationError("Git metadata changed during authorization")
    return {
        "approval_commit_sha": approval,
        "authorization": "GO",
        "authorization_source": "unsigned_repository_control_manual_review",
        "candidate_scope_base_sha": approval,
        "integrity": "verified",
        "limitations": LIMITATIONS,
        "manual_reviewers": record["manual_reviewers"],
        "packet_id": handoff["packet_id"],
        "schema": RESULT_SCHEMA,
        "tag": tag,
    }


def authorize(
    artifact_dir: Path,
    repo: Path,
    *,
    approval_revision: str = DEFAULT_APPROVAL_REVISION,
    now: dt.datetime | None = None,
) -> dict[str, Any]:
    """Verify packet integrity/toolchain, then authorize its local Git commit."""

    try:
        handoff = verify.verify_handoff(
            artifact_dir,
            repo=None,
            now=now,
        )
        packet_files = verify.read_packet(artifact_dir)
    except verify.VerificationError as error:
        raise AuthorizationError(
            f"packet integrity verification failed: {error}"
        ) from error
    packet_integrity_status(handoff)
    git_executable = bound_git_executable(handoff)
    git_metadata_seal(repo)
    repo = canonical_repo(repo, git_executable=git_executable)
    try:
        verify.verify_handoff(
            artifact_dir,
            repo=repo,
            require_head=False,
            check_toolchain=True,
            now=now,
            git_executable=git_executable,
        )
    except verify.VerificationError as error:
        raise AuthorizationError(
            f"packet repository/toolchain verification failed: {error}"
        ) from error
    return validate_approval_commit(
        repo,
        handoff,
        packet_files,
        approval_revision=approval_revision,
        now=now,
    )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument(
        "--approval-commit",
        default=DEFAULT_APPROVAL_REVISION,
        help=f"approval object id or v53 ref (default: {DEFAULT_APPROVAL_REVISION})",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        result = authorize(
            args.artifact_dir,
            args.repo,
            approval_revision=args.approval_commit,
        )
    except AuthorizationError as error:
        print(f"v53 R0 authorization=NO_GO: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(verify.canonical_json(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
