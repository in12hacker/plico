#!/usr/bin/env python3
"""Migration-A.1 architecture-owned corpus runner (A1-R04).

All subprocess invocations are fixed inline literal argument lists (no
shell, no interpolation). Two modes, shared rules:

  preflight — offline build + smoke filter (a0* cases) on the clean twin;
  formal    — offline build + full clean corpus, then the six-mutation
              red matrix (one literal invocation per mutation).

Per-case classification is parsed from the corpus binary's own
`case <name>: <outcome>` and `summary:` lines, so the rule set lives in
exactly one place (the corpus itself).

Usage:
  python3 run_corpus.py --mode preflight
  python3 run_corpus.py --mode formal
"""

import argparse
import pathlib
import re
import subprocess
import sys

HARNESS = pathlib.Path(__file__).resolve().parent / "reference-adapter"
ENV = {
    "PATH": "/usr/local/bin:/usr/bin:/bin",
    "CARGO_NET_OFFLINE": "1",
    "HOME": str(pathlib.Path.home()),
}
RUN_TIMEOUT_SECONDS = 120
MUTATIONS = [
    "mut-ignore-id",
    "mut-no-deadline",
    "mut-late-response-reuse",
    "mut-drop-no-reap",
    "mut-no-wire-cap",
    "mut-loosen-exact14",
]
RMCP_CACHE_DIR = pathlib.Path.home() / ".cargo" / "registry" / "src"
CASE_LINE = re.compile(r"^case ([a-z0-9]+): (pass|FAIL)", re.MULTILINE)


def parse(stdout: str) -> list[tuple[str, str]]:
    return [(name, outcome.lower()) for name, outcome in CASE_LINE.findall(stdout)]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=["preflight", "formal"], required=True)
    args = parser.parse_args()

    build = subprocess.run(
        ["cargo", "build", "--offline", "--tests"],
        cwd=HARNESS, capture_output=True, text=True, env=ENV,
    )
    if build.returncode != 0:
        print("build: FAIL (offline build failed)")
        print(build.stderr[-2000:])
        return 1
    print("build: pass (offline)")

    results: list[tuple[str, str]] = []

    if args.mode == "preflight":
        clean = subprocess.run(
            ["cargo", "test", "--offline", "--test", "corpus", "--", "a0"],
            cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
        )
        results = parse(clean.stdout)
        for name, outcome in results:
            print(f"clean:{name} (must-pass): {outcome if outcome == 'pass' else 'fail'}")
    else:
        clean = subprocess.run(
            ["cargo", "test", "--offline", "--test", "corpus"],
            cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
        )
        results = [(name, outcome) for name, outcome in parse(clean.stdout)]
        for name, outcome in results:
            print(f"clean:{name} (must-pass): {outcome if outcome == 'pass' else 'fail'}")

        mutation_results: list[tuple[str, str]] = []
        if all(outcome == "pass" for _, outcome in results):
            def spawn_literal_ignore() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-ignore-id", "--", "a01"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            def spawn_literal_nodeadline() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-no-deadline", "--", "a03"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            def spawn_literal_late() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-late-response-reuse", "--", "a06"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            def spawn_literal_noreap() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-drop-no-reap", "--", "a07"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            def spawn_literal_nocap() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-no-wire-cap", "--", "a09"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            def spawn_literal_loosen() -> int:
                try:
                    completed = subprocess.run(
                        ["cargo", "test", "--offline", "--test", "corpus", "--features", "mut-loosen-exact14", "--", "a12d"],
                        cwd=HARNESS, capture_output=True, text=True, timeout=RUN_TIMEOUT_SECONDS, env=ENV,
                    )
                    return completed.returncode
                except subprocess.TimeoutExpired:
                    return 1

            mutation_codes = {
                "mut-ignore-id": spawn_literal_ignore(),
                "mut-no-deadline": spawn_literal_nodeadline(),
                "mut-late-response-reuse": spawn_literal_late(),
                "mut-drop-no-reap": spawn_literal_noreap(),
                "mut-no-wire-cap": spawn_literal_nocap(),
                "mut-loosen-exact14": spawn_literal_loosen(),
            }
            for feature in MUTATIONS:
                verdict = "red" if mutation_codes[feature] != 0 else "fail"
                mutation_results.append((feature, verdict))
                print(f"mutation:{feature} (must-red): {verdict}")
        else:
            for feature in MUTATIONS:
                mutation_results.append((feature, "not-run"))
                print(f"mutation:{feature} (must-red): not-run (clean corpus failed first)")

        results = results + mutation_results

    executed = [outcome for _, outcome in results if outcome != "not-run"]
    good = sum(1 for outcome in executed if outcome in {"pass", "red"})
    bad = len(executed) - good
    not_run = len(results) - len(executed)
    print(f"summary: executed={len(executed)} pass-or-red={good} fail={bad} not-run={not_run}")
    print(
        "rmcp-path: "
        + ("registry cache primed (SDK-path evidence available on this machine)"
           if any(RMCP_CACHE_DIR.rglob("rmcp-3.1.3"))
           else "not-run (registry cache not primed)")
    )
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
