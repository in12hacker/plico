"""Exact typed projection of the public object.search execution evidence."""

from __future__ import annotations

import os
from typing import Any

RETRIEVAL_PATHS = frozenset(
    {
        "bm25",
        "vector",
        "tag_fallback",
        "knowledge_graph_temporal",
        "knowledge_graph_ppr",
        "knowledge_graph_path_discovery",
        "knowledge_graph_causal",
        "reranker",
    }
)
EMBEDDING_STATES = frozenset({"not_probed", "succeeded", "degraded"})
EMBEDDING_DEGRADATIONS = frozenset(
    {
        "provider_unavailable",
        "model_unavailable",
        "input_rejected",
        "execution_failed",
    }
)


def validate_embedding_query(value: Any) -> tuple[str, str | None]:
    if not isinstance(value, dict) or set(value) not in ({"state"}, {"state", "degradation"}):
        raise RuntimeError("object.search embedding_query has an invalid typed shape")
    state = value.get("state")
    degradation = value.get("degradation")
    if state not in EMBEDDING_STATES:
        raise RuntimeError("object.search embedding_query state is unsupported")
    if degradation is not None and degradation not in EMBEDDING_DEGRADATIONS:
        raise RuntimeError("object.search embedding_query degradation is unsupported")
    if (state == "degraded") != (degradation is not None):
        raise RuntimeError("object.search embedding_query state/degradation disagree")
    return str(state), None if degradation is None else str(degradation)


def validate_retrieval_execution(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list) or not value:
        raise RuntimeError("object.search retrieval execution ledger is missing")
    validated = []
    for item in value:
        if not isinstance(item, dict) or set(item) not in (
            {"path", "candidates", "accepted"},
            {"path", "candidates", "accepted", "degradation"},
        ):
            raise RuntimeError("object.search retrieval execution has an invalid typed shape")
        path = item.get("path")
        candidates = item.get("candidates")
        accepted = item.get("accepted")
        degradation = item.get("degradation")
        if path not in RETRIEVAL_PATHS:
            raise RuntimeError("object.search retrieval path is unsupported")
        if (
            isinstance(candidates, bool)
            or not isinstance(candidates, int)
            or candidates < 0
            or isinstance(accepted, bool)
            or not isinstance(accepted, int)
            or accepted < 0
            or accepted > candidates
            or degradation not in {None, "execution_failed"}
        ):
            raise RuntimeError("object.search retrieval counts/degradation are invalid")
        validated.append(
            {
                "path": str(path),
                "candidates": candidates,
                "accepted": accepted,
                "degradation": degradation,
            }
        )
    return validated


def verified_vector_execution(embedding_state: str, execution: list[dict[str, Any]]) -> bool:
    return (
        embedding_state == "succeeded"
        and all(item["degradation"] is None for item in execution)
        and any(item["path"] == "vector" and item["accepted"] > 0 for item in execution)
    )


def real_embedding_required() -> bool:
    return os.environ.get("PLICO_BENCH_REQUIRE_REAL_EMBEDDING", "").lower() in {
        "1",
        "true",
        "yes",
    }
