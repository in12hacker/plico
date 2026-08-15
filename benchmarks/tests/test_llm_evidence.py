"""Content-free LLM identity replay invariants."""

from __future__ import annotations

import pytest

from plico_benchmarks.core.llm_evidence import (
    summarize_llm_costs,
    summarize_llm_identity,
)


def _attempt(**changes: object) -> dict[str, object]:
    record: dict[str, object] = {
        "role": "reader",
        "status": "ok",
        "requested_model_alias": "deepseek-v4-flash",
        "official_model_version": None,
        "model_revision_attestation": "unattested_alias",
        "response_model": "deepseek-v4-flash",
        "system_fingerprint": "fp-safe-1",
        "cross_run_comparability": ("requires_same_system_fingerprint_and_five_run_variance_ci"),
        "thinking": "disabled",
        "reasoning_effort": None,
        "temperature": 0.0,
        "top_p": 1.0,
        "generation_seed": "provider_unavailable",
        "usd_accounted": "0.001",
    }
    record.update(changes)
    return record


def test_alias_identity_is_integrity_verified_but_not_cross_run_attested() -> None:
    identity = summarize_llm_identity([_attempt(), _attempt()])

    assert identity["status"] == "verified_attempt_integrity_not_cross_run_comparability"
    assert identity["roles"]["reader"]["identity_class"] == (
        "unattested_alias_requires_same_fingerprint_and_five_runs"
    )
    assert summarize_llm_costs([_attempt(), _attempt()])["total_usd"] == "0.002"


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("system_fingerprint", "fp-safe-2"),
        ("response_model", "deepseek-v4-pro"),
        ("requested_model_alias", "deepseek-v4-pro"),
    ],
)
def test_mixed_success_identity_or_role_configuration_is_rejected(field: str, value: str) -> None:
    with pytest.raises(ValueError, match="mixed|inconsistent"):
        summarize_llm_identity([_attempt(), _attempt(**{field: value})])


def test_failed_attempt_without_response_cannot_upgrade_or_break_exact_identity() -> None:
    failed = _attempt(
        status="remote_error",
        official_model_version=None,
        model_revision_attestation="unattested_no_response",
        response_model=None,
        system_fingerprint=None,
        cross_run_comparability="not_comparable_no_response",
    )

    identity = summarize_llm_identity([failed, _attempt()])

    assert identity["roles"]["reader"]["response_model"] == "deepseek-v4-flash"


def test_role_with_no_successful_attempt_has_no_identity() -> None:
    with pytest.raises(ValueError, match="no successful"):
        summarize_llm_identity(
            [
                _attempt(
                    status="timeout",
                    model_revision_attestation="unattested_no_response",
                    response_model=None,
                    system_fingerprint=None,
                    cross_run_comparability="not_comparable_no_response",
                )
            ]
        )
