#!/usr/bin/env python3
"""Verify a v53 candidate against the base sealed in a verified R0 packet."""

from __future__ import annotations

import argparse
import collections
import fnmatch
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from contextlib import contextmanager
from decimal import Decimal
from pathlib import Path

import verify
import authorize


TEST_DECLARATION = re.compile(
    r"(?m)^\s*(?:async\s+)?fn\s+(execution_observation_f(\d{2})_[A-Za-z0-9_]+)\s*\("
)
PLAIN_PUBLIC = re.compile(
    r"(?m)^\s*pub\s+(?:async\s+)?(?:const|enum|fn|mod|static|struct|trait|type|union|use)\b"
)
PUBLIC_IN = re.compile(r"(?m)^\s*pub\s*\(\s*in\b")
CRATE_IMPORT_BYPASS = re.compile(
    r"(?m)^\s*(?:pub\([^)]*\)\s+)?use\s+crate\s*(?:;|as\b|::\s*\{)"
)
SIDE_DOOR = re.compile(
    r"(?m)(?:macro_rules!|#\s*\[\s*macro_export\s*\]|cfg!\s*\(\s*feature|"
    r"#\s*\[\s*cfg\s*\(\s*feature|include!\s*\(|env!\s*\()"
)
OBSERVATION_DENY_PATTERNS = {
    "unsafe code": re.compile(r"\bunsafe\b"),
    "direct filesystem I/O": re.compile(
        r"(?:\bstd::fs\b|\btokio::fs\b|\bOpenOptions\b|\bPathBuf\b|\bFile::)"
    ),
    "canonical Memory ledger/model": re.compile(
        r"(?:crate::memory::(?:ledger|model|current_view|projection)|"
        r"\bLayeredMemory\b|\bMemoryEntry\b|\bMemoryId\b|\bRevisionId\b)"
    ),
    "runtime or public layer": re.compile(
        r"crate::(?:kernel|scheduler|intent|tool|api|client|mcp|bin)(?:::|\b)"
    ),
    "derived index or model provider": re.compile(
        r"crate::(?:fs|llm)(?:::|\b)|\bSemanticFS\b|\bEventBus\b|"
        r"TrajectoryTracker|ExperienceMiner|SkillForge|AgentKeyStore"
    ),
    "configuration side door": re.compile(r"(?:std::env|crate::config|option_env!)"),
    "benchmark dependency": re.compile(r"(?:crate::benchmarks|benchmarks::)"),
}
ALLOWED_IMPORT_ROOTS = {
    "core",
    "crate",
    "self",
    "serde",
    "serde_json",
    "serde_json_canonicalizer",
    "sha2",
    "std",
    "thiserror",
    "uuid",
}
USE_ROOT = re.compile(
    r"(?m)^\s*(?:pub\([^)]*\)\s+)?use\s+(?:::)?([A-Za-z_][A-Za-z0-9_]*)"
)
CRATE_MODULE = re.compile(r"\bcrate::([A-Za-z_][A-Za-z0-9_]*)")
CAS_DIRECT = re.compile(r"\bcrate::cas::([A-Za-z_][A-Za-z0-9_]*)")
CAS_GROUP = re.compile(r"(?s)\buse\s+crate::cas::\{([^}]{}]*)\}\s*;")
LISTED_F_TEST = re.compile(
    r"(?m)^(?P<name>\S*execution_observation_f(?P<id>\d{2})_\S*): test$"
)
UUID_TEXT = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)
SHA_TEXT = re.compile(r"^[0-9a-f]{64}$")
GIT_ENV_EXACT = {
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_REPLACE_REF_BASE",
    "GIT_WORK_TREE",
}
TOOLCHAIN_ENV_EXACT = {
    "CARGO",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO_TARGET_DIR",
    "LD_LIBRARY_PATH",
    "LD_PRELOAD",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOC",
    "RUSTDOCFLAGS",
    "RUSTFLAGS",
    "RUSTUP_TOOLCHAIN",
}


