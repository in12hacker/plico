"""Deterministic, content-free sample selection for benchmark adapters."""

from __future__ import annotations

import hashlib
import json
from collections.abc import Callable, Iterable
from typing import Any, TypeVar

T = TypeVar("T")


def stable_stratified_sample(
    items: Iterable[T],
    *,
    limit: int | None,
    seed: int,
    namespace: str,
    sample_id: Callable[[T], str],
    stratum: Callable[[T], str],
) -> list[T]:
    """Select a deterministic, balanced sample without depending on input order."""
    materialized = list(items)
    identities = [sample_id(item) for item in materialized]
    if any(not identity for identity in identities) or len(set(identities)) != len(identities):
        raise ValueError(f"{namespace} sample IDs must be non-empty and unique")
    if limit is not None and (isinstance(limit, bool) or not isinstance(limit, int) or limit < 0):
        raise ValueError(f"{namespace} sample limit must be a non-negative integer or all")

    groups: dict[str, list[T]] = {}
    for item in materialized:
        label = stratum(item)
        if not label:
            raise ValueError(f"{namespace} sample stratum must be non-empty")
        groups.setdefault(label, []).append(item)
    for label, group in groups.items():
        group.sort(key=lambda item: _rank(seed, namespace, label, sample_id(item)))

    target = len(materialized) if limit is None else min(limit, len(materialized))
    selected: list[T] = []
    labels = sorted(groups, key=lambda label: _rank(seed, namespace, "stratum", label))
    cursor = 0
    while len(selected) < target:
        progressed = False
        for label in labels:
            group = groups[label]
            if cursor < len(group):
                selected.append(group[cursor])
                progressed = True
                if len(selected) == target:
                    break
        if not progressed:
            break
        cursor += 1
    return sorted(
        selected,
        key=lambda item: _rank(seed, namespace, "execution", sample_id(item)),
    )


def selection_artifact(
    *, role: str, seed: int, profile: str, sample_ids: list[str]
) -> dict[str, Any]:
    """Bind an ordered sample set without persisting questions or answers."""
    if not sample_ids or len(set(sample_ids)) != len(sample_ids):
        raise ValueError("selection artifact requires unique non-empty sample IDs")
    payload = json.dumps(
        {
            "schema": "plico.benchmark.sample-selection/v1",
            "role": role,
            "seed": seed,
            "profile": profile,
            "sample_ids": sample_ids,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return {
        "role": role,
        "file_name": f"embedded:{role}.json",
        "bytes": len(payload),
        "sha256": hashlib.sha256(payload).hexdigest(),
    }


def configured_profile(section: dict[str, Any]) -> str:
    """Resolve the run class to one explicit sampling profile."""
    import os

    sampling = section.get("sampling")
    if not isinstance(sampling, dict):
        raise ValueError("suite sampling configuration is missing")
    requested = os.environ.get("PLICO_BENCH_RUN_CLASS", "").strip()
    profile = requested or str(sampling.get("default_profile", ""))
    if profile == "official-protocol-compatible":
        profile = "official"
    if profile == "official":
        raise RuntimeError(
            "official conformance is unsupported until upstream revisions, full cardinality, "
            "and official adapters are pinned"
        )
    profiles = sampling.get("profiles")
    if not isinstance(profiles, dict) or profile not in profiles:
        raise ValueError(f"unsupported benchmark sampling profile: {profile!r}")
    return profile


def configured_limit(
    section: dict[str, Any], *, profile: str, dataset: str, override: int | None
) -> int | None:
    """Return an exact per-dataset query limit; ``None`` means all."""
    if override is not None:
        if isinstance(override, bool) or override <= 0:
            raise ValueError("samples override must be a positive integer")
        return override
    value = section["sampling"]["profiles"][profile].get(dataset)
    if value == "all":
        return None
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise ValueError(f"invalid {dataset} sample limit for profile {profile}")
    return value


def _rank(seed: int, namespace: str, *parts: str) -> bytes:
    digest = hashlib.sha256()
    digest.update(b"plico.benchmark.sample-rank.v1\0")
    digest.update(str(seed).encode("ascii"))
    for part in (namespace, *parts):
        digest.update(b"\0")
        digest.update(part.encode("utf-8"))
    return digest.digest()
