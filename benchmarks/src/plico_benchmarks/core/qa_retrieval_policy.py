"""Frozen retrieval policy for the no-KG conversational-QA baseline."""

from __future__ import annotations

import hashlib
import json
import os
from typing import Any

QA_RETRIEVAL_POLICY_ROLE = "conversational_qa_retrieval_policy"
QA_RETRIEVAL_POLICY = {
    "schema": "plico.benchmark.qa-retrieval-policy/v1",
    "knowledge_graph_auto_extract": False,
    "required_execution_paths": ["vector", "bm25"],
    "path_match": "exact_ordered",
    "degradation_allowed": False,
}


def resolve_qa_retrieval_policy() -> dict[str, Any]:
    """Require the daemon-affecting environment to match the frozen policy."""
    configured = os.environ.get("PLICO_KG_AUTO_EXTRACT")
    if configured is None:
        raise RuntimeError("QA no-KG baseline requires PLICO_KG_AUTO_EXTRACT=false")
    if configured.strip().lower() != "false":
        raise RuntimeError("QA no-KG baseline requires PLICO_KG_AUTO_EXTRACT=false")
    return _policy_copy()


def qa_retrieval_policy_artifact(policy: Any) -> dict[str, Any]:
    """Bind the exact embedded policy bytes into the run manifest."""
    validate_qa_retrieval_policy(policy)
    payload = json.dumps(policy, sort_keys=True, separators=(",", ":")).encode()
    return {
        "role": QA_RETRIEVAL_POLICY_ROLE,
        "file_name": f"embedded:{QA_RETRIEVAL_POLICY_ROLE}.json",
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def validate_qa_retrieval_policy(policy: Any) -> None:
    """Reject any policy drift, including truthy or missing KG settings."""
    if policy != QA_RETRIEVAL_POLICY:
        raise ValueError("QA retrieval policy does not match the frozen no-KG contract")


def validate_exact_qa_execution(execution: list[dict[str, Any]]) -> None:
    """Require exactly one undegraded vector path followed by one BM25 path."""
    if [item.get("path") for item in execution] != QA_RETRIEVAL_POLICY[
        "required_execution_paths"
    ] or any(item.get("degradation") is not None for item in execution):
        raise ValueError("QA retrieval execution is not exact undegraded vector+bm25")


def _policy_copy() -> dict[str, Any]:
    return {
        **QA_RETRIEVAL_POLICY,
        "required_execution_paths": list(QA_RETRIEVAL_POLICY["required_execution_paths"]),
    }