def _rust_tokens(text: str) -> list[str]:
    """Return identifiers/punctuation while discarding Rust comments and literals.

    This is deliberately a small lexical scanner, not a Rust parser.  It handles
    nested block comments and every Rust string spelling needed to prevent
    whitespace/comment/literal tricks from bypassing the scope deny rules.
    """

    tokens: list[str] = []
    index = 0
    length = len(text)
    while index < length:
        character = text[index]
        if character.isspace():
            index += 1
            continue
        if text.startswith("//", index):
            newline = text.find("\n", index + 2)
            index = length if newline < 0 else newline + 1
            continue
        if text.startswith("/*", index):
            depth = 1
            index += 2
            while index < length and depth:
                if text.startswith("/*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
            if depth:
                raise verify.VerificationError("unterminated Rust block comment")
            continue

        raw_start = index
        if character == "b" and index + 1 < length and text[index + 1] == "r":
            raw_start = index + 1
        if text[raw_start] == "r":
            cursor = raw_start + 1
            while cursor < length and text[cursor] == "#":
                cursor += 1
            if cursor < length and text[cursor] == '"':
                hashes = cursor - raw_start - 1
                terminator = '"' + ("#" * hashes)
                end = text.find(terminator, cursor + 1)
                if end < 0:
                    raise verify.VerificationError("unterminated Rust raw string")
                index = end + len(terminator)
                continue

        if character == "b" and index + 1 < length and text[index + 1] in {'"', "'"}:
            index += 1
            character = text[index]
        if character == '"':
            index += 1
            escaped = False
            while index < length:
                current = text[index]
                index += 1
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    break
            else:
                raise verify.VerificationError("unterminated Rust string")
            continue
        if character == "'":
            # A lifetime is lexed as apostrophe + identifier.  A character
            # literal has a closing apostrophe before whitespace/punctuation.
            cursor = index + 1
            escaped = False
            while cursor < length and cursor - index <= 12:
                current = text[cursor]
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == "'":
                    index = cursor + 1
                    break
                elif current.isspace():
                    break
                cursor += 1
            else:
                cursor = length
            if index > cursor:
                continue
            tokens.append("'")
            index += 1
            continue
        if character.isalpha() or character == "_":
            cursor = index + 1
            while cursor < length and (text[cursor].isalnum() or text[cursor] == "_"):
                cursor += 1
            tokens.append(text[index:cursor])
            index = cursor
            continue
        if text.startswith("::", index):
            tokens.append("::")
            index += 2
            continue
        tokens.append(character)
        index += 1
    return tokens


def _matching_token(tokens: list[str], start: int, opening: str, closing: str) -> int:
    depth = 0
    for index in range(start, len(tokens)):
        if tokens[index] == opening:
            depth += 1
        elif tokens[index] == closing:
            depth -= 1
            if depth == 0:
                return index
    raise verify.VerificationError(f"unterminated Rust token group: {opening}")


def _scan_rust_tokens(path: str, text: str, *, observation: bool) -> list[str]:
    tokens = _rust_tokens(text)
    forbidden_macros = {
        "cfg",
        "env",
        "include",
        "include_bytes",
        "include_str",
        "macro_rules",
        "option_env",
    }
    forbidden_names = {
        "AgentKeyStore",
        "EventBus",
        "ExperienceMiner",
        "File",
        "LayeredMemory",
        "MemoryEntry",
        "MemoryId",
        "OpenOptions",
        "PathBuf",
        "RevisionId",
        "SemanticFS",
        "SkillForge",
        "TrajectoryTracker",
    }
    forbidden_std_modules = {"env", "fs", "io", "net", "os", "process", "thread"}
    allowed_path_roots = ALLOWED_IMPORT_ROOTS | {
        "canonical",
        "error",
        "hash",
        "ids",
        "model",
        "tests",
        "validation",
    }
    for index, token in enumerate(tokens):
        if (
            token in forbidden_macros
            and index + 1 < len(tokens)
            and tokens[index + 1] == "!"
        ):
            raise verify.VerificationError(
                f"macro/environment side door is forbidden: {path}"
            )
        if token == "#" and index + 1 < len(tokens) and tokens[index + 1] == "[":
            end = _matching_token(tokens, index + 1, "[", "]")
            attribute = tokens[index + 2 : end]
            if attribute and attribute[0] in {
                "cfg",
                "cfg_attr",
                "path",
                "macro_export",
            }:
                if attribute != ["cfg", "(", "test", ")"]:
                    raise verify.VerificationError(
                        f"conditional/path/macro attribute is forbidden: {path}"
                    )
        if not observation:
            continue
        if (
            re.fullmatch(r"[a-z_][A-Za-z0-9_]*", token)
            and index + 1 < len(tokens)
            and tokens[index + 1] == "::"
            and (
                index == 0
                or tokens[index - 1] != "::"
                or index == 1
                or not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", tokens[index - 2])
            )
            and token not in allowed_path_roots
        ):
            raise verify.VerificationError(
                f"unapproved fully-qualified path root {token!r}: {path}"
            )
        if (
            token == "extern"
            and index + 1 < len(tokens)
            and tokens[index + 1] == "crate"
        ):
            raise verify.VerificationError(
                f"extern crate is forbidden in observation source: {path}"
            )
        if token == "unsafe":
            raise verify.VerificationError(f"unsafe code is forbidden: {path}")
        if token in forbidden_names:
            raise verify.VerificationError(
                f"forbidden dependency name {token!r}: {path}"
            )
        if token == "pub":
            if index + 1 >= len(tokens) or tokens[index + 1] != "(":
                raise verify.VerificationError(
                    f"plain public export is forbidden in observation module: {path}"
                )
            end = _matching_token(tokens, index + 1, "(", ")")
            if tokens[index + 2 : end] not in (["crate"], ["super"]):
                raise verify.VerificationError(
                    f"only pub(crate)/pub(super) visibility is allowed: {path}"
                )
        if token == "super" and tokens[index : index + 2] == ["super", "::"]:
            raise verify.VerificationError(
                f"super paths are forbidden by the WP1 module boundary: {path}"
            )
        if token in {"crate", "std", "tokio"} and index + 2 < len(tokens):
            if tokens[index + 1] != "::":
                continue
            module = tokens[index + 2]
            if token == "crate":
                raise verify.VerificationError(
                    f"crate dependencies are forbidden by the WP1 pure-type boundary: {path}"
                )
            if token == "tokio" or (token == "std" and module in forbidden_std_modules):
                raise verify.VerificationError(f"runtime/I/O path is forbidden: {path}")
        if token == "use":
            try:
                statement_end = tokens.index(";", index + 1)
            except ValueError as error:
                raise verify.VerificationError(
                    f"unterminated Rust use item: {path}"
                ) from error
            statement = tokens[index + 1 : statement_end]
            if "as" in statement:
                raise verify.VerificationError(
                    f"aliased Rust use item is forbidden: {path}"
                )
            cursor = index + 1
            if cursor < len(tokens) and tokens[cursor] == "::":
                cursor += 1
            if cursor >= len(tokens) or not re.fullmatch(
                r"[A-Za-z_][A-Za-z0-9_]*", tokens[cursor]
            ):
                raise verify.VerificationError(f"unparseable Rust use item: {path}")
            root = tokens[cursor]
            if root not in ALLOWED_IMPORT_ROOTS:
                raise verify.VerificationError(
                    f"unapproved import root {root!r} in observation module: {path}"
                )
            if root in {"crate", "self"}:
                if cursor + 1 >= len(tokens) or tokens[cursor + 1] != "::":
                    raise verify.VerificationError(
                        f"bare local-root import is forbidden: {path}"
                    )
                if cursor + 2 >= len(tokens) or tokens[cursor + 2] in {"{"}:
                    raise verify.VerificationError(
                        f"local-root group import is forbidden: {path}"
                    )
            if root == "std":
                if cursor + 2 < len(tokens) and tokens[cursor + 2] == "{":
                    raise verify.VerificationError(
                        f"std root-group import is forbidden: {path}"
                    )
                if forbidden_std_modules.intersection(statement):
                    raise verify.VerificationError(
                        f"std I/O/runtime import is forbidden: {path}"
                    )

    return tokens


def _verify_wp1_memory_module_anchor(repo: Path, base: str, candidate: str) -> None:
    """Require the sole memory module change to be one crate-private declaration."""

    module_path = "src/memory/mod.rs"
    base_mode, _, base_bytes = verify.git_object(repo, base, module_path)
    candidate_mode, _, candidate_bytes = verify.git_object(repo, candidate, module_path)
    if base_mode != "100644" or candidate_mode != "100644":
        raise verify.VerificationError(
            "WP1 memory module anchor must remain a regular 100644 Git blob"
        )
    anchor = b"pub(crate) mod execution_observation;\n"
    if anchor in base_bytes:
        raise verify.VerificationError(
            "WP1 memory module anchor already exists in the approved scope base"
        )
    candidate_lines = candidate_bytes.splitlines(keepends=True)
    if candidate_lines.count(anchor) != 1:
        raise verify.VerificationError(
            "WP1 must add exactly one crate-private execution_observation module anchor"
        )
    without_anchor = b"".join(line for line in candidate_lines if line != anchor)
    if without_anchor != base_bytes:
        raise verify.VerificationError(
            "WP1 src/memory/mod.rs changed beyond the exact crate-private module anchor"
        )


@contextmanager
def _sanitized_git_environment():
    affected = {
        key: value for key, value in os.environ.items() if key.startswith("GIT_")
    }
    for key in list(os.environ):
        if key.startswith("GIT_"):
            os.environ.pop(key, None)
    os.environ.update({"GIT_NO_LAZY_FETCH": "1", "GIT_NO_REPLACE_OBJECTS": "1"})
    try:
        yield
    finally:
        for key in list(os.environ):
            if key.startswith("GIT_"):
                os.environ.pop(key, None)
        os.environ.update(affected)


def _scope_git_environment() -> dict[str, str]:
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


@contextmanager
def _absolute_git_runner(git_path: Path):
    """Make every verifier Git call use the packet-bound absolute executable."""

    original = verify.run_git

    def run_git_absolute(
        repo: Path,
        args: list[str],
        *,
        input_bytes: bytes | None = None,
        git_executable: Path | None = None,
    ) -> bytes:
        if git_executable is not None:
            try:
                requested = Path(git_executable).resolve(strict=True)
                frozen = git_path.resolve(strict=True)
            except OSError as error:
                raise verify.VerificationError(
                    f"Git executable identity cannot be resolved: {error}"
                ) from error
            if requested != frozen:
                raise verify.VerificationError(
                    "Git executable differs from the packet-frozen scope tool"
                )
        try:
            result = subprocess.run(
                [
                    os.fspath(git_path),
                    "--no-pager",
                    "--no-replace-objects",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.untrackedCache=false",
                    "-c",
                    "core.preloadIndex=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-C",
                    os.fspath(repo),
                    *args,
                ],
                env=_scope_git_environment(),
                input=input_bytes,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
                timeout=120,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise verify.VerificationError(
                f"cannot execute frozen git: {error}"
            ) from error
        if result.returncode != 0:
            detail = (
                result.stderr.decode("utf-8", errors="replace").strip().splitlines()
            )
            suffix = detail[-1] if detail else "unknown git failure"
            raise verify.VerificationError(f"git {' '.join(args[:2])} failed: {suffix}")
        return result.stdout

    verify.run_git = run_git_absolute
    try:
        yield
    finally:
        verify.run_git = original


def _hardened_tool_environment(cargo_path: Path) -> dict[str, str]:
    environment = os.environ.copy()
    for key in list(environment):
        if (
            key in TOOLCHAIN_ENV_EXACT
            or key.startswith("CARGO_ALIAS_")
            or key.startswith("CARGO_BUILD_")
            or key.startswith("DYLD_")
        ):
            environment.pop(key, None)
    environment["PATH"] = os.pathsep.join(
        (os.fspath(cargo_path.parent), "/usr/bin", "/bin")
    )
    environment.update({"EMBEDDING_BACKEND": "stub", "LLM_BACKEND": "stub"})
    return environment


@contextmanager
def _isolated_candidate_environment(base: dict[str, str]):
    """Use an alias-free Cargo home and private build/temp directories."""

    with tempfile.TemporaryDirectory(prefix="plico-v53-execution-") as temporary:
        root = Path(temporary)
        home = root / "home"
        cargo_home = home / ".cargo"
        temp = root / "tmp"
        target = root / "target"
        for directory in (home, cargo_home, temp, target):
            directory.mkdir(mode=0o700, exist_ok=True)
        original_cargo_home = Path(
            os.environ.get("CARGO_HOME", os.fspath(Path.home() / ".cargo"))
        )
        for cache_name in ("git", "registry"):
            cache = original_cargo_home / cache_name
            if cache.is_dir():
                (cargo_home / cache_name).symlink_to(cache, target_is_directory=True)
        environment = base.copy()
        environment.update(
            {
                "CARGO_HOME": os.fspath(cargo_home),
                "CARGO_NET_OFFLINE": "true",
                "CARGO_TARGET_DIR": os.fspath(target),
                "HOME": os.fspath(home),
                "PYTHONDONTWRITEBYTECODE": "1",
                "TMPDIR": os.fspath(temp),
            }
        )
        yield environment, root


def _resolve_frozen_cargo(
    spec: dict[str, object], observed: dict[str, object]
) -> dict[str, object]:
    located = shutil.which("cargo")
    if located is None:
        raise verify.VerificationError("cargo is absent from PATH")
    cargo_path = Path(located).absolute()
    realpath = cargo_path.resolve(strict=True)
    info = realpath.stat()
    if (
        not stat.S_ISREG(info.st_mode)
        or info.st_uid != os.geteuid()
        or info.st_mode & 0o022
    ):
        raise verify.VerificationError(
            "cargo target must be current-owner, regular, and non-writable by group/other"
        )
    cargo_bytes = realpath.read_bytes()
    cargo_digest = verify.sha256_bytes(cargo_bytes)
    sealed_cargo = observed["cargo"]
    if cargo_digest != sealed_cargo["launcher_sha256"]:
        raise verify.VerificationError("cargo launcher content differs from R0 packet")
    environment = _hardened_tool_environment(cargo_path)
    for name in ("cargo", "cargo_llvm_cov", "rustc", "git"):
        current = verify._observe_tool(
            name,
            spec["toolchain"][name],
            None,
            environment=environment,
        )
        if current != observed[name]:
            raise verify.VerificationError(
                f"frozen logical version/content identity mismatch: {name}"
            )
    rustup = verify._tool_launcher("rustup", None)
    resolved = subprocess.run(
        [os.fspath(rustup), "which", "cargo", "--toolchain", "1.95.0"],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
        timeout=30,
    )
    resolved_path = Path(resolved.stdout.decode("utf-8", errors="replace").strip())
    if resolved.returncode != 0 or not resolved_path.is_absolute():
        raise verify.VerificationError("resolved cargo 1.95.0 lookup failed")
    resolved_realpath = resolved_path.resolve(strict=True)
    sealed_resolved = sealed_cargo["resolved_tool"]
    if verify.sha256_bytes(resolved_realpath.read_bytes()) != sealed_resolved["sha256"]:
        raise verify.VerificationError(
            "resolved cargo 1.95.0 identity differs from R0 packet"
        )
    if verify.sha256_bytes(realpath.read_bytes()) != cargo_digest:
        raise verify.VerificationError(
            "cargo executable changed during identity verification"
        )
    cov_located = shutil.which("cargo-llvm-cov", path=environment["PATH"])
    if cov_located is None:
        raise verify.VerificationError("cargo-llvm-cov executable is absent")
    cov_path = Path(cov_located).absolute()
    cov_realpath = cov_path.resolve(strict=True)
    cov_digest = verify.sha256_bytes(cov_realpath.read_bytes())
    sealed_cov = observed["cargo_llvm_cov"]["resolved_tool"]
    if cov_digest != sealed_cov["sha256"]:
        raise verify.VerificationError(
            "resolved cargo-llvm-cov identity differs from R0 packet"
        )
    git_path = verify._tool_launcher(spec["toolchain"]["git"]["command"][0], None)
    git_realpath = git_path.resolve(strict=True)
    git_digest = verify.sha256_bytes(git_realpath.read_bytes())
    return {
        "cargo_path": cargo_path,
        "cargo_realpath": realpath,
        "cargo_sha256": cargo_digest,
        "environment": environment,
        "resolved_cargo_path": resolved_path,
        "resolved_cargo_realpath": resolved_realpath,
        "resolved_cargo_sha256": sealed_resolved["sha256"],
        "cargo_llvm_cov_path": cov_path,
        "cargo_llvm_cov_realpath": cov_realpath,
        "cargo_llvm_cov_sha256": cov_digest,
        "git_path": git_path,
        "git_realpath": git_realpath,
        "git_sha256": git_digest,
    }


def _assert_cargo_unchanged(toolchain: dict[str, object]) -> None:
    cargo_path = toolchain["cargo_path"]
    try:
        realpath = cargo_path.resolve(strict=True)
        digest = verify.sha256_bytes(realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"cargo identity cannot be re-read: {error}"
        ) from error
    if realpath != toolchain["cargo_realpath"] or digest != toolchain["cargo_sha256"]:
        raise verify.VerificationError(
            "cargo realpath/digest changed after it was frozen"
        )
    try:
        resolved_realpath = toolchain["resolved_cargo_path"].resolve(strict=True)
        resolved_digest = verify.sha256_bytes(resolved_realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"resolved cargo identity cannot be re-read: {error}"
        ) from error
    if (
        resolved_realpath != toolchain["resolved_cargo_realpath"]
        or resolved_digest != toolchain["resolved_cargo_sha256"]
    ):
        raise verify.VerificationError(
            "resolved cargo 1.95.0 digest changed after it was frozen"
        )
    try:
        cov_realpath = toolchain["cargo_llvm_cov_path"].resolve(strict=True)
        cov_digest = verify.sha256_bytes(cov_realpath.read_bytes())
    except OSError as error:
        raise verify.VerificationError(
            f"cargo-llvm-cov identity cannot be re-read: {error}"
        ) from error
    if (
        cov_realpath != toolchain["cargo_llvm_cov_realpath"]
        or cov_digest != toolchain["cargo_llvm_cov_sha256"]
    ):
        raise verify.VerificationError(
            "cargo-llvm-cov realpath/digest changed after it was frozen"
        )
    for name in ("git",):
        try:
            current_realpath = toolchain[f"{name}_path"].resolve(strict=True)
            current_digest = verify.sha256_bytes(current_realpath.read_bytes())
        except OSError as error:
            raise verify.VerificationError(
                f"{name} identity cannot be re-read: {error}"
            ) from error
        if (
            current_realpath != toolchain[f"{name}_realpath"]
            or current_digest != toolchain[f"{name}_sha256"]
        ):
            raise verify.VerificationError(
                f"{name} realpath/digest changed after it was frozen"
            )


def _run_bounded_process(
    command: list[str],
    *,
    cwd: Path,
    environment: dict[str, str],
    timeout: int,
) -> subprocess.CompletedProcess[bytes]:
    """Run a candidate command in a new process group and kill the group on timeout."""

    try:
        process = subprocess.Popen(
            command,
            cwd=cwd,
            env=environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            stdout, _ = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, _ = process.communicate()
            raise verify.VerificationError(
                f"candidate command exceeded {timeout} seconds: {command[0]}"
            ) from error
    except OSError as error:
        raise verify.VerificationError(
            f"candidate command could not execute: {error}"
        ) from error
    return subprocess.CompletedProcess(command, process.returncode, stdout, b"")


def _control_file_bytes(path: Path, label: str) -> bytes:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return b""
    except OSError as error:
        raise verify.VerificationError(
            f"cannot inspect Git {label}: {error}"
        ) from error
    if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
        raise verify.VerificationError(f"Git {label} must be absent or regular")
    if info.st_size > 1024 * 1024:
        raise verify.VerificationError(f"Git {label} exceeds 1 MiB")
    try:
        data = path.read_bytes()
    except OSError as error:
        raise verify.VerificationError(f"cannot read Git {label}: {error}") from error
    if len(data) != info.st_size:
        raise verify.VerificationError(f"Git {label} changed while reading")
    return data


def _effective_control_lines(data: bytes, label: str) -> list[bytes]:
    lines = []
    for line in data.splitlines():
        stripped = line.strip()
        if stripped and not stripped.startswith(b"#"):
            lines.append(stripped)
    if lines:
        raise verify.VerificationError(f"Git {label} contains active local input")
    return lines


def _dangerous_untracked_path(path: str) -> bool:
    parts = Path(path).parts
    if not parts:
        return True
    if parts[0] in {"src", "tests", "benches", "examples", ".cargo"}:
        return True
    name = parts[-1]
    return (
        name in {"Cargo.lock", "Cargo.toml", "build.rs", "rust-toolchain.toml"}
        or name.endswith(".rs")
        or path.startswith("scripts/milestones/v53/")
    )


def _decode_git_paths(data: bytes, label: str) -> list[str]:
    paths = []
    for raw in data.split(b"\0"):
        if not raw:
            continue
        try:
            path = raw.decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise verify.VerificationError(f"non-UTF-8 path in {label}") from error
        if any(ord(character) < 32 or ord(character) == 127 for character in path):
            raise verify.VerificationError(f"control character path in {label}")
        paths.append(path)
    return paths


def _audit_repository_metadata(repo: Path) -> str:
    """Reject Git metadata/worktree inputs that can change object interpretation."""

    replacement_refs = verify.run_git(
        repo, ["for-each-ref", "--format=%(refname)", "refs/replace"]
    )
    if replacement_refs.strip():
        raise verify.VerificationError(
            "Git replacement refs are forbidden during scope verification"
        )
    shallow = (
        verify.run_git(repo, ["rev-parse", "--is-shallow-repository"])
        .decode("ascii", errors="strict")
        .strip()
    )
    if shallow != "false":
        raise verify.VerificationError("shallow repositories are forbidden")
    sparse = (
        verify.run_git(
            repo, ["config", "--bool", "--default", "false", "core.sparseCheckout"]
        )
        .decode("ascii", errors="strict")
        .strip()
    )
    if sparse != "false":
        raise verify.VerificationError("sparse checkout is forbidden")

    config_names_raw = verify.run_git(
        repo, ["config", "--local", "--null", "--name-only", "--list"]
    )
    try:
        config_names = [
            name.decode("utf-8", errors="strict").lower()
            for name in config_names_raw.split(b"\0")
            if name
        ]
    except UnicodeDecodeError as error:
        raise verify.VerificationError(
            "local Git config contains non-UTF-8 keys"
        ) from error
    dangerous_exact = {
        "core.attributesfile",
        "core.excludesfile",
        "core.fsmonitor",
        "core.preloadindex",
        "core.untrackedcache",
        "extensions.partialclone",
    }
    for name in config_names:
        if (
            name in dangerous_exact
            or name.startswith("include.")
            or name.startswith("includeif.")
            or name.endswith(".promisor")
            or name.endswith(".partialclonefilter")
        ):
            raise verify.VerificationError(
                f"dangerous local Git config is forbidden: {name}"
            )
    config_bytes = verify.run_git(repo, ["config", "--local", "--null", "--list"])

    common_text = (
        verify.run_git(repo, ["rev-parse", "--git-common-dir"])
        .decode("utf-8", errors="strict")
        .strip()
    )
    common_path = Path(common_text)
    if not common_path.is_absolute():
        common_path = repo / common_path
    try:
        common_path = common_path.resolve(strict=True)
    except OSError as error:
        raise verify.VerificationError(
            f"Git common directory cannot be resolved: {error}"
        ) from error
    controls = {
        "grafts": _control_file_bytes(common_path / "info/grafts", "grafts"),
        "info/exclude": _control_file_bytes(
            common_path / "info/exclude", "info/exclude"
        ),
        "object alternates": _control_file_bytes(
            common_path / "objects/info/alternates", "object alternates"
        ),
    }
    for label, data in controls.items():
        _effective_control_lines(data, label)
    if any((common_path / "objects/pack").glob("*.promisor")):
        raise verify.VerificationError("promisor pack files are forbidden")

    flags = verify.run_git(repo, ["ls-files", "-v", "-z"])
    for record in flags.split(b"\0"):
        if not record:
            continue
        if len(record) < 3 or record[1:2] != b" ":
            raise verify.VerificationError("malformed Git index flag output")
        tag = record[:1]
        if tag == b"S" or (b"a" <= tag <= b"z"):
            raise verify.VerificationError(
                "assume-unchanged/skip-worktree index flags are forbidden"
            )

    try:
        (repo / ".cargo").lstat()
    except FileNotFoundError:
        pass
    except OSError as error:
        raise verify.VerificationError(
            f"cannot inspect repository .cargo: {error}"
        ) from error
    else:
        raise verify.VerificationError("repository-local .cargo input is forbidden")

    untracked = _decode_git_paths(
        verify.run_git(repo, ["ls-files", "--others", "--exclude-standard", "-z"]),
        "untracked inventory",
    )
    ignored = _decode_git_paths(
        verify.run_git(
            repo,
            ["ls-files", "--others", "--ignored", "--exclude-standard", "-z"],
        ),
        "ignored inventory",
    )
    dangerous = sorted(
        path for path in {*untracked, *ignored} if _dangerous_untracked_path(path)
    )
    if dangerous:
        raise verify.VerificationError(
            f"dangerous ignored/untracked repository input is present: {dangerous[0]}"
        )

    fingerprint_parts = [config_bytes]
    for label in sorted(controls):
        fingerprint_parts.extend((label.encode("utf-8"), controls[label]))
    return verify.sha256_bytes(b"\0".join(fingerprint_parts))


def _assert_execution_seal(
    repo: Path, candidate: str, toolchain: dict[str, object]
) -> None:
    _assert_cargo_unchanged(toolchain)
    fingerprint = _audit_repository_metadata(repo)
    if fingerprint != toolchain.get("repository_metadata_fingerprint"):
        raise verify.VerificationError(
            "repository metadata changed during candidate execution"
        )
    if verify.resolve_commit(repo, "HEAD") != candidate:
        raise verify.VerificationError("candidate HEAD changed during execution")
    verify.git_status_clean(repo)


def _parse_name_status(data: bytes) -> list[tuple[str, str]]:
    fields = data.split(b"\0")
    if fields and fields[-1] == b"":
        fields.pop()
    if len(fields) % 2:
        raise verify.VerificationError("malformed NUL-delimited Git name-status output")
    changes: list[tuple[str, str]] = []
    for index in range(0, len(fields), 2):
        try:
            status = fields[index].decode("ascii", errors="strict")
            path = fields[index + 1].decode("utf-8", errors="strict")
        except UnicodeDecodeError as error:
            raise verify.VerificationError(
                "non-canonical path/status in Git diff"
            ) from error
        if (
            not status
            or not path
            or any(ord(character) < 32 or ord(character) == 127 for character in path)
        ):
            raise verify.VerificationError("empty/control path in Git diff")
        changes.append((status, path))
    return changes


def _is_allowed(path: str, scope: dict[str, object]) -> bool:
    if path in scope["allowed_exact"]:
        return True
    if any(path.startswith(prefix) for prefix in scope["allowed_prefixes"]):
        return True
    return any(fnmatch.fnmatchcase(path, pattern) for pattern in scope["allowed_globs"])


def _check_repo_checkout(repo: Path, candidate: str, require_clean: bool) -> None:
    sparse = (
        verify.run_git(
            repo, ["config", "--bool", "--default", "false", "core.sparseCheckout"]
        )
        .decode("ascii", errors="strict")
        .strip()
    )
    if sparse == "true":
        raise verify.VerificationError(
            "sparse checkout is forbidden for scope verification"
        )
    if require_clean:
        if verify.resolve_commit(repo, "HEAD") != candidate:
            raise verify.VerificationError(
                "--require-clean requires candidate to be HEAD"
            )
        verify.git_status_clean(repo)
        ignored = verify.run_git(
            repo,
            [
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
                "--",
                "src/memory/execution_observation",
                "tests",
            ],
        )
        relevant = [
            path
            for path in ignored.split(b"\0")
            if path
            and (
                path.startswith(b"src/memory/execution_observation/")
                or path.startswith(b"tests/execution_observation_")
            )
        ]
        if relevant:
            raise verify.VerificationError(
                "ignored v53 implementation/test files are present"
            )


def _candidate_files(repo: Path, candidate: str) -> dict[str, bytes]:
    names_data = verify.run_git(
        repo,
        [
            "ls-tree",
            "-r",
            "--name-only",
            "-z",
            candidate,
            "--",
            "src/memory/execution_observation",
            "tests",
        ],
    )
    result: dict[str, bytes] = {}
    for raw in names_data.split(b"\0"):
        if not raw:
            continue
        path = raw.decode("utf-8", errors="strict")
        if path.endswith(".rs") and (
            path.startswith("src/memory/execution_observation/")
            or fnmatch.fnmatchcase(path, "tests/execution_observation_*.rs")
        ):
            _, _, data = verify.git_object(repo, candidate, path)
            result[path] = data
    return result


def _scan_observation_source(
    path: str,
    data: bytes,
    *,
    maximum_bytes: int,
    maximum_lines_exclusive: int,
) -> None:
    if len(data) > maximum_bytes:
        raise verify.VerificationError(f"observation source exceeds byte limit: {path}")
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise verify.VerificationError(
            f"observation Rust source is not UTF-8: {path}"
        ) from error
    if len(text.splitlines()) >= maximum_lines_exclusive:
        raise verify.VerificationError(
            f"observation source must remain below 300 lines: {path}"
        )
    _scan_rust_tokens(path, text, observation=True)


def _read_lcov(path: Path, repo: Path) -> dict[str, dict[int, int]]:
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(path, flags)
    except OSError as error:
        raise verify.VerificationError(f"cannot open coverage LCOV: {error}") from error
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid():
            raise verify.VerificationError(
                "coverage LCOV must be a current-owner regular file"
            )
        if info.st_size <= 0 or info.st_size > 64 * 1024 * 1024:
            raise verify.VerificationError("coverage LCOV is empty or exceeds 64 MiB")
        chunks: list[bytes] = []
        remaining = info.st_size
        while remaining:
            chunk = os.read(fd, min(remaining, 1024 * 1024))
            if not chunk:
                raise verify.VerificationError("coverage LCOV changed while reading")
            chunks.append(chunk)
            remaining -= len(chunk)
        data = b"".join(chunks)
    finally:
        os.close(fd)
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise verify.VerificationError("coverage LCOV is not UTF-8") from error

    repo_real = os.path.realpath(repo)
    records: dict[str, dict[int, int]] = {}
    current: str | None = None
    ended = True
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("SF:"):
            if not ended:
                raise verify.VerificationError(
                    f"LCOV nested SF record at line {number}"
                )
            source = line[3:]
            if not source:
                raise verify.VerificationError(
                    f"LCOV empty source path at line {number}"
                )
            if os.path.isabs(source):
                source_real = os.path.realpath(source)
                try:
                    if os.path.commonpath((repo_real, source_real)) != repo_real:
                        raise verify.VerificationError(
                            "LCOV source is outside the candidate repository"
                        )
                except ValueError as error:
                    raise verify.VerificationError(
                        "LCOV source path is not comparable to repository"
                    ) from error
                source = os.path.relpath(source_real, repo_real)
            else:
                normalized = os.path.normpath(source)
                if normalized == ".." or normalized.startswith("../"):
                    raise verify.VerificationError(
                        "LCOV relative source escapes the repository"
                    )
                source = normalized
            if source in records:
                raise verify.VerificationError(
                    f"LCOV duplicate source record: {source}"
                )
            records[source] = {}
            current = source
            ended = False
        elif line.startswith("DA:"):
            if current is None or ended:
                raise verify.VerificationError(
                    f"LCOV DA outside source record at line {number}"
                )
            parts = line[3:].split(",")
            if len(parts) < 2 or not parts[0].isdigit() or not parts[1].isdigit():
                raise verify.VerificationError(f"LCOV malformed DA at line {number}")
            source_line, hits = int(parts[0]), int(parts[1])
            if source_line <= 0 or source_line in records[current]:
                raise verify.VerificationError(
                    f"LCOV duplicate/invalid executable line at line {number}"
                )
            records[current][source_line] = hits
        elif line == "end_of_record":
            if current is None or ended:
                raise verify.VerificationError(
                    f"LCOV unmatched end_of_record at line {number}"
                )
            current = None
            ended = True
    if not ended:
        raise verify.VerificationError("LCOV final source record is not terminated")
    if not records:
        raise verify.VerificationError("LCOV contains no source records")
    return records


def _verify_coverage(
    lcov_path: Path,
    repo: Path,
    candidate: str,
    candidate_files: dict[str, bytes],
    contract: dict[str, object],
) -> dict[str, object]:
    records = _read_lcov(lcov_path, repo)
    for path, lines in records.items():
        if not path.endswith(".rs"):
            raise verify.VerificationError(f"LCOV source is not a Rust file: {path}")
        mode, _, git_bytes = verify.git_object(repo, candidate, path)
        worktree_path = repo / path
        try:
            info = worktree_path.lstat()
        except OSError as error:
            raise verify.VerificationError(
                f"LCOV source is absent from candidate worktree: {path}"
            ) from error
        if (
            mode != "100644"
            or not stat.S_ISREG(info.st_mode)
            or worktree_path.read_bytes() != git_bytes
        ):
            raise verify.VerificationError(
                f"LCOV source is not the candidate regular Git blob: {path}"
            )
        try:
            source_lines = len(git_bytes.decode("utf-8", errors="strict").splitlines())
        except UnicodeDecodeError as error:
            raise verify.VerificationError(
                f"LCOV Rust source is not UTF-8: {path}"
            ) from error
        if any(line > source_lines for line in lines):
            raise verify.VerificationError(
                f"LCOV DA line exceeds candidate source: {path}"
            )
    total_found = sum(len(lines) for lines in records.values())
    total_hit = sum(
        sum(1 for hits in lines.values() if hits > 0) for lines in records.values()
    )
    if total_found == 0:
        raise verify.VerificationError("LCOV reports zero executable lines")
    minimum_found = contract["baseline_global"]["executable_lines"]
    if total_found < minimum_found:
        raise verify.VerificationError(
            f"LCOV executable-line denominator is below frozen baseline: {total_found}/{minimum_found}"
        )
    global_minimum = Decimal(contract["global_minimum_percent"])
    if Decimal(total_hit * 100) < global_minimum * Decimal(total_found):
        raise verify.VerificationError(
            f"global line coverage is below {global_minimum}%: {total_hit}/{total_found}"
        )

    candidate_observation = {
        path
        for path in candidate_files
        if path.startswith("src/memory/execution_observation/")
    }
    observation_records = {
        path: lines
        for path, lines in records.items()
        if path.startswith("src/memory/execution_observation/")
    }
    if not observation_records or not (
        set(observation_records) & candidate_observation
    ):
        raise verify.VerificationError(
            "LCOV has no candidate observation module record"
        )
    observation_found = sum(len(lines) for lines in observation_records.values())
    observation_hit = sum(
        sum(1 for hits in lines.values() if hits > 0)
        for lines in observation_records.values()
    )
    if observation_found == 0:
        raise verify.VerificationError(
            "LCOV observation module has zero executable lines"
        )
    observation_minimum = Decimal(contract["observation_minimum_percent"])
    if Decimal(observation_hit * 100) < observation_minimum * Decimal(
        observation_found
    ):
        raise verify.VerificationError(
            f"observation line coverage is below {observation_minimum}%: "
            f"{observation_hit}/{observation_found}"
        )
    return {
        "global": f"{total_hit}/{total_found}",
        "observation": f"{observation_hit}/{observation_found}",
    }


def _run_and_verify_coverage(
    repo: Path,
    candidate: str,
    candidate_files: dict[str, bytes],
    contract: dict[str, object],
    toolchain: dict[str, object],
) -> dict[str, object]:
    _assert_cargo_unchanged(toolchain)
    if verify.resolve_commit(repo, "HEAD") != candidate:
        raise verify.VerificationError("coverage must run at candidate HEAD")
    verify.git_status_clean(repo)
    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        execution_root,
    ):
        output_name = os.fspath(execution_root / "coverage.lcov")
        command = [
            os.fspath(toolchain["cargo_path"]),
            "llvm-cov",
            "--locked",
            "--lib",
            "--all-features",
            "--lcov",
            "--output-path",
            output_name,
        ]
        environment.update(contract["environment"])
        result = _run_bounded_process(
            command,
            cwd=repo,
            environment=environment,
            timeout=contract["timeout_seconds"],
        )
        if result.returncode != 0:
            lines = result.stdout.decode("utf-8", errors="replace").strip().splitlines()
            detail = lines[-1] if lines else "no output"
            raise verify.VerificationError(
                f"frozen coverage command exited nonzero: {detail}"
            )
        _assert_execution_seal(repo, candidate, toolchain)
        return _verify_coverage(
            Path(output_name), repo, candidate, candidate_files, contract
        )


def _parse_listed_f_tests(output: str) -> dict[str, list[str]]:
    listed = {f"F{index:02d}": [] for index in range(1, 17)}
    seen: set[str] = set()
    for match in LISTED_F_TEST.finditer(output):
        test_id = f"F{match.group('id')}"
        name = match.group("name")
        if test_id not in listed:
            raise verify.VerificationError(f"cargo listed unknown F-test id: {test_id}")
        if name in seen:
            raise verify.VerificationError(
                f"cargo listed duplicate F-test identity: {name}"
            )
        seen.add(name)
        listed[test_id].append(name)
    return listed


def _parse_exact_f_test_execution(output: str, expected_name: str) -> None:
    escaped = re.escape(expected_name)
    ok_lines = re.findall(rf"(?m)^test\s+{escaped}\s+\.\.\.\s+ok\s*$", output)
    ignored_lines = re.findall(
        rf"(?m)^test\s+{escaped}\s+\.\.\.\s+ignored(?:,.*)?\s*$", output
    )
    one_pass_summaries = re.findall(
        r"(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored; \d+ measured; \d+ filtered out;"
        r" finished in .+$",
        output,
    )
    if ignored_lines or len(ok_lines) != 1 or len(one_pass_summaries) != 1:
        raise verify.VerificationError(
            f"exact F-test did not prove one non-ignored execution: {expected_name}"
        )


def _run_required_f_tests(
    source_repo: Path,
    candidate: str,
    candidate_checkout: Path,
    candidate_manifest: dict[str, tuple[str, str]],
    required_test_ids: set[str],
    test_contract: dict[str, object],
    toolchain: dict[str, object],
) -> dict[str, list[str]]:
    _assert_execution_seal(source_repo, candidate, toolchain)
    _verify_materialized_tree(candidate_checkout, candidate_manifest)
    with _isolated_candidate_environment(toolchain["environment"]) as (
        environment,
        _,
    ):
        list_command = [
            os.fspath(toolchain["cargo_path"]),
            "test",
            "--locked",
            "--all-features",
            "execution_observation_f",
            "--",
            "--list",
        ]
        result = _run_bounded_process(
            list_command,
            cwd=candidate_checkout,
            environment=environment,
            timeout=1200,
        )
        output = result.stdout.decode("utf-8", errors="replace")
        if result.returncode != 0:
            detail = output.strip().splitlines()
            raise verify.VerificationError(
                f"F-test command exited nonzero: {detail[-1] if detail else 'no output'}"
            )
        _assert_execution_seal(source_repo, candidate, toolchain)
        _verify_materialized_tree(candidate_checkout, candidate_manifest)
        listed = _parse_listed_f_tests(output)
        executed = {test_id: [] for test_id in sorted(required_test_ids)}
        for test_id in sorted(required_test_ids):
            names = listed[test_id]
            minimum = test_contract[test_id]["minimum_tests"]
            if len(names) < minimum:
                raise verify.VerificationError(
                    f"cargo listed {len(names)} tests for {test_id}, required {minimum}"
                )
            for name in names:
                exact_command = [
                    os.fspath(toolchain["cargo_path"]),
                    "test",
                    "--locked",
                    "--all-features",
                    name,
                    "--",
                    "--exact",
                    "--nocapture",
                ]
                exact = _run_bounded_process(
                    exact_command,
                    cwd=candidate_checkout,
                    environment=environment,
                    timeout=1200,
                )
                exact_output = exact.stdout.decode("utf-8", errors="replace")
                if exact.returncode != 0:
                    detail = exact_output.strip().splitlines()
                    raise verify.VerificationError(
                        f"exact F-test exited nonzero: {detail[-1] if detail else 'no output'}"
                    )
                _parse_exact_f_test_execution(exact_output, name)
                _assert_execution_seal(source_repo, candidate, toolchain)
                _verify_materialized_tree(candidate_checkout, candidate_manifest)
                executed[test_id].append(name)
        return executed


def _extract_git_archive(
    repo: Path, commit: str, destination: Path
) -> dict[str, tuple[str, str]]:
    """Materialize exact Git blobs without archive attributes or the worktree."""

    destination.mkdir(mode=0o700)
    tree = verify.run_git(repo, ["ls-tree", "-r", "-z", commit])
    manifest: dict[str, tuple[str, str]] = {}
    for record in tree.split(b"\0"):
        if not record:
            continue
        try:
            header, raw_path = record.split(b"\t", 1)
            mode, kind, object_id = header.decode("ascii", errors="strict").split()
            path = raw_path.decode("utf-8", errors="strict")
        except (UnicodeDecodeError, ValueError) as error:
            raise verify.VerificationError("malformed Git tree entry") from error
        parts = Path(path).parts
        if (
            not parts
            or path.startswith("/")
            or ".." in parts
            or "\\" in path
            or any(ord(character) < 32 or ord(character) == 127 for character in path)
        ):
            raise verify.VerificationError("Git tree contains an unsafe path")
        if ".git" in parts:
            raise verify.VerificationError(
                "Git object tree may not materialize repository-control paths"
            )
        if parts[0] == ".cargo":
            raise verify.VerificationError(
                "repository-local .cargo is forbidden in the Git object tree"
            )
        if mode not in {"100644", "100755"} or kind != "blob":
            raise verify.VerificationError(
                f"Git tree contains symlink/special/submodule entry: {path}"
            )
        if path in manifest:
            raise verify.VerificationError(f"duplicate Git tree path: {path}")
        data = verify.run_git(repo, ["cat-file", "blob", object_id])
        digest = verify.sha256_bytes(data)
        target = destination.joinpath(*parts)
        target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        try:
            target.write_bytes(data)
            target.chmod(0o500 if mode == "100755" else 0o400)
        except OSError as error:
            raise verify.VerificationError(
                f"cannot materialize Git object path {path}: {error}"
            ) from error
        manifest[path] = (mode, digest)
    if not manifest:
        raise verify.VerificationError("Git tree materialization produced no files")
    return manifest


def _verify_materialized_tree(
    checkout: Path, manifest: dict[str, tuple[str, str]]
) -> None:
    observed: set[str] = set()
    for path in checkout.rglob("*"):
        relative = path.relative_to(checkout).as_posix()
        try:
            info = path.lstat()
        except OSError as error:
            raise verify.VerificationError(
                f"cannot inspect materialized candidate path {relative}: {error}"
            ) from error
        if stat.S_ISDIR(info.st_mode):
            continue
        if not stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            raise verify.VerificationError(
                f"materialized candidate contains special path: {relative}"
            )
        expected = manifest.get(relative)
        if expected is None:
            raise verify.VerificationError(
                f"candidate command created an unbound source path: {relative}"
            )
        executable = bool(info.st_mode & 0o111)
        if executable != (expected[0] == "100755"):
            raise verify.VerificationError(
                f"candidate command changed source mode: {relative}"
            )
        if verify.sha256_bytes(path.read_bytes()) != expected[1]:
            raise verify.VerificationError(
                f"candidate command changed Git object bytes: {relative}"
            )
        observed.add(relative)
    missing = set(manifest) - observed
    if missing:
        raise verify.VerificationError(
            f"materialized candidate lost Git object path: {sorted(missing)[0]}"
        )


def _verified_candidate_files_from_checkout(
    checkout: Path, candidate_files: dict[str, bytes]
) -> dict[str, bytes]:
    result: dict[str, bytes] = {}
    for relative, expected in candidate_files.items():
        path = checkout / relative
        try:
            info = path.lstat()
            data = path.read_bytes()
        except OSError as error:
            raise verify.VerificationError(
                f"cannot read materialized candidate source {relative}: {error}"
            ) from error
        if (
            not stat.S_ISREG(info.st_mode)
            or stat.S_ISLNK(info.st_mode)
            or data != expected
        ):
            raise verify.VerificationError(
                f"materialized source differs from candidate Git object: {relative}"
            )
        result[relative] = data
    return result


def _run_wp1_archive_gate(
    source_repo: Path,
    base: str,
    candidate: str,
    scope: dict[str, object],
    test_contract: dict[str, object],
    toolchain: dict[str, object],
) -> tuple[dict[str, bytes], dict[str, list[str]]]:
    """Scan/build/test exact object materializations, never the developer worktree."""

    with tempfile.TemporaryDirectory(prefix="plico-v53-scope-objects-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        base_checkout = root / "base"
        candidate_checkout = root / "candidate"
        base_manifest = _extract_git_archive(source_repo, base, base_checkout)
        candidate_manifest = _extract_git_archive(
            source_repo, candidate, candidate_checkout
        )
        _verify_materialized_tree(base_checkout, base_manifest)
        _verify_materialized_tree(candidate_checkout, candidate_manifest)

        object_files = _candidate_files(source_repo, candidate)
        candidate_files = _verified_candidate_files_from_checkout(
            candidate_checkout, object_files
        )
        observation_sources = {
            path: data
            for path, data in candidate_files.items()
            if path.startswith("src/memory/execution_observation/")
        }
        if not observation_sources:
            raise verify.VerificationError(
                "candidate has no observation module Rust source"
            )
        for path, data in observation_sources.items():
            _scan_observation_source(
                path,
                data,
                maximum_bytes=scope["observation_file_max_bytes"],
                maximum_lines_exclusive=scope["observation_file_max_lines_exclusive"],
            )

        declarations: dict[str, int] = {f"F{index:02d}": 0 for index in range(1, 17)}
        total_tests = 0
        for path, data in candidate_files.items():
            text = data.decode("utf-8", errors="strict")
            for match in TEST_DECLARATION.finditer(text):
                test_id = f"F{match.group(2)}"
                if test_id not in declarations:
                    raise verify.VerificationError(
                        f"unknown F-test id in {path}: {test_id}"
                    )
                declarations[test_id] += 1
                total_tests += 1
        if total_tests == 0:
            raise verify.VerificationError(
                "scope gate matched zero execution_observation_fNN tests"
            )

        required_test_ids = {
            test_id
            for test_id, contract in test_contract.items()
            if contract["work_package"] == "WP1"
        }
        for test_id in sorted(required_test_ids):
            minimum = test_contract[test_id]["minimum_tests"]
            if declarations[test_id] < minimum:
                raise verify.VerificationError(
                    f"WP1 requires at least {minimum} source declaration for {test_id}"
                )

        candidate_self_evidence = _run_required_f_tests(
            source_repo,
            candidate,
            candidate_checkout,
            candidate_manifest,
            required_test_ids,
            test_contract,
            toolchain,
        )
        _verify_materialized_tree(candidate_checkout, candidate_manifest)
        return candidate_files, candidate_self_evidence


def _normalize_semantic(value: object, key: str = "") -> object:
    if isinstance(value, dict):
        return {
            name: _normalize_semantic(item, name)
            for name, item in sorted(value.items())
        }
    if isinstance(value, list):
        return [_normalize_semantic(item, key) for item in value]
    if isinstance(value, str):
        if UUID_TEXT.fullmatch(value):
            return "<uuid>"
        if SHA_TEXT.fullmatch(value):
            return "<sha256>"
        return value
    if isinstance(value, int) and any(
        token in key for token in ("time", "created", "updated", "recorded")
    ):
        return "<writer-time>"
    return value


def _run_lifecycle_cli(binary: Path, vault: Path, operation: list[str]) -> object:
    if not Path("/proc/self/task").is_dir() or not Path("/proc/self/fd").is_dir():
        raise verify.VerificationError(
            "lifecycle thread/handle proof requires Linux /proc"
        )
    command = [os.fspath(binary), "--embedded", "--root", os.fspath(vault), *operation]
    environment = os.environ.copy()
    environment.update(
        {"EMBEDDING_BACKEND": "stub", "LLM_BACKEND": "stub", "RUST_LOG": "off"}
    )
    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=environment,
        start_new_session=True,
    )
    deadline = time.monotonic() + 120
    observation_resource = False
    samples = 0
    maximum_threads = 0
    maximum_handles = 0
    while process.poll() is None:
        if time.monotonic() >= deadline:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise verify.VerificationError("lifecycle CLI exceeded 120 seconds")
        proc = Path("/proc") / str(process.pid)
        tasks = list((proc / "task").glob("*/comm"))
        descriptors = list((proc / "fd").glob("*"))
        samples += 1
        maximum_threads = max(maximum_threads, len(tasks))
        maximum_handles = max(maximum_handles, len(descriptors))
        for task in tasks:
            try:
                observation_resource |= "execution-observation" in task.read_text(
                    encoding="utf-8", errors="replace"
                )
            except OSError:
                pass
        for descriptor in descriptors:
            try:
                observation_resource |= (
                    "execution-observation-fixture-ledger" in os.readlink(descriptor)
                )
            except OSError:
                pass
        time.sleep(0.005)
    stdout, stderr = process.communicate()
    if process.returncode != 0:
        detail = stderr.decode("utf-8", errors="replace").strip().splitlines()
        raise verify.VerificationError(
            f"lifecycle CLI failed: {detail[-1] if detail else 'no stderr'}"
        )
    if observation_resource:
        raise verify.VerificationError(
            "production lifecycle exposed an observation thread/handle"
        )
    if samples == 0:
        raise verify.VerificationError(
            "lifecycle process ended before thread/handle sampling"
        )
    return {
        "response": _normalize_semantic(
            verify.strict_json_loads(stdout, "lifecycle CLI response")
        ),
        "maximum_threads": maximum_threads,
        "maximum_handles": maximum_handles,
        "observation_resource_absent": True,
    }


def _normalize_inventory_path(relative: Path) -> str:
    parts = []
    for part in relative.parts:
        if SHA_TEXT.fullmatch(part):
            parts.append("<sha256>")
        elif UUID_TEXT.fullmatch(part):
            parts.append("<uuid>")
        else:
            parts.append(part)
    return "/".join(parts)


def _vault_inventory(vault: Path) -> list[tuple[object, ...]]:
    inventory: collections.Counter[tuple[object, ...]] = collections.Counter()
    for path in sorted(vault.rglob("*")):
        relative = path.relative_to(vault)
        normalized = _normalize_inventory_path(relative)
        if "execution-observation-fixture-ledger" in relative.parts:
            raise verify.VerificationError(
                "production lifecycle created observation namespace"
            )
        info = path.lstat()
        if stat.S_ISLNK(info.st_mode) or not (
            stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)
        ):
            raise verify.VerificationError(
                "lifecycle vault contains symlink/special state"
            )
        if stat.S_ISDIR(info.st_mode):
            inventory[("dir", normalized, stat.S_IMODE(info.st_mode))] += 1
            continue
        data = path.read_bytes()
        schemas: set[str] = set()
        for line in data.splitlines() or [data]:
            try:
                parsed = json.loads(line)
            except (UnicodeDecodeError, json.JSONDecodeError):
                continue
            stack = [parsed]
            while stack:
                item = stack.pop()
                if isinstance(item, dict):
                    schema = item.get("schema")
                    if isinstance(schema, str):
                        schemas.add(schema)
                    stack.extend(item.values())
                elif isinstance(item, list):
                    stack.extend(item)
        inventory[
            (
                "file",
                normalized,
                stat.S_IMODE(info.st_mode),
                len(data),
                tuple(sorted(schemas)),
            )
        ] += 1
    return sorted((*key, count) for key, count in inventory.items())


def _run_lifecycle_checkout(
    checkout: Path, target: Path, toolchain: dict[str, object]
) -> dict[str, object]:
    _assert_cargo_unchanged(toolchain)
    environment = toolchain["environment"].copy()
    environment.update(
        {
            "CARGO_TARGET_DIR": os.fspath(target),
            "EMBEDDING_BACKEND": "stub",
            "LLM_BACKEND": "stub",
        }
    )
    result = _run_bounded_process(
        [os.fspath(toolchain["cargo_path"]), "build", "--locked", "--bin", "aicli"],
        cwd=checkout,
        environment=environment,
        timeout=1200,
    )
    if result.returncode != 0:
        output = result.stdout.decode("utf-8", errors="replace").strip().splitlines()
        raise verify.VerificationError(
            f"lifecycle aicli build failed: {output[-1] if output else 'no output'}"
        )
    _assert_cargo_unchanged(toolchain)
    binary = target / "debug/aicli"
    vault = checkout.parent / f"{checkout.name}-vault"
    responses = [
        _run_lifecycle_cli(binary, vault, ["capabilities.describe"]),
        _run_lifecycle_cli(
            binary,
            vault,
            [
                "memory.create",
                "--content",
                "plico-v53-deterministic-lifecycle-fixture",
                "--tag",
                "plico:v53:lifecycle",
            ],
        ),
        _run_lifecycle_cli(binary, vault, ["capabilities.describe"]),
        _run_lifecycle_cli(
            binary,
            vault,
            [
                "memory.recall",
                "--query",
                "plico-v53-deterministic-lifecycle-fixture",
                "--limit",
                "5",
            ],
        ),
    ]
    return {"responses": responses, "mutation_inventory": _vault_inventory(vault)}


def _run_lifecycle_differential(
    repo: Path, base: str, candidate: str, toolchain: dict[str, object]
) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="plico-v53-lifecycle-") as temporary:
        root = Path(temporary)
        base_checkout = root / "base"
        candidate_checkout = root / "candidate"
        _extract_git_archive(repo, base, base_checkout)
        _extract_git_archive(repo, candidate, candidate_checkout)
        target = root / "target"
        base_result = _run_lifecycle_checkout(base_checkout, target, toolchain)
        candidate_result = _run_lifecycle_checkout(
            candidate_checkout, target, toolchain
        )
        if base_result != candidate_result:
            raise verify.VerificationError(
                "base/candidate lifecycle semantic fixture or mutation inventory differs"
            )
        return {
            "schema": "plico.v53.lifecycle-differential-result/v1",
            "base": base,
            "candidate": candidate,
            "semantic_fixtures_equal": True,
            "mutation_inventories_equal": True,
            "observation_namespace_thread_handle_absent": True,
        }


