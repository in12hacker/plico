#!/usr/bin/env python3
"""Focused adversarial tests for the architecture-owned v53 R0 tools."""

from __future__ import annotations

import copy
import datetime as dt
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import authorize
import verify
import verify_scope

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SPEC_PATH = HERE / "r0_spec.json"
_TOOLCHAIN_OBSERVED = None


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


def frozen_spec() -> dict[str, object]:
    return verify.validate_spec(
        verify.strict_json_loads(SPEC_PATH.read_bytes(), "r0_spec.json")
    )


def toolchain_observed(spec: dict[str, object]) -> dict[str, object]:
    global _TOOLCHAIN_OBSERVED
    if _TOOLCHAIN_OBSERVED is None:
        _TOOLCHAIN_OBSERVED = verify.validate_toolchain(spec, REPO)
    return copy.deepcopy(_TOOLCHAIN_OBSERVED)


def make_repo(root: Path) -> tuple[Path, dict[str, object], str]:
    repo = root / "repo"
    repo.mkdir()
    spec = frozen_spec()
    for relative in spec["required_bindings"]:
        target = repo / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        source = REPO / relative
        if source.is_file():
            data = source.read_bytes()
        else:
            data = b"bound\n"
        if relative == spec["contract"]["path"]:
            data = data.replace(b"Draft / Architecture Review", b"Architecture-Frozen")
        target.write_bytes(data)
    memory_module = repo / "src/memory/mod.rs"
    if not memory_module.exists():
        memory_module.parent.mkdir(parents=True, exist_ok=True)
        memory_module.write_bytes((REPO / "src/memory/mod.rs").read_bytes())
    git(repo, "init", "-q")
    git(repo, "config", "user.name", "v53-test")
    git(repo, "config", "user.email", "v53-test@example.invalid")
    git(repo, "add", "--all")
    git(repo, "commit", "-qm", "frozen base")
    return repo, spec, git(repo, "rev-parse", "HEAD")


def make_packet(root: Path, repo: Path, spec: dict[str, object], base: str) -> Path:
    bindings = []
    for relative in spec["required_bindings"]:
        mode, object_id, data = verify.git_object(repo, base, relative)
        bindings.append(
            {
                "bytes": len(data),
                "git_blob": object_id,
                "mode": mode,
                "path": relative,
                "sha256": verify.sha256_bytes(data),
            }
        )
    packet_id = "r0-0123456789abcdef0123456789abcdef"
    generated = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)
    expires = generated + dt.timedelta(hours=1)
    handoff = {
        "authorization": {
            "approval_path": spec["local_gate_contract"]["approval"]["approval_path"],
            "state": "unverified",
        },
        "bindings": bindings,
        "contract_version": spec["contract_version"],
        "expires_at_utc": expires.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "generated_at_utc": generated.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "implementation_base_sha": base,
        "implementation_base_tree": git(repo, "rev-parse", f"{base}^{{tree}}"),
        "packet_id": packet_id,
        "product_baseline_sha": spec["product_baseline_sha"],
        "schema": verify.HANDOFF_SCHEMA,
        "spec": spec,
        "toolchain_observed": toolchain_observed(spec),
    }
    packet = root / "packet"
    packet.mkdir(mode=0o700)
    lock = verify.canonical_json({"packet_id": packet_id, "schema": verify.LOCK_SCHEMA})
    handoff_bytes = verify.canonical_json(handoff)
    sidecar = verify.canonical_json(
        {
            "algorithm": "sha256",
            "artifact": "handoff.json",
            "bytes": len(handoff_bytes),
            "schema": verify.DIGEST_SCHEMA,
            "sha256": verify.sha256_bytes(handoff_bytes),
        }
    )
    committed = verify.canonical_json(
        {
            "handoff_sha256": verify.sha256_bytes(handoff_bytes),
            "packet_id": packet_id,
            "schema": verify.COMMIT_SCHEMA,
            "sidecar_sha256": verify.sha256_bytes(sidecar),
        }
    )
    for name, data in {
        "LOCK": lock,
        "handoff.json": handoff_bytes,
        "handoff.sha256.json": sidecar,
        "COMMITTED": committed,
    }.items():
        (packet / name).write_bytes(data)
        (packet / name).chmod(0o600)
    return packet


