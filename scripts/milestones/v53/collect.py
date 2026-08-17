#!/usr/bin/env python3
"""Collect the architecture-owned v53 WP2 exact-four-file handoff packet."""

from __future__ import annotations

import argparse
import datetime as dt
import os
import secrets
import stat
import sys
from pathlib import Path

import verify


SPEC_PATH = "scripts/milestones/v53/wp2_spec.json"


def _ensure_external_output(repo: Path, output: Path) -> None:
    repo_real = os.path.realpath(repo)
    parent_real = os.path.realpath(output.parent)
    output_real = os.path.join(parent_real, output.name)
    try:
        common = os.path.commonpath((repo_real, output_real))
    except ValueError as error:
        raise verify.VerificationError("repo/output path comparison failed") from error
    if common == repo_real:
        raise verify.VerificationError(
            "WP2 packet must be outside the source repository"
        )
    if os.path.lexists(output):
        raise verify.VerificationError("output directory already exists")


def _create_output_directory(path: Path) -> tuple[int, int]:
    parent_fd = verify._open_dir_no_symlinks(path.parent)
    try:
        os.mkdir(path.name, 0o700, dir_fd=parent_fd)
        directory_fd = os.open(
            path.name,
            os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0),
            dir_fd=parent_fd,
        )
        os.fchmod(directory_fd, 0o700)
        info = os.fstat(directory_fd)
        if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise verify.VerificationError(
                "cannot establish owner-only output directory"
            )
        return parent_fd, directory_fd
    except Exception:
        os.close(parent_fd)
        raise


def _write_new(directory_fd: int, name: str, data: bytes) -> None:
    if name not in verify.PACKET_FILES:
        raise verify.VerificationError(f"refusing non-packet output name: {name}")
    flags = (
        os.O_WRONLY
        | os.O_CREAT
        | os.O_EXCL
        | os.O_CLOEXEC
        | getattr(os, "O_NOFOLLOW", 0)
    )
    try:
        fd = os.open(name, flags, 0o600, dir_fd=directory_fd)
    except OSError as error:
        raise verify.VerificationError(
            f"cannot create packet entry {name}: {error}"
        ) from error
    try:
        os.fchmod(fd, 0o600)
        offset = 0
        while offset < len(data):
            written = os.write(fd, data[offset:])
            if written <= 0:
                raise verify.VerificationError(f"short write for packet entry {name}")
            offset += written
        os.fsync(fd)
    finally:
        os.close(fd)


def collect(
    repo: Path,
    output_dir: Path,
    *,
    ttl_seconds: int,
) -> dict[str, object]:
    repo = Path(repo)
    output_dir = Path(output_dir)
    _ensure_external_output(repo, output_dir)
    verify.git_status_clean(repo)
    base = verify.resolve_commit(repo, "HEAD")
    tree = verify.run_git(repo, ["rev-parse", f"{base}^{{tree}}"])
    tree_id = tree.decode("ascii", errors="strict").strip()
    verify._require_sha(tree_id, "implementation-base tree", verify.GIT_OBJECT_ID)

    _, _, spec_bytes = verify.git_object(repo, base, SPEC_PATH)
    spec = verify.validate_spec(verify.strict_json_loads(spec_bytes, SPEC_PATH))

    objects: dict[str, bytes] = {}
    bindings: list[dict[str, object]] = []
    for path in spec["required_bindings"]:
        mode, object_id, data = verify.git_object(repo, base, path)
        objects[path] = data
        bindings.append(
            {
                "bytes": len(data),
                "git_blob": object_id,
                "mode": mode,
                "path": path,
                "sha256": verify.sha256_bytes(data),
            }
        )
    verify.validate_bound_documents(spec, objects)
    observed = verify.validate_toolchain(spec, repo)

    generated = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
    freshness = spec["local_gate_contract"]["freshness"]
    if (
        not isinstance(ttl_seconds, int)
        or isinstance(ttl_seconds, bool)
        or not 1 <= ttl_seconds <= freshness["maximum_ttl_seconds"]
    ):
        raise verify.VerificationError(
            f"--ttl-seconds must be in 1..{freshness['maximum_ttl_seconds']}"
        )
    expires = generated + dt.timedelta(seconds=ttl_seconds)

    packet_id = f"wp2-r2-{secrets.token_hex(16)}"
    handoff: dict[str, object] = {
        "authorization": {
            "approval_path": spec["local_gate_contract"]["approval"]["approval_path"],
            "state": "unverified",
        },
        "bindings": bindings,
        "contract_version": spec["contract_version"],
        "expires_at_utc": expires.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "generated_at_utc": generated.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "implementation_base_sha": base,
        "implementation_base_tree": tree_id,
        "packet_id": packet_id,
        "product_baseline_sha": spec["product_baseline_sha"],
        "schema": verify.HANDOFF_SCHEMA,
        "spec": spec,
        "toolchain_observed": observed,
    }
    verify.validate_handoff(handoff, now=generated)

    lock_bytes = verify.canonical_json(
        {"packet_id": packet_id, "schema": verify.LOCK_SCHEMA}
    )
    handoff_bytes = verify.canonical_json(handoff)
    sidecar_bytes = verify.canonical_json(
        {
            "algorithm": "sha256",
            "artifact": "handoff.json",
            "bytes": len(handoff_bytes),
            "schema": verify.DIGEST_SCHEMA,
            "sha256": verify.sha256_bytes(handoff_bytes),
        }
    )
    committed_bytes = verify.canonical_json(
        {
            "handoff_sha256": verify.sha256_bytes(handoff_bytes),
            "packet_id": packet_id,
            "schema": verify.COMMIT_SCHEMA,
            "sidecar_sha256": verify.sha256_bytes(sidecar_bytes),
        }
    )

    parent_fd, directory_fd = _create_output_directory(output_dir)
    try:
        _write_new(directory_fd, "LOCK", lock_bytes)
        _write_new(directory_fd, "handoff.json", handoff_bytes)
        _write_new(directory_fd, "handoff.sha256.json", sidecar_bytes)
        os.fsync(directory_fd)
        _write_new(directory_fd, "COMMITTED", committed_bytes)
        os.fsync(directory_fd)
        os.fsync(parent_fd)
    finally:
        os.close(directory_fd)
        os.close(parent_fd)

    verify.git_status_clean(repo)
    verified = verify.verify_handoff(output_dir, repo=repo, require_head=True)
    return verified


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument(
        "--ttl-seconds",
        type=int,
        default=1_209_600,
        help="packet lifetime in seconds (default/max: 1209600; fourteen days)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        handoff = collect(
            args.repo,
            args.output_dir,
            ttl_seconds=args.ttl_seconds,
        )
    except verify.VerificationError as error:
        print(f"v53 WP2 collection failed: {error}", file=sys.stderr)
        return 1
    print(
        f"v53 WP2-R2 collected: packet={handoff['packet_id']} "
        f"implementation_base={handoff['implementation_base_sha']} "
        "integrity=verified authorization=unverified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