def _verify_scope_sanitized(
    handoff_dir: Path,
    repo: Path,
    handoff: dict[str, object],
    toolchain: dict[str, object],
    authorization: dict[str, object],
    *,
    approval_revision: str,
    candidate_revision: str,
    work_package: str,
    require_clean: bool,
) -> dict[str, object]:
    toolchain["repository_metadata_fingerprint"] = _audit_repository_metadata(repo)
    verified_against_repo = verify.verify_handoff(handoff_dir, repo=repo)
    if verified_against_repo != handoff:
        raise verify.VerificationError("R0 packet changed during scope verification")
    if (
        authorization.get("authorization") != "GO"
        or authorization.get("integrity") != "verified"
        or authorization.get("packet_id") != handoff["packet_id"]
    ):
        raise verify.VerificationError(
            "offline authorization result is not a verified GO for this packet"
        )
    base = authorization.get("candidate_scope_base_sha")
    if (
        not isinstance(base, str)
        or base != authorization.get("approval_commit_sha")
        or not verify.GIT_OBJECT_ID.fullmatch(base)
    ):
        raise verify.VerificationError(
            "offline authorization did not return one canonical approval scope base"
        )
    candidate = verify.resolve_commit(repo, candidate_revision)
    ancestor = verify.run_git(repo, ["merge-base", "--is-ancestor", base, candidate])
    if ancestor:
        raise verify.VerificationError("unexpected merge-base output")
    _check_repo_checkout(repo, candidate, require_clean)

    raw = verify.run_git(
        repo,
        ["diff", "--name-status", "-z", "--no-renames", base, candidate, "--"],
    )
    changes = _parse_name_status(raw)
    if not changes:
        raise verify.VerificationError("candidate has no implementation diff")
    scope = handoff["spec"]["developer_scope"]
    if scope["active_work_package"] != work_package:
        raise verify.VerificationError(
            "requested work package differs from the packet-frozen active work package"
        )
    work_package_scope = scope["work_packages"].get(work_package)
    if not isinstance(work_package_scope, dict):
        raise verify.VerificationError(
            "requested work package has no packet-frozen developer allowlist"
        )
    architecture_owned = set(scope["architecture_owned"])
    for status, path in changes:
        if status not in {"A", "M"}:
            raise verify.VerificationError(
                f"delete/type/merge change is forbidden: {status} {path}"
            )
        if path in architecture_owned:
            raise verify.VerificationError(f"architecture-owned file changed: {path}")
        if path in scope["forbidden_exact"] or any(
            path.startswith(prefix) for prefix in scope["forbidden_prefixes"]
        ):
            raise verify.VerificationError(f"forbidden path changed: {path}")
        if not _is_allowed(path, work_package_scope):
            raise verify.VerificationError(
                f"path is outside the frozen {work_package} developer allowlist: {path}"
            )
        mode, _, data = verify.git_object(repo, candidate, path)
        if mode != "100644":
            raise verify.VerificationError(
                f"developer file mode must remain 100644: {path}"
            )
        if path.endswith(".rs"):
            try:
                rust_text = data.decode("utf-8", errors="strict")
            except UnicodeDecodeError as error:
                raise verify.VerificationError(
                    f"changed Rust source is not UTF-8: {path}"
                ) from error
            _scan_rust_tokens(path, rust_text, observation=False)

    _verify_wp1_memory_module_anchor(repo, base, candidate)

    candidate_files, candidate_self_evidence = _run_wp1_archive_gate(
        repo,
        base,
        candidate,
        scope,
        handoff["spec"]["test_contract"],
        toolchain,
    )

    lifecycle_result: dict[str, object] | None = None
    if work_package in {"WP5", "WP6"}:
        lifecycle_result = _run_lifecycle_differential(repo, base, candidate, toolchain)

    coverage_result: dict[str, object] | None = None
    coverage_contract = handoff["spec"]["coverage_contract"]
    if work_package in coverage_contract["required_work_packages"]:
        coverage_result = _run_and_verify_coverage(
            repo, candidate, candidate_files, coverage_contract, toolchain
        )

    _assert_execution_seal(repo, candidate, toolchain)
    final_handoff = verify.verify_handoff(handoff_dir, repo=repo)
    if final_handoff != handoff:
        raise verify.VerificationError("R0 packet changed during candidate execution")
    try:
        final_authorization = authorize.authorize(
            handoff_dir,
            repo,
            approval_revision=approval_revision,
        )
    except authorize.AuthorizationError as error:
        raise verify.VerificationError(
            f"offline approval changed during candidate execution: {error}"
        ) from error
    if final_authorization != authorization:
        raise verify.VerificationError(
            "offline approval ref/record changed during candidate execution"
        )
    return {
        "approval_commit": base,
        "authorization_source": authorization["authorization_source"],
        "base": base,
        "candidate": candidate,
        "changed_paths": len(changes),
        "coverage": coverage_result,
        "candidate_self_evidence_f_tests": candidate_self_evidence,
        "external_architecture_corpus": "required-before-R1-or-later-acceptance",
        "lifecycle_differential": lifecycle_result,
        "toolchain": {
            "cargo_sha256": toolchain["cargo_sha256"],
            "resolved_cargo_sha256": toolchain["resolved_cargo_sha256"],
            "cargo_llvm_cov_sha256": toolchain["cargo_llvm_cov_sha256"],
            "git_sha256": toolchain["git_sha256"],
            "identity": "portable-logical-name-version-content-digest",
        },
        "work_package": work_package,
    }