def reseal(packet: Path, mutate) -> None:
    handoff = verify.strict_json_loads(
        (packet / "handoff.json").read_bytes(), "handoff"
    )
    mutate(handoff)
    handoff_bytes = verify.canonical_json(handoff)
    sidecar = verify.canonical_json(
        {
            "algorithm": "sha256",
            "artifact": "handoff.json",
            "bytes": len(handoff_bytes),
            "schema": verify.DIGEST_SCHEMA,
            "sha256": verify.sha256_bytes(handoff_bytes),
        }
    )
    committed = verify.canonical_json(
        {
            "handoff_sha256": verify.sha256_bytes(handoff_bytes),
            "packet_id": handoff["packet_id"],
            "schema": verify.COMMIT_SCHEMA,
            "sidecar_sha256": verify.sha256_bytes(sidecar),
        }
    )
    (packet / "handoff.json").write_bytes(handoff_bytes)
    (packet / "handoff.sha256.json").write_bytes(sidecar)
    (packet / "COMMITTED").write_bytes(committed)


def add_approval(repo: Path, packet: Path) -> str:
    packet_files = verify.read_packet(packet)
    handoff = verify.strict_json_loads(packet_files["handoff.json"], "handoff")
    approved = verify.parse_utc(
        handoff["generated_at_utc"], "handoff.generated_at_utc"
    ) + dt.timedelta(seconds=1)
    approved_text = approved.strftime("%Y-%m-%dT%H:%M:%SZ")
    record = authorize.expected_record_bindings(handoff, packet_files)
    record.update(
        {
            "approved_at_utc": approved_text,
            "attestation": "unsigned_repository_control",
            "authority_limitations": authorize.LIMITATIONS,
            "decision": "GO",
            "manual_reviewers": ["Plico architecture group"],
            "packet_authorization": "unverified",
            "review_method": "manual_review",
            "schema": authorize.APPROVAL_SCHEMA,
        }
    )
    approval_path = repo / authorize.APPROVAL_PATH
    approval_path.parent.mkdir(parents=True, exist_ok=True)
    approval_path.write_bytes(verify.canonical_json(record))
    git(repo, "add", "--all")
    git(repo, "commit", "-qm", "v53 R0 approval", commit_time=approved_text)
    approval = git(repo, "rev-parse", "HEAD")
    git(repo, "tag", authorize.approval_tag_name(packet_files), approval)
    git(repo, "update-ref", authorize.DEFAULT_APPROVAL_REVISION, approval)
    return approval


def approval_result(packet: Path, approval: str) -> dict[str, object]:
    handoff = verify.strict_json_loads(
        verify.read_packet(packet)["handoff.json"], "handoff"
    )
    return {
        "approval_commit_sha": approval,
        "authorization": "GO",
        "authorization_source": "unsigned_repository_control_manual_review",
        "candidate_scope_base_sha": approval,
        "integrity": "verified",
        "packet_id": handoff["packet_id"],
    }


