#!/usr/bin/env python3
"""Run packet-free WP2-R2 static developer self-preflight.

This command is deliberately incapable of authorizing a candidate.  It reads
the checked-in architecture spec and committed Git objects, then calls the
same static collector used by formal ``verify_scope.py``.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import verify
import verify_scope


SCHEMA = "plico.v53.wp2-r2-developer-self-preflight/v1"


def run_preflight(
    repo: Path,
    *,
    base_revision: str,
    candidate_revision: str,
    require_clean: bool,
) -> dict[str, object]:
    if not verify.GIT_OBJECT_ID.fullmatch(base_revision):
        raise verify.VerificationError("--base must be one exact A3 commit object id")
    base = verify.resolve_commit(repo, base_revision)
    _, _, spec_bytes = verify.git_object(repo, base, verify.SPEC_PATH)
    spec = verify.validate_spec(verify.strict_json_loads(spec_bytes, verify.SPEC_PATH))
    candidate = verify.resolve_commit(repo, candidate_revision)
    ancestor_output = verify.run_git(
        repo, ["merge-base", "--is-ancestor", base, candidate]
    )
    if ancestor_output:
        raise verify.VerificationError("unexpected merge-base output")

    checkout_issues: list[dict[str, str]] = []
    if require_clean:
        try:
            verify_scope._check_repo_checkout(repo, candidate, True)
        except verify.VerificationError as error:
            checkout_issues.append(
                verify_scope._static_issue("checkout", "checkout_not_clean", str(error))
            )
    evidence = verify_scope.collect_wp2_static_evidence(
        repo, base, candidate, spec["developer_scope"]
    )
    issues = checkout_issues + evidence["issues"]
    evidence["issues"] = issues
    evidence["issue_count"] = len(issues)
    evidence["checks"]["checkout"] = {
        "issue_count": len(checkout_issues),
        "required_clean": require_clean,
    }
    return {
        "authorization": "unverified",
        "gate_eligible": False,
        "schema": SCHEMA,
        "self_evidence_only": True,
        "status": "PASS" if not issues else "FAIL",
        "static_evidence": evidence,
    }


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--base", required=True, help="exact A3 architecture base")
    parser.add_argument("--candidate", default="HEAD")
    parser.add_argument("--require-clean", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        with verify_scope._sanitized_git_environment():
            result = run_preflight(
                args.repo,
                base_revision=args.base,
                candidate_revision=args.candidate,
                require_clean=args.require_clean,
            )
    except (OSError, verify.VerificationError) as error:
        result = {
            "authorization": "unverified",
            "gate_eligible": False,
            "schema": SCHEMA,
            "self_evidence_only": True,
            "status": "FAIL",
            "static_evidence": {
                "issue_count": 1,
                "issues": [
                    {
                        "check": "preflight",
                        "code": "preflight_unavailable",
                        "message": str(error),
                    }
                ],
            },
        }
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
