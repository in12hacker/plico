"""Explicit collectors for concrete P3-A dogfood input artifacts."""

from __future__ import annotations

import json
import math
import os
import struct
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from plico_benchmarks.core.dogfood_artifacts import (
    collect_canonical_inventory,
    collect_projection_inventory,
)
from plico_benchmarks.core.dogfood_io import canonical_json, write_private_exclusive
from plico_benchmarks.core.dogfood_schema import canonical_uuid, safe_label, sha256


def collect_canonical_checkpoint(
    *,
    vault: Path,
    output: Path,
    bundle_run_id: str,
    phase: str,
    daemon_instance_id: str,
    sequence: int,
) -> None:
    """Write one owner-only raw checkpoint of the current memory-ledger tree."""
    _validate_checkpoint_identity(
        vault,
        output,
        bundle_run_id,
        daemon_instance_id,
        phase,
        sequence,
        ("before_rebuild", "after_rebuild", "before_restart", "after_restart"),
    )
    inventory = json.loads(collect_canonical_inventory(vault))
    envelope = {
        "schema": "plico.p3a.canonical-checkpoint/v1",
        "bundle_run_id": bundle_run_id,
        "phase": phase,
        "daemon_instance_id": daemon_instance_id,
        "sequence": sequence,
        "entries": inventory["entries"],
    }
    write_private_exclusive(output, canonical_json(envelope))


def collect_v1_zero_state_checkpoint(
    *,
    vault: Path,
    output: Path,
    bundle_run_id: str,
    phase: str,
    daemon_instance_id: str,
    sequence: int,
) -> None:
    """Capture canonical and projection trees around one rejected v1 envelope."""
    _validate_checkpoint_identity(
        vault,
        output,
        bundle_run_id,
        daemon_instance_id,
        phase,
        sequence,
        ("before_v1_reject", "after_v1_reject"),
    )
    canonical = json.loads(collect_canonical_inventory(vault))
    projection = json.loads(collect_projection_inventory(vault))
    envelope = {
        "schema": "plico.p3a.v1-zero-state-checkpoint/v1",
        "bundle_run_id": bundle_run_id,
        "phase": phase,
        "daemon_instance_id": daemon_instance_id,
        "sequence": sequence,
        "canonical_entries": canonical["entries"],
        "projection_entries": projection["entries"],
    }
    write_private_exclusive(output, canonical_json(envelope))


