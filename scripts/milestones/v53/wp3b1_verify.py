#!/usr/bin/env python3
"""Local-only scope verifier for v53 WP3B.1 (ADR-0010 contract freeze).

Checks a candidate worktree against the machine-readable contract in
wp3b1_spec.json:

  1. the candidate descends from the frozen architecture base;
  2. the changed-file set stays inside the allowed paths and touches no
     forbidden path;
  3. the frozen facade API surface is present verbatim in facade.rs;
  4. no facade-forbidden symbol leaks into the facade file;
  5. the corpus JSON parses and carries every required category.

No network, no GitHub, no cargo invocation: pure git + text inspection.

Usage:
  python3 wp3b1_verify.py --repo <path> [--spec <wp3b1_spec.json>]
      [--candidate <commit-ish>] [--base <commit-ish>]
"""

import argparse
import json
import pathlib
import subprocess
import sys

FACADE_FILE = "src/memory/execution_observation/store/facade.rs"
REQUIRED_API_MARKERS = [
    "pub(crate) struct FixtureObservationLedgerV1",
    "pub(crate) fn open_fixture(",
    "pub(crate) fn append_started(",
    "pub(crate) fn append_terminal(",
    "pub(crate) fn read_attempt(",
]


def git(repo: str, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", repo, *args], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def changed_files(repo: str, base: str, candidate: str) -> list[str]:
    out = git(repo, "diff", "--name-only", base, candidate)
    return [line for line in out.splitlines() if line.strip()]


def path_matches(path: str, roots: list[str]) -> bool:
    for root in roots:
        root = root.rstrip("/")
        if path == root or path.startswith(root + "/"):
            return True
    return False


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=True)
    parser.add_argument("--spec", default=str(pathlib.Path(__file__).with_name("wp3b1_spec.json")))
    parser.add_argument("--candidate", default="HEAD")
    parser.add_argument("--base", default=None)
    args = parser.parse_args()

    spec = json.loads(pathlib.Path(args.spec).read_text(encoding="utf-8"))
    base = args.base
    if base is None:
        tag = spec.get("architecture_base_tag")
        if not tag:
            raise SystemExit("spec carries neither architecture_base_tag nor --base")
        base = git(args.repo, "rev-parse", f"refs/tags/{tag}^{{commit}}").strip()

    failures: list[str] = []

    # 1. ancestry
    merge_base = git(args.repo, "merge-base", "--is-ancestor", base, args.candidate)
    # merge-base --is-ancestor prints nothing and exits 0 on success; the git()
    # helper already exits on nonzero.

    # 2. diff scope
    allowed = spec["developer_scope"]["allowed_paths"]
    forbidden = spec["developer_scope"]["forbidden_paths"]
    changed = changed_files(args.repo, base, args.candidate)
    for path in changed:
        if path_matches(path, forbidden):
            failures.append(f"forbidden path touched: {path}")
        elif not path_matches(path, allowed):
            failures.append(f"outside allowed paths: {path}")

    # 3/4. facade surface (only once the facade file exists in the candidate)
    facade_path = pathlib.Path(args.repo) / FACADE_FILE
    if facade_path.exists():
        facade = facade_path.read_text(encoding="utf-8")
        for marker in REQUIRED_API_MARKERS:
            if marker not in facade:
                failures.append(f"frozen API marker missing from facade: {marker}")
        for symbol in spec["developer_scope"]["facade_forbidden_symbols"]:
            if symbol in facade:
                failures.append(f"forbidden symbol in facade: {symbol}")
    elif changed:
        failures.append(f"frozen facade file absent: {FACADE_FILE}")

    # 5. corpus completeness
    corpus_path = (
        pathlib.Path(args.repo)
        / spec["corpus"]["path"]
    )
    if not corpus_path.exists():
        failures.append(f"corpus missing: {spec['corpus']['path']}")
    else:
        corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
        categories = {case["category"] for case in corpus["cases"]}
        for required in spec["corpus"]["categories"]:
            if required not in categories:
                failures.append(f"corpus category missing: {required}")

    print(f"WP3B.1 scope verification against base {base[:12]}")
    print(f"candidate: {git(args.repo, 'rev-parse', args.candidate).strip()}")
    print(f"changed files: {len(changed)}")
    if failures:
        print("RESULT: FAIL")
        for failure in failures:
            print(f"  - {failure}")
        return 1
    print("RESULT: PASS (scope-clean; runtime gates are the candidate's own evidence)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