class V53ToolTests(unittest.TestCase):
    def test_valid_packet_and_git_bindings(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, spec, base = make_repo(root)
            packet = make_packet(root, repo, spec, base)
            self.assertEqual(
                verify.verify_handoff(packet, repo=repo)["implementation_base_sha"],
                base,
            )

    def test_packet_tamper_extra_symlink_and_mode_fail_closed(self) -> None:
        attacks = ("tamper", "extra", "symlink", "mode")
        for attack in attacks:
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                repo, spec, base = make_repo(root)
                packet = make_packet(root, repo, spec, base)
                if attack == "tamper":
                    with (packet / "handoff.json").open("ab") as handle:
                        handle.write(b" ")
                elif attack == "extra":
                    (packet / "EXTRA").write_bytes(b"x")
                    (packet / "EXTRA").chmod(0o600)
                elif attack == "symlink":
                    (packet / "LOCK").unlink()
                    (packet / "LOCK").symlink_to("handoff.json")
                else:
                    (packet / "handoff.json").chmod(0o644)
                with self.assertRaises(verify.VerificationError):
                    verify.verify_handoff(packet, repo=repo)

    def test_git_binding_and_wrong_base_fail_closed(self) -> None:
        for attack in ("binding", "base"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                repo, spec, base = make_repo(root)
                packet = make_packet(root, repo, spec, base)
                if attack == "binding":
                    reseal(
                        packet,
                        lambda handoff: handoff["bindings"][0].update(sha256="0" * 64),
                    )
                else:
                    reseal(
                        packet,
                        lambda handoff: handoff.update(
                            implementation_base_sha="0" * 40
                        ),
                    )
                with self.assertRaises(verify.VerificationError):
                    verify.verify_handoff(packet, repo=repo)

    def test_packet_authorization_and_freshness_fail_closed(self) -> None:
        attacks = ("self-authorized", "future", "ttl", "expired")
        for attack in attacks:
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                repo, spec, base = make_repo(root)
                packet = make_packet(root, repo, spec, base)
                now = dt.datetime.now(dt.timezone.utc).replace(microsecond=0)

                def mutate(handoff):
                    if attack == "self-authorized":
                        handoff["authorization"]["state"] = "verified"
                    elif attack == "future":
                        generated = now + dt.timedelta(seconds=301)
                        handoff["generated_at_utc"] = generated.strftime(
                            "%Y-%m-%dT%H:%M:%SZ"
                        )
                        handoff["expires_at_utc"] = (
                            generated + dt.timedelta(seconds=60)
                        ).strftime("%Y-%m-%dT%H:%M:%SZ")
                    elif attack == "ttl":
                        handoff["generated_at_utc"] = now.strftime("%Y-%m-%dT%H:%M:%SZ")
                        handoff["expires_at_utc"] = (
                            now + dt.timedelta(seconds=1_209_601)
                        ).strftime("%Y-%m-%dT%H:%M:%SZ")
                    else:
                        handoff["generated_at_utc"] = (
                            now - dt.timedelta(hours=2)
                        ).strftime("%Y-%m-%dT%H:%M:%SZ")
                        handoff["expires_at_utc"] = (
                            now - dt.timedelta(seconds=1)
                        ).strftime("%Y-%m-%dT%H:%M:%SZ")

                reseal(packet, mutate)
                with self.assertRaises(verify.VerificationError):
                    verify.verify_handoff(packet, repo=repo, now=now)

    def test_architecture_owned_diff_and_missing_f_fail(self) -> None:
        for attack in ("architecture", "missing_f"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                repo, spec, base = make_repo(root)
                packet = make_packet(root, repo, spec, base)
                approval = add_approval(repo, packet)
                if attack == "architecture":
                    with (repo / ".gitignore").open("ab") as handle:
                        handle.write(b"architecture-change\n")
                else:
                    source = repo / "src/memory/execution_observation/mod.rs"
                    source.parent.mkdir(parents=True)
                    source.write_text(
                        "#[test]\nfn execution_observation_f10_only() {}\n",
                        encoding="utf-8",
                    )
                git(repo, "add", "--all")
                git(repo, "commit", "-qm", "candidate")
                with (
                    mock.patch(
                        "verify_scope.authorize.authorize",
                        return_value=approval_result(packet, approval),
                    ),
                    self.assertRaises(verify.VerificationError),
                ):
                    verify_scope.verify_scope(
                        packet,
                        repo,
                        approval_revision=approval,
                        candidate_revision="HEAD",
                        work_package="WP1",
                        require_clean=True,
                    )

    def test_r0_rejects_later_work_packages_and_wp1_cas_escape(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, spec, base = make_repo(root)
            packet = make_packet(root, repo, spec, base)
            for work_package in ("WP2", "WP6"):
                with (
                    self.subTest(work_package=work_package),
                    self.assertRaisesRegex(
                        verify.VerificationError, "only permits WP1"
                    ),
                ):
                    verify_scope.verify_scope(
                        packet,
                        repo,
                        approval_revision="0" * 40,
                        candidate_revision=base,
                        work_package=work_package,
                        require_clean=True,
                    )

            approval = add_approval(repo, packet)
            with (repo / "src/memory/mod.rs").open("ab") as handle:
                handle.write(b"pub(crate) mod execution_observation;\n")
            cas_escape = repo / "src/cas/ledger_store.rs"
            cas_escape.parent.mkdir(parents=True, exist_ok=True)
            cas_escape.write_bytes(b"// WP1 CAS escape\n")
            git(repo, "add", "--all")
            git(repo, "commit", "-qm", "forbidden WP1 CAS change")
            with (
                mock.patch(
                    "verify_scope.authorize.authorize",
                    return_value=approval_result(packet, approval),
                ),
                self.assertRaisesRegex(verify.VerificationError, "forbidden path"),
            ):
                verify_scope.verify_scope(
                    packet,
                    repo,
                    approval_revision=approval,
                    candidate_revision="HEAD",
                    work_package="WP1",
                    require_clean=True,
                )

    def test_crate_group_alias_and_pub_in_are_rejected(self) -> None:
        attacks = (
            b"use crate::{memory::LayeredMemory};\n",
            b"use crate as root;\nfn probe() { let _ = root::memory::x; }\n",
            b"extern crate self as escaped;\n",
            b"use super::ledger;\n",
            b"use super::layered;\n",
            b"pub(in crate::memory) fn probe() {}\n",
            b"fn probe() { crate /* nested /* x */ */ ::scheduler::run(); }\n",
            b"fn probe(v: crate /* x */ :: cas :: PersonalVaultStorage) {}\n",
            b'fn probe() { std /* x */ :: fs::read("x"); }\n',
            b'fn probe() { tokio /* x */ :: fs::read("x"); }\n',
            b"use std::{env, net, os, process, thread};\n",
            b"use std as standard; fn probe() { standard::process::exit(1); }\n",
            b'fn probe() { ::reqwest::get("x"); }\n',
            b"use redb::Database;\n",
            b"use rustix::fs;\n",
            b"use tiny_http::Server;\n",
            b"use walkdir::WalkDir;\n",
            b"use tracing::info;\n",
            b"macro_rules /* x */ ! hidden { () => {} }\n",
            b'fn probe() { include_str /* x */ ! ("safe-looking.rs"); }\n',
            b"pub /* hidden */ fn probe() {}\n",
            b'#[cfg(target_os = "linux")] fn probe() {}\n',
            b'#[path = "outside.rs"] mod injected;\n',
            b'fn probe() { option_env /* x */ ! ("RUSTFLAGS"); }\n',
        )
        for source in attacks:
            with self.subTest(source=source):
                with self.assertRaises(verify.VerificationError):
                    verify_scope._scan_observation_source(
                        "src/memory/execution_observation/attack.rs",
                        source,
                        maximum_bytes=65_536,
                        maximum_lines_exclusive=300,
                    )

    def test_rust_scanner_ignores_literals_but_not_side_doors(self) -> None:
        verify_scope._scan_observation_source(
            "src/memory/execution_observation/valid.rs",
            b'pub(crate) fn valid() { let _ = r#"crate::scheduler include_str!(x)"#; }\n',
            maximum_bytes=65_536,
            maximum_lines_exclusive=300,
        )

    def test_wp1_memory_anchor_rejects_live_hook_and_public_reexport(self) -> None:
        candidates = {
            "live-hook": (
                b"mod existing;\n"
                b"pub(crate) mod execution_observation;\n"
                b"fn initialize_observation() {}\n"
            ),
            "public-reexport": (
                b"mod existing;\n"
                b"pub(crate) mod execution_observation;\n"
                b"pub use execution_observation::*;\n"
            ),
            "public-module": b"mod existing;\npub mod execution_observation;\n",
        }
        for name, candidate_bytes in candidates.items():
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temporary:
                repo = Path(temporary) / "repo"
                repo.mkdir()
                module = repo / "src/memory/mod.rs"
                module.parent.mkdir(parents=True)
                module.write_bytes(b"mod existing;\n")
                git(repo, "init", "-q")
                git(repo, "config", "user.name", "v53-test")
                git(repo, "config", "user.email", "v53-test@example.invalid")
                git(repo, "add", "--all")
                git(repo, "commit", "-qm", "base")
                base = git(repo, "rev-parse", "HEAD")
                module.write_bytes(candidate_bytes)
                git(repo, "add", "--all")
                git(repo, "commit", "-qm", "candidate")
                candidate = git(repo, "rev-parse", "HEAD")
                with self.assertRaises(verify.VerificationError):
                    verify_scope._verify_wp1_memory_module_anchor(repo, base, candidate)

    def test_golden_and_toolchain_spec_mutations_are_rejected(self) -> None:
        for attack in ("golden", "toolchain"):
            with self.subTest(attack=attack):
                spec = copy.deepcopy(frozen_spec())
                if attack == "golden":
                    spec["golden_vectors"]["started_request"]["sha256"] = "0" * 64
                    with self.assertRaises(verify.VerificationError):
                        verify.validate_spec(spec)
                else:
                    spec["toolchain"]["rustc"]["required_lines"][1] = (
                        "commit-hash: " + "0" * 40
                    )
                    verify.validate_spec(spec)
                    with self.assertRaises(verify.VerificationError):
                        verify.validate_toolchain(spec, REPO)

    def test_exact_wire_schema_mutations_are_rejected(self) -> None:
        mutations = {
            "nullable": lambda spec: spec["wire_contract"]["nullable_fields"].remove(
                "AppendTerminalRequestV1.execution_elapsed_ms"
            ),
            "observation-shape": lambda spec: spec["wire_contract"][
                "attempt_observation_fields"
            ].remove("terminal_receipt"),
            "uuid": lambda spec: spec["wire_contract"].update(
                uuid_encoding="any-uuid-string"
            ),
            "schema": lambda spec: spec["record_schemas"].update(
                root="plico.execution-observation.arbitrary/v1"
            ),
            "hash-domain": lambda spec: spec["hash_domains"].update(
                root="plico.execution-observation.fixture.other-root.v1\0"
            ),
            "dual-slot": lambda spec: spec["state_machine"]["dual_slot"].__setitem__(
                0, "E/E=anything"
            ),
            "read-provenance": lambda spec: spec["field_provenance"][
                "replay_derived"
            ].remove("FixtureAttemptObservationV1.terminal_receipt"),
            "toolchain-source": lambda spec: spec["toolchain"]["rustc"].update(
                source="rust-toolchain.toml channel=stable"
            ),
        }
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                spec = copy.deepcopy(frozen_spec())
                mutate(spec)
                with self.assertRaises(verify.VerificationError):
                    verify.validate_spec(spec)

    def test_full_golden_chain_is_closed_and_null_explicit(self) -> None:
        spec = frozen_spec()
        vectors = spec["golden_vectors"]
        self.assertEqual(
            {vector["domain_key"] for vector in vectors.values()},
            set(spec["hash_domains"]),
        )
        started = verify.strict_json_loads(
            vectors["started_request"]["canonical_jcs_utf8"].encode(),
            "started golden",
        )
        terminal = verify.strict_json_loads(
            vectors["terminal_request"]["canonical_jcs_utf8"].encode(),
            "terminal golden",
        )
        genesis = verify.strict_json_loads(
            vectors["genesis_root"]["canonical_jcs_utf8"].encode(),
            "genesis golden",
        )
        self.assertIn("fixture_role_ref", started)
        self.assertIn("fixture_session_ref", started)
        self.assertIn("execution_elapsed_ms", terminal)
        self.assertIsNone(started["fixture_role_ref"])
        self.assertIsNone(started["fixture_session_ref"])
        self.assertIsNone(terminal["execution_elapsed_ms"])
        self.assertIsNone(genesis["previous_root_sha256"])
        self.assertIsNone(genesis["event_segment_head_sha256"])

    def test_recomputed_golden_identity_mutation_is_rejected(self) -> None:
        spec = copy.deepcopy(frozen_spec())
        vector = spec["golden_vectors"]["started_event"]
        event = verify.strict_json_loads(
            vector["canonical_jcs_utf8"].encode(), "started event"
        )
        event["root_generation"] = 9
        canonical = json.dumps(
            event,
            ensure_ascii=False,
            allow_nan=False,
            sort_keys=True,
            separators=(",", ":"),
        )
        vector["canonical_jcs_utf8"] = canonical
        vector["sha256"] = verify.sha256_bytes(
            spec["hash_domains"]["started_event"].encode() + canonical.encode()
        )
        with self.assertRaises(verify.VerificationError):
            verify.validate_spec(spec)

    def test_actual_f_test_parser_rejects_ordinary_cfg_and_ignored_evidence(
        self,
    ) -> None:
        listed = verify_scope._parse_listed_f_tests(
            "module::execution_observation_f10_real: test\n"
        )
        self.assertEqual(listed["F10"], ["module::execution_observation_f10_real"])
        invalid_outputs = (
            "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored\n",
            "test module::execution_observation_f10_real ... ignored\n"
            "test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; "
            "0 filtered out; finished in 0.00s\n",
            "test module::execution_observation_f10_real ... ok\n",
            "test module::execution_observation_f10_fake ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
            "0 filtered out; finished in 0.00s\n",
        )
        for output in invalid_outputs:
            with (
                self.subTest(output=output),
                self.assertRaises(verify.VerificationError),
            ):
                verify_scope._parse_exact_f_test_execution(
                    output, "module::execution_observation_f10_real"
                )
        verify_scope._parse_exact_f_test_execution(
            "test module::execution_observation_f10_real ... ok\n"
            "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; "
            "0 filtered out; finished in 0.00s\n",
            "module::execution_observation_f10_real",
        )

    def test_lifecycle_inventory_rejects_observation_namespace(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            vault = Path(temporary) / "vault"
            (vault / "execution-observation-fixture-ledger").mkdir(parents=True)
            with self.assertRaises(verify.VerificationError):
                verify_scope._vault_inventory(vault)

    def test_fake_cargo_and_alias_environment_are_rejected_or_cleared(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            fake = Path(temporary) / "cargo"
            fake.write_text("#!/bin/sh\necho fake\n", encoding="utf-8")
            fake.chmod(0o755)
            spec = frozen_spec()
            observed = toolchain_observed(spec)
            with mock.patch.dict(os.environ, {"PATH": temporary}, clear=False):
                with self.assertRaises(verify.VerificationError):
                    verify_scope._resolve_frozen_cargo(spec, observed)
        cargo_path = Path("/trusted/toolchain/bin/cargo")
        with mock.patch.dict(
            os.environ,
            {
                "CARGO_ALIAS_TEST": "malicious",
                "CARGO": "/tmp/fake-cargo",
                "RUSTC_WRAPPER": "/tmp/wrapper",
                "RUSTFLAGS": "-C link-arg=evil",
                "LD_PRELOAD": "/tmp/preload.so",
            },
            clear=False,
        ):
            environment = verify_scope._hardened_tool_environment(cargo_path)
        for key in (
            "CARGO_ALIAS_TEST",
            "CARGO",
            "RUSTC_WRAPPER",
            "RUSTFLAGS",
            "LD_PRELOAD",
        ):
            self.assertNotIn(key, environment)
        self.assertEqual(environment["PATH"], "/trusted/toolchain/bin:/usr/bin:/bin")

    def test_git_replace_refs_are_rejected_before_scope_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo, spec, base = make_repo(root)
            packet = make_packet(root, repo, spec, base)
            approval = add_approval(repo, packet)
            source = repo / "src/memory/execution_observation/mod.rs"
            source.parent.mkdir(parents=True)
            source.write_text(
                "#[test]\nfn execution_observation_f10_real() {}\n"
                "#[test]\nfn execution_observation_f13_real() {}\n",
                encoding="utf-8",
            )
            git(repo, "add", "--all")
            git(repo, "commit", "-qm", "candidate")
            candidate = git(repo, "rev-parse", "HEAD")
            git(repo, "replace", candidate, base)
            with (
                mock.patch(
                    "verify_scope.authorize.authorize",
                    return_value=approval_result(packet, approval),
                ),
                self.assertRaisesRegex(verify.VerificationError, "replacement refs"),
            ):
                verify_scope.verify_scope(
                    packet,
                    repo,
                    approval_revision=approval,
                    candidate_revision=candidate,
                    work_package="WP1",
                    require_clean=True,
                )

    def test_scope_rejects_git_metadata_side_channels(self) -> None:
        attacks = (
            "fsmonitor",
            "assume-unchanged",
            "skip-worktree",
            "info-exclude",
            "grafts",
            "promisor",
            "shallow",
        )
        for attack in attacks:
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, _, base = make_repo(Path(temporary))
                if attack == "fsmonitor":
                    git(repo, "config", "core.fsmonitor", "true")
                elif attack == "assume-unchanged":
                    git(repo, "update-index", "--assume-unchanged", ".gitignore")
                elif attack == "skip-worktree":
                    git(repo, "update-index", "--skip-worktree", ".gitignore")
                elif attack == "info-exclude":
                    (repo / ".git/info/exclude").write_text(
                        "src/memory/execution_observation/**\n", encoding="utf-8"
                    )
                elif attack == "grafts":
                    (repo / ".git/info/grafts").write_text(
                        f"{base}\n", encoding="ascii"
                    )
                elif attack == "promisor":
                    git(repo, "config", "remote.origin.promisor", "true")
                else:
                    (repo / ".git/shallow").write_text(f"{base}\n", encoding="ascii")
                with (
                    verify_scope._sanitized_git_environment(),
                    verify_scope._absolute_git_runner(Path("/usr/bin/git")),
                    self.assertRaises(verify.VerificationError),
                ):
                    verify_scope._audit_repository_metadata(repo)

    def test_scope_rejects_repo_cargo_and_ignored_rust_inputs(self) -> None:
        for attack in ("cargo", "ignored-rust"):
            with (
                self.subTest(attack=attack),
                tempfile.TemporaryDirectory() as temporary,
            ):
                repo, _, _ = make_repo(Path(temporary))
                if attack == "cargo":
                    (repo / ".cargo").mkdir()
                    (repo / ".cargo/config.toml").write_text(
                        "[alias]\ntest = 'run --bin attacker'\n", encoding="utf-8"
                    )
                else:
                    with (repo / ".gitignore").open("ab") as handle:
                        handle.write(b"src/ignored_attack.rs\n")
                    git(repo, "add", ".gitignore")
                    git(repo, "commit", "-qm", "freeze ignore attack")
                    (repo / "src").mkdir(exist_ok=True)
                    (repo / "src/ignored_attack.rs").write_text(
                        'compile_error!("worktree poison");\n', encoding="utf-8"
                    )
                with (
                    verify_scope._sanitized_git_environment(),
                    verify_scope._absolute_git_runner(Path("/usr/bin/git")),
                    self.assertRaises(verify.VerificationError),
                ):
                    verify_scope._audit_repository_metadata(repo)

    def test_object_materialization_ignores_poisoned_worktree(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / "repo"
            repo.mkdir()
            git(repo, "init", "-q")
            git(repo, "config", "user.name", "v53-test")
            git(repo, "config", "user.email", "v53-test@example.invalid")
            source = repo / "safe.rs"
            source.write_bytes(b"fn from_git_object() {}\n")
            git(repo, "add", "safe.rs")
            git(repo, "commit", "-qm", "safe object")
            candidate = git(repo, "rev-parse", "HEAD")
            source.write_bytes(b'compile_error!("worktree poison");\n')
            checkout = root / "checkout"
            with (
                verify_scope._sanitized_git_environment(),
                verify_scope._absolute_git_runner(Path("/usr/bin/git")),
            ):
                manifest = verify_scope._extract_git_archive(repo, candidate, checkout)
            self.assertEqual(
                (checkout / "safe.rs").read_bytes(), b"fn from_git_object() {}\n"
            )
            verify_scope._verify_materialized_tree(checkout, manifest)

    def test_scope_git_runner_has_fixed_environment_and_config(self) -> None:
        completed = subprocess.CompletedProcess(
            ["/usr/bin/git"], 0, stdout=b"", stderr=b""
        )
        with (
            mock.patch("verify_scope.subprocess.run", return_value=completed) as run,
            verify_scope._absolute_git_runner(Path("/usr/bin/git")),
        ):
            verify.run_git(Path("/tmp/repo"), ["status", "--porcelain=v1"])
        command = run.call_args.args[0]
        self.assertIn("--no-replace-objects", command)
        for setting in (
            "core.fsmonitor=false",
            "core.untrackedCache=false",
            "core.preloadIndex=false",
            "core.hooksPath=/dev/null",
        ):
            self.assertIn(setting, command)
        self.assertEqual(
            run.call_args.kwargs["env"], verify_scope._scope_git_environment()
        )

    def test_scope_detects_local_config_mutation_at_final_seal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            repo, _, candidate = make_repo(Path(temporary))
            with (
                verify_scope._sanitized_git_environment(),
                verify_scope._absolute_git_runner(Path("/usr/bin/git")),
            ):
                fingerprint = verify_scope._audit_repository_metadata(repo)
                git(repo, "config", "user.email", "mutated@example.invalid")
                with (
                    mock.patch.object(verify_scope, "_assert_cargo_unchanged"),
                    self.assertRaisesRegex(
                        verify.VerificationError, "metadata changed"
                    ),
                ):
                    verify_scope._assert_execution_seal(
                        repo,
                        candidate,
                        {"repository_metadata_fingerprint": fingerprint},
                    )

    def test_forged_or_missing_observation_lcov_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            repo = root / "repo"
            repo.mkdir()
            observation = repo / "src/memory/execution_observation/mod.rs"
            observation.parent.mkdir(parents=True)
            observation.write_text(
                "fn covered() {}\nfn missed() {}\n", encoding="utf-8"
            )
            library = repo / "src/lib.rs"
            library.write_text("fn library() {}\n", encoding="utf-8")
            git(repo, "init", "-q")
            git(repo, "config", "user.name", "v53-test")
            git(repo, "config", "user.email", "v53-test@example.invalid")
            git(repo, "add", "--all")
            git(repo, "commit", "-qm", "candidate")
            candidate = git(repo, "rev-parse", "HEAD")
            source = {
                "src/memory/execution_observation/mod.rs": observation.read_bytes()
            }
            contract = frozen_spec()["coverage_contract"]
            cases = {
                "missing-module": "SF:src/lib.rs\nDA:1,1\nend_of_record\n",
                "diluted": (
                    "SF:src/memory/execution_observation/mod.rs\nDA:1,1\nDA:2,0\nend_of_record\n"
                ),
                "outside-repo": "SF:/outside/forged.rs\nDA:1,1\nend_of_record\n",
                "nonexistent-source": "SF:src/missing.rs\nDA:1,1\nend_of_record\n",
                "out-of-range": "SF:src/lib.rs\nDA:2,1\nend_of_record\n",
            }
            for name, content in cases.items():
                with self.subTest(name=name):
                    lcov = root / f"{name}.lcov"
                    lcov.write_text(content, encoding="utf-8")
                    with self.assertRaises(verify.VerificationError):
                        verify_scope._verify_coverage(
                            lcov, repo, candidate, source, contract
                        )


if __name__ == "__main__":
    unittest.main()
