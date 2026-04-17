#!/bin/bash
# plico-post-commit.sh — Git post-commit hook for Plico dogfooding.
#
# Automatically stores each commit as a CAS object with module-scoped tags.
# Install: ln -sf ../../scripts/plico-post-commit.sh .git/hooks/post-commit
#
# Set PLICO_DOGFOOD_ROOT to point to your dogfooding storage root.
# Disable temporarily: PLICO_DOGFOOD_SKIP=1 git commit ...

set -euo pipefail

# Skip if explicitly disabled
[[ "${PLICO_DOGFOOD_SKIP:-}" == "1" ]] && exit 0

ROOT="${PLICO_DOGFOOD_ROOT:-/tmp/plico-dogfood}"
AGENT="plico-dev"
export EMBEDDING_BACKEND=stub

# Only run if the aicli binary exists
AICLI="$(git rev-parse --show-toplevel)/target/debug/aicli"
if [[ ! -x "$AICLI" ]]; then
    exit 0
fi

COMMIT_MSG=$(git log -1 --format="%s" HEAD)
COMMIT_HASH=$(git log -1 --format="%H" HEAD)
COMMIT_DATE=$(git log -1 --format="%ai" HEAD)

# Detect changed modules from the diff
CHANGED_MODULES=$(git diff HEAD~1 --name-only 2>/dev/null \
    | grep "^src/" \
    | cut -d/ -f2 \
    | sort -u \
    | sed 's/^/plico:module:/' \
    | tr '\n' ',' \
    | sed 's/,$//')

# Build tag list
TAGS="plico:type:code-change"
if [[ -n "$CHANGED_MODULES" ]]; then
    TAGS="$TAGS,$CHANGED_MODULES"
fi

CONTENT="commit: $COMMIT_HASH
date: $COMMIT_DATE
message: $COMMIT_MSG"

"$AICLI" --root "$ROOT" put \
    --content "$CONTENT" \
    --tags "$TAGS" \
    --agent "$AGENT" \
    >/dev/null 2>&1 || true