def collect_ollama_probe(
    *,
    base_url: str,
    configured_tag: str,
    output: Path,
    requested_target_dimension: int | None,
    adaptive_prefix_contract_id: str,
) -> None:
    """Probe exact Ollama identity and shape without persisting bodies or endpoints."""
    safe_label(configured_tag)
    if ":" not in configured_tag:
        raise ValueError("Ollama probe requires an explicit full model tag")
    if adaptive_prefix_contract_id not in {
        "provider-native-input-v1",
        "qwen3-web-search-query-document-native-v1",
    }:
        raise ValueError("Ollama probe has an unsupported adaptive prefix contract")
    base = base_url.rstrip("/")
    if not base.startswith(("http://", "https://")):
        raise ValueError("Ollama probe URL must use HTTP or HTTPS")
    before = _exact_model(_get_json(f"{base}/api/tags"), configured_tag)
    version_before = _get_json(f"{base}/api/version")
    if set(version_before) != {"version"} or not isinstance(version_before["version"], str):
        raise ValueError("Ollama version response is invalid")
    response = _post_json(
        f"{base}/api/embed",
        {
            "model": configured_tag,
            "input": "plico document identity probe v1",
            "truncate": False,
        },
    )
    allowed_response = {
        "model",
        "embeddings",
        "prompt_eval_count",
        "total_duration",
        "load_duration",
    }
    if not set(response).issubset(allowed_response) or not {"model", "embeddings"}.issubset(
        response
    ):
        raise ValueError("Ollama embedding response has unexpected fields")
    for name, maximum in (
        ("prompt_eval_count", 2**32 - 1),
        ("total_duration", 2**64 - 1),
        ("load_duration", 2**64 - 1),
    ):
        value = response.get(name, 0)
        if not isinstance(value, int) or isinstance(value, bool) or not 0 <= value <= maximum:
            raise ValueError("Ollama embedding timing metadata is outside its Rust type")
    if response.get("model") != configured_tag:
        raise ValueError("Ollama embedding response model differs from the configured exact tag")
    embeddings = response.get("embeddings")
    if (
        not isinstance(embeddings, list)
        or len(embeddings) != 1
        or not isinstance(embeddings[0], list)
        or not embeddings[0]
    ):
        raise ValueError("Ollama embedding probe shape is invalid")
    vector = embeddings[0]
    if any(not isinstance(value, (int, float)) or isinstance(value, bool) for value in vector):
        raise ValueError("Ollama embedding probe contains a nonnumeric component")
    numeric = []
    for value in vector:
        try:
            as_f32 = struct.unpack("!f", struct.pack("!f", float(value)))[0]
        except (OverflowError, struct.error) as error:
            raise ValueError("Ollama embedding component is not representable as f32") from error
        numeric.append(as_f32)
    if any(not math.isfinite(value) for value in numeric) or all(value == 0.0 for value in numeric):
        raise ValueError("Ollama embedding probe is not finite and nonzero")
    raw_dimension = len(numeric)
    if raw_dimension > 65_536:
        raise ValueError("Ollama embedding dimension exceeds the Rust limit")
    if requested_target_dimension is not None and not (
        0 < requested_target_dimension <= raw_dimension
    ):
        raise ValueError("Ollama target dimension is outside the provider shape")
    effective_dimension = requested_target_dimension or raw_dimension
    effective = numeric[:effective_dimension]
    normalization = "provider_native"
    transform = "provider-native-document-v1"
    if effective_dimension < raw_dimension:
        magnitude = math.sqrt(sum(value * value for value in effective))
        if not math.isfinite(magnitude) or magnitude == 0.0:
            raise ValueError("Ollama truncated probe vector cannot be normalized")
        effective = [value / magnitude for value in effective]
        normalization = "l2_after_matryoshka_truncation_v1"
        transform = "plico-matryoshka-truncate-l2-v1"
    effective_norm = math.sqrt(sum(value * value for value in effective))
    after = _exact_model(_get_json(f"{base}/api/tags"), configured_tag)
    version_after = _get_json(f"{base}/api/version")
    if set(version_after) != {"version"} or not isinstance(version_after["version"], str):
        raise ValueError("Ollama version response is invalid")
    if before != after or version_before != version_after:
        raise ValueError("Ollama provider evidence changed during the probe")
    artifact = {
        "schema": "plico.p3a.ollama-probe/v1",
        "configured_exact_tag": configured_tag,
        "exact_tag_match_count": 1,
        "model_digest_before": before["digest"],
        "model_digest_after": after["digest"],
        "server_version": version_before["version"],
        "api_contract": "ollama-api-embed-truncate-false/v1",
        "raw_dimension": raw_dimension,
        "effective_dimension": effective_dimension,
        "requested_target_dimension": requested_target_dimension,
        "adaptive_prefix_contract_id": adaptive_prefix_contract_id,
        "normalization": normalization,
        "transform_contract_id": transform,
        "probe_vector_count": 1,
        "probe_dimension": raw_dimension,
        "probe_all_finite": True,
        "probe_nonzero": True,
        "probe_l2_norm": effective_norm,
    }
    write_private_exclusive(output, canonical_json(artifact))


def _exact_model(value: dict[str, Any], configured_tag: str) -> dict[str, str]:
    if set(value) != {"models"} or not isinstance(value["models"], list):
        raise ValueError("Ollama tags response is invalid")
    matches = [
        item
        for item in value["models"]
        if isinstance(item, dict) and item.get("name") == configured_tag
    ]
    if len(matches) != 1:
        raise ValueError("Ollama configured model tag is absent or ambiguous")
    digest = matches[0].get("digest")
    if not isinstance(digest, str):
        raise ValueError("Ollama configured model digest is missing")
    sha256(digest)
    return {"name": configured_tag, "digest": digest}


def _get_json(url: str) -> dict[str, Any]:
    request = urllib.request.Request(url, method="GET")
    return _request_json(request)


def _post_json(url: str, value: dict[str, Any]) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        data=json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode(),
        headers={"content-type": "application/json"},
        method="POST",
    )
    return _request_json(request)


def _request_json(request: urllib.request.Request) -> dict[str, Any]:
    try:
        with urllib.request.urlopen(request, timeout=15) as response:
            if response.status != 200:
                raise ValueError("Ollama collector received a non-success status")
            payload = response.read(16 * 1024 * 1024 + 1)
    except (OSError, urllib.error.URLError) as error:
        raise ValueError("Ollama collector request failed") from error
    if len(payload) > 16 * 1024 * 1024:
        raise ValueError("Ollama collector response exceeded its byte limit")
    value = json.loads(payload)
    if not isinstance(value, dict):
        raise ValueError("Ollama collector response is not a JSON object")
    return value


def _validate_checkpoint_identity(
    vault: Path,
    output: Path,
    run_id: str,
    daemon_id: str,
    phase: str,
    sequence: int,
    phases: tuple[str, ...],
) -> None:
    canonical_uuid(run_id)
    canonical_uuid(daemon_id)
    if phase not in phases or sequence != phases.index(phase) + 1:
        raise ValueError("checkpoint phase and sequence are invalid")
    vault_path = os.path.abspath(vault)
    output_path = os.path.abspath(output)
    if os.path.commonpath((vault_path, output_path)) in {vault_path, output_path}:
        raise ValueError("checkpoint output must not overlap the live vault")