def verify_scope(
    handoff_dir: Path,
    repo: Path,
    *,
    approval_revision: str,
    candidate_revision: str,
    work_package: str,
    require_clean: bool,
) -> dict[str, object]:
    if work_package != "WP1":
        raise verify.VerificationError(
            "the R0 authorization boundary only permits WP1; later work packages "
            "require a new architecture approval"
        )
    with _sanitized_git_environment():
        try:
            authorization = authorize.authorize(
                handoff_dir,
                repo,
                approval_revision=approval_revision,
            )
        except authorize.AuthorizationError as error:
            raise verify.VerificationError(
                f"offline R0 authorization failed: {error}"
            ) from error
        handoff = verify.verify_handoff(handoff_dir, repo=None)
        toolchain = _resolve_frozen_cargo(
            handoff["spec"], handoff["toolchain_observed"]
        )
        with _absolute_git_runner(toolchain["git_path"]):
            return _verify_scope_sanitized(
                handoff_dir,
                repo,
                handoff,
                toolchain,
                authorization,
                approval_revision=approval_revision,
                candidate_revision=candidate_revision,
                work_package=work_package,
                require_clean=require_clean,
            )


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--handoff-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument(
        "--approval-commit",
        required=True,
        help="allowed v53 approval ref or exact approval commit object id",
    )
    parser.add_argument("--candidate", default="HEAD")
    parser.add_argument("--work-package", choices=["WP1"], required=True)
    parser.add_argument("--require-clean", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        result = verify_scope(
            args.handoff_dir,
            args.repo,
            approval_revision=args.approval_commit,
            candidate_revision=args.candidate,
            work_package=args.work_package,
            require_clean=args.require_clean,
        )
    except verify.VerificationError as error:
        print(f"v53 scope verification failed: {error}", file=sys.stderr)
        return 1
    print(
        "v53 scope verified: "
        f"base={result['base']} candidate={result['candidate']} "
        f"changed_paths={result['changed_paths']} "
        "candidate_self_evidence_f_tests="
        f"{sum(len(names) for names in result['candidate_self_evidence_f_tests'].values())} "
        f"work_package={result['work_package']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
