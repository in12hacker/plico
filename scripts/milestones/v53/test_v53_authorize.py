#!/usr/bin/env python3
"""Adversarial local-Git tests for the v53 WP2 authorization boundary."""

from __future__ import annotations

import datetime as dt
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import authorize
import verify


NOW = dt.datetime(2026, 8, 17, 12, 0, 0, tzinfo=dt.timezone.utc)
APPROVED_AT = "2026-08-17T11:00:00Z"
PACKET_GENERATED_AT = "2026-08-17T10:00:00Z"
PACKET_EXPIRES_AT = "2026-08-18T12:00:00Z"
PACKET_ID = "wp2-r2-0123456789abcdef0123456789abcdef"
CONTRACT_PATH = "docs/milestones/v53-wp2-r2-checkpoint.md"
ADR_PATH = "docs/adr/0008-execution-observation-store-substrate-v1.md"


def git(repo: Path, *args: str, commit_time: str | None = None) -> str:
    environment = os.environ.copy()
    if commit_time is not None:
        environment["GIT_AUTHOR_DATE"] = commit_time
        environment["GIT_COMMITTER_DATE"] = commit_time
    result = subprocess.run(
        ["git", "-C", os.fspath(repo), *args],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
    )
    return result.stdout.decode("utf-8", errors="strict").strip()


def write(repo: Path, relative: str, data: bytes) -> None:
    target = repo / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)


def make_handoff(repo: Path, base: str) -> tuple[dict[str, object], dict[str, bytes]]:
    paths = (ADR_PATH, CONTRACT_PATH, authorize.SPEC_PATH)
    bindings = []
    for path in paths:
        data = git(repo, "show", f"{base}:{path}").encode("utf-8")
        bindings.append({"path": path, "sha256": verify.sha256_bytes(data)})
    git_launcher = Path(shutil.which("git") or "/usr/bin/git")
    git_realpath = git_launcher.resolve(strict=True)
    git_version = git(repo, "--version")
    handoff = {
        "authorization": {
            "approval_path": authorize.APPROVAL_PATH,
            "state": "unverified",
        },
        "bindings": bindings,
        "contract_version": "plico.milestone.v53.wp2-r2/1",
        "expires_at_utc": PACKET_EXPIRES_AT,
        "generated_at_utc": PACKET_GENERATED_AT,
        "implementation_base_sha": base,
        "implementation_base_tree": git(repo, "rev-parse", f"{base}^{{tree}}"),
        "packet_id": PACKET_ID,
        "spec": {
            "accepted_adr": {"path": ADR_PATH},
            "contract": {"path": CONTRACT_PATH},
            "toolchain": {
                "git": {
                    "command": ["git", "--version"],
                    "expected": git_version,
                    "required_lines": [],
                }
            },
        },
        "toolchain_observed": {
            "git": {
                "launcher_name": "git",
                "launcher_sha256": verify.sha256_bytes(git_realpath.read_bytes()),
                "resolved_tool": None,
                "role": "git",
                "version": git_version,
            }
        },
    }
    packet_files = {
        "COMMITTED": b'{"committed":"fixture"}\n',
        "handoff.json": b'{"handoff":"fixture"}\n',
    }
    return handoff, packet_files


def approval_record(
    handoff: dict[str, object], packet_files: dict[str, bytes]
) -> dict[str, object]:
    record = authorize.expected_record_bindings(handoff, packet_files)
    record.update(
        {
            "approved_at_utc": APPROVED_AT,
            "attestation": "unsigned_repository_control",
            "authority_limitations": authorize.LIMITATIONS,
            "decision": "GO",
            "manual_reviewers": ["Plico architecture group"],
            "packet_authorization": "unverified",
            "review_method": "manual_review",
            "schema": authorize.APPROVAL_SCHEMA,
        }
    )
    return record


def create_approval_commit(
    repo: Path,
    handoff: dict[str, object],
    packet_files: dict[str, bytes],
    *,
    mutate_record=None,
    extra_diff: bool = False,
) -> str:
    record = approval_record(handoff, packet_files)
    if mutate_record is not None:
        mutate_record(record)
    write(repo, authorize.APPROVAL_PATH, verify.canonical_json(record))
    if extra_diff:
        write(repo, "unexpected.txt", b"not allowed\n")
    git(repo, "add", "--all")
    git(repo, "commit", "-qm", "v53 WP2 manual approval", commit_time=APPROVED_AT)
    return git(repo, "rev-parse", "HEAD")


