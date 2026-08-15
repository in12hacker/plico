"""Replayable summaries for content-free DeepSeek attempt evidence."""

from __future__ import annotations

from decimal import Decimal
from typing import Any, Iterable


def summarize_llm_identity(records: Iterable[dict[str, Any]]) -> dict[str, Any]:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for record in records:
        role = record.get("role")
        if not isinstance(role, str) or not role:
            raise ValueError("LLM evidence role is invalid")
        grouped.setdefault(role, []).append(record)
    if not grouped:
        raise ValueError("LLM identity requires at least one finalized attempt")

    roles = {}
    for role, attempts in sorted(grouped.items()):
        requested_model = _single(attempts, "requested_model_alias", allow_none=False)
        thinking = _single(attempts, "thinking", allow_none=False)
        reasoning_effort = _single(attempts, "reasoning_effort", allow_none=True)
        temperature = _single(attempts, "temperature", allow_none=True)
        top_p = _single(attempts, "top_p", allow_none=True)
        generation_seed = _single(attempts, "generation_seed", allow_none=False)
        successful = [attempt for attempt in attempts if attempt.get("status") == "ok"]
        if not successful:
            raise ValueError("LLM role has no successful identity-bearing attempt")
        attestation = _single(successful, "model_revision_attestation", allow_none=False)
        response_model = _single(successful, "response_model", allow_none=False)
        official_version = _single(successful, "official_model_version", allow_none=True)
        fingerprint = _single(successful, "system_fingerprint", allow_none=True)
        comparison = _single(successful, "cross_run_comparability", allow_none=False)
        if attestation == "attested_exact_version":
            if (
                not isinstance(official_version, str)
                or response_model != official_version
                or comparison != "requires_five_run_variance_ci"
            ):
                raise ValueError("attested LLM revision evidence is inconsistent")
            identity_class = "attested_exact_version_requires_five_runs"
        elif attestation == "unattested_alias":
            if (
                official_version is not None
                or response_model != requested_model
                or not isinstance(fingerprint, str)
                or not fingerprint
                or comparison != "requires_same_system_fingerprint_and_five_run_variance_ci"
            ):
                raise ValueError("alias LLM identity evidence is inconsistent")
            identity_class = "unattested_alias_requires_same_fingerprint_and_five_runs"
        else:
            raise ValueError("successful LLM attempt has no supported model attestation")
        roles[role] = {
            "provider": "deepseek",
            "api_origin": "https://api.deepseek.com",
            "requested_model_alias": requested_model,
            "official_model_version": official_version,
            "model_revision_attestation": attestation,
            "response_model": response_model,
            "system_fingerprint": fingerprint,
            "identity_class": identity_class,
            "cross_run_comparability": comparison,
            "thinking": thinking,
            "reasoning_effort": reasoning_effort,
            "temperature": temperature,
            "top_p": top_p,
            "generation_seed": generation_seed,
        }
    return {
        "status": "verified_attempt_integrity_not_cross_run_comparability",
        "roles": roles,
    }


def summarize_llm_costs(records: Iterable[dict[str, Any]]) -> dict[str, Any]:
    materialized = list(records)
    roles: dict[str, Decimal] = {}
    for record in materialized:
        role = record.get("role")
        if not isinstance(role, str) or not role:
            raise ValueError("LLM cost evidence role is invalid")
        roles[role] = roles.get(role, Decimal(0)) + Decimal(record["usd_accounted"])
    return {
        "currency": "USD",
        "accounting": "per_attempt_recomputed_and_budget_reconciled",
        "total_usd": format(sum(roles.values(), Decimal(0)), "f"),
        "by_role_usd": {role: format(value, "f") for role, value in sorted(roles.items())},
        "attempt_count": len(materialized),
    }


def _single(records: list[dict[str, Any]], field: str, *, allow_none: bool) -> Any:
    values = {record.get(field) for record in records}
    if len(values) != 1:
        raise ValueError(f"LLM evidence has mixed {field}")
    value = next(iter(values))
    if value is None and not allow_none:
        raise ValueError(f"LLM evidence has no {field}")
    return value