def fixture_repo(root: Path) -> tuple[Path, dict[str, object], dict[str, bytes], str]:
    repo = root / "repo"
    repo.mkdir()
    git(repo, "init", "-q")
    git(repo, "config", "user.name", "v53-test")
    git(repo, "config", "user.email", "v53-test@example.invalid")
    write(repo, ADR_PATH, b"accepted adr\n")
    write(repo, CONTRACT_PATH, b"frozen contract\n")
    write(repo, authorize.SPEC_PATH, b'{"spec":"fixture"}\n')
    git(repo, "add", "--all")
    git(repo, "commit", "-qm", "packet base", commit_time=PACKET_GENERATED_AT)
    base = git(repo, "rev-parse", "HEAD")
    handoff, packet_files = make_handoff(repo, base)
    return repo, handoff, packet_files, base


def tag_approval(repo: Path, packet_files: dict[str, bytes], commit: str) -> None:
    git(repo, "tag", authorize.approval_tag_name(packet_files), commit)


class V53AuthorizationTests(unittest.TestCase):
    def test_valid_approval_commit_is_candidate_scope_base(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            approval = create_approval_commit(repo, handoff, packet_files)
            tag_approval(repo, packet_files, approval)
            git(repo, "update-ref", authorize.DEFAULT_APPROVAL_REVISION, approval)
            result = authorize.validate_approval_commit(
                repo,
                handoff,
                packet_files,
                now=NOW,
            )
            self.assertEqual(result["authorization"], "GO")
            self.assertEqual(result["candidate_scope_base_sha"], approval)

    def test_packet_self_report_remains_unverified(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            self.assertEqual(
                authorize.packet_integrity_status(handoff)["authorization"],
                "unverified",
            )
            handoff["authorization"]["state"] = "verified"
            with self.assertRaises(authorize.AuthorizationError):
                authorize.packet_integrity_status(handoff)
            handoff["authorization"]["state"] = "unverified"
            with self.assertRaises(authorize.AuthorizationError):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    now=NOW,
                )

    def test_wrong_parent_and_extra_diff_fail_closed(self) -> None:
        for attack in ("wrong_parent", "extra_diff"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, handoff, packet_files, base = fixture_repo(Path(temporary))
                if attack == "wrong_parent":
                    write(repo, "intermediate.txt", b"changes parent\n")
                    git(repo, "add", "--all")
                    git(repo, "commit", "-qm", "intermediate", commit_time=APPROVED_AT)
                approval = create_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    extra_diff=attack == "extra_diff",
                )
                tag_approval(repo, packet_files, approval)
                self.assertNotEqual(approval, base)
                with self.assertRaises(authorize.AuthorizationError):
                    authorize.validate_approval_commit(
                        repo,
                        handoff,
                        packet_files,
                        approval_revision=approval,
                        now=NOW,
                    )

    def test_wrong_packet_or_document_digest_fails_closed(self) -> None:
        attacks = ("handoff", "committed", "contract", "spec", "adr")
        fields = {
            "handoff": "handoff_sha256",
            "committed": "committed_sha256",
            "contract": "contract_sha256",
            "spec": "spec_sha256",
            "adr": "accepted_adr_sha256",
        }
        for attack in attacks:
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
                approval = create_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    mutate_record=lambda record, field=fields[attack]: record.update(
                        {field: "f" * 64}
                    ),
                )
                tag_approval(repo, packet_files, approval)
                with self.assertRaises(authorize.AuthorizationError):
                    authorize.validate_approval_commit(
                        repo,
                        handoff,
                        packet_files,
                        approval_revision=approval,
                        now=NOW,
                    )

    def test_expired_record_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            approval = create_approval_commit(repo, handoff, packet_files)
            tag_approval(repo, packet_files, approval)
            with self.assertRaises(authorize.AuthorizationError):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    approval_revision=approval,
                    now=dt.datetime(2026, 8, 19, tzinfo=dt.timezone.utc),
                )

    def test_missing_or_fake_tag_fails_closed(self) -> None:
        for attack in ("missing", "wrong_target"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, handoff, packet_files, base = fixture_repo(Path(temporary))
                approval = create_approval_commit(repo, handoff, packet_files)
                if attack == "wrong_target":
                    tag_approval(repo, packet_files, base)
                with self.assertRaises(authorize.AuthorizationError):
                    authorize.validate_approval_commit(
                        repo,
                        handoff,
                        packet_files,
                        approval_revision=approval,
                        now=NOW,
                    )

    def test_annotated_tag_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            approval = create_approval_commit(repo, handoff, packet_files)
            git(
                repo,
                "tag",
                "-a",
                authorize.approval_tag_name(packet_files),
                "-m",
                "not a lightweight approval",
                approval,
            )
            with self.assertRaisesRegex(
                authorize.AuthorizationError, "must be lightweight"
            ):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    approval_revision=approval,
                    now=NOW,
                )

    def test_fake_ref_is_rejected_before_git_resolution(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            approval = create_approval_commit(repo, handoff, packet_files)
            tag_approval(repo, packet_files, approval)
            git(repo, "branch", "mallory", approval)
            with self.assertRaises(authorize.AuthorizationError):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    approval_revision="refs/heads/mallory",
                    now=NOW,
                )

    def test_fake_path_git_is_rejected_before_authorization_git(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, handoff, packet_files, _ = fixture_repo(root)
            approval = create_approval_commit(repo, handoff, packet_files)
            tag_approval(repo, packet_files, approval)
            fake_directory = root / "fake-bin"
            fake_directory.mkdir()
            fake_git = fake_directory / "git"
            fake_git.write_text("#!/bin/sh\nexit 99\n", encoding="utf-8")
            fake_git.chmod(0o755)
            with (
                mock.patch.dict(os.environ, {"PATH": os.fspath(fake_directory)}),
                self.assertRaisesRegex(
                    authorize.AuthorizationError, "identity differs"
                ),
            ):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    approval_revision=approval,
                    now=NOW,
                )

    def test_mutated_packet_bound_git_digest_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
            approval = create_approval_commit(repo, handoff, packet_files)
            tag_approval(repo, packet_files, approval)
            handoff["toolchain_observed"]["git"]["launcher_sha256"] = "0" * 64
            with self.assertRaisesRegex(
                authorize.AuthorizationError, "identity differs"
            ):
                authorize.validate_approval_commit(
                    repo,
                    handoff,
                    packet_files,
                    approval_revision=approval,
                    now=NOW,
                )

    def test_history_overlay_and_partial_metadata_fail_before_parent(self) -> None:
        attacks = ("graft", "replace", "shallow", "partial-config", "promisor")
        for attack in attacks:
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, handoff, packet_files, base = fixture_repo(Path(temporary))
                approval = create_approval_commit(repo, handoff, packet_files)
                tag_approval(repo, packet_files, approval)
                if attack == "graft":
                    target = repo / ".git/info/grafts"
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f"{approval} {base}\n", encoding="ascii")
                elif attack == "replace":
                    target = repo / f".git/refs/replace/{approval}"
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(f"{base}\n", encoding="ascii")
                elif attack == "shallow":
                    (repo / ".git/shallow").write_text(f"{base}\n", encoding="ascii")
                elif attack == "partial-config":
                    with (repo / ".git/config").open("ab") as handle:
                        handle.write(b"[extensions]\n\tpartialClone = origin\n")
                else:
                    target = repo / ".git/objects/pack/attack.promisor"
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(b"")
                with self.assertRaises(authorize.AuthorizationError):
                    authorize.validate_approval_commit(
                        repo,
                        handoff,
                        packet_files,
                        approval_revision=approval,
                        now=NOW,
                    )

    def test_record_is_canonical_and_limitations_are_exact(self) -> None:
        for attack in ("noncanonical", "false_identity"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, handoff, packet_files, _ = fixture_repo(Path(temporary))
                record = approval_record(handoff, packet_files)
                if attack == "noncanonical":
                    data = (str(record) + "\n").encode()
                else:
                    record["attestation"] = "cryptographically_signed_owner"
                    data = verify.canonical_json(record)
                write(repo, authorize.APPROVAL_PATH, data)
                git(repo, "add", "--all")
                git(repo, "commit", "-qm", "bad approval", commit_time=APPROVED_AT)
                approval = git(repo, "rev-parse", "HEAD")
                tag_approval(repo, packet_files, approval)
                with self.assertRaises(authorize.AuthorizationError):
                    authorize.validate_approval_commit(
                        repo,
                        handoff,
                        packet_files,
                        approval_revision=approval,
                        now=NOW,
                    )


if __name__ == "__main__":
    unittest.main()
