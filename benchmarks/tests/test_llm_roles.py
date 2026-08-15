from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import textwrap
import threading
import uuid
from dataclasses import replace
from datetime import UTC, datetime
from decimal import Decimal

import pytest

from plico_benchmarks.core.dogfood_io import canonical_json
from plico_benchmarks.core.judge import Judge
from plico_benchmarks.core.llm import (
    DeepSeekLlm,
    LlmBudgetError,
    LlmProtocolError,
    LlmRemoteError,
)
from plico_benchmarks.core.llm_journal import (
    JOURNAL_DIR_ENV,
    RUN_ID_ENV,
    LlmJournalError,
    mark_attempt_journal_complete,
    read_attempt_journal,
)
from plico_benchmarks.core.llm_roles import (
    DEEPSEEK_API_BASE,
    LlmConfigurationError,
    LlmPricingError,
    LlmRole,
    LlmRoleConfig,
    RoleBudget,
    select_deepseek_price,
)

OLD_PRICE_INSTANT = datetime(2026, 8, 15, 0, 0, tzinfo=UTC)


@pytest.fixture(autouse=True)
def durable_attempt_journal(tmp_path, monkeypatch):
    directory = tmp_path / "llm-attempt-journal"
    directory.mkdir(mode=0o700)
    run_id = str(uuid.uuid4())
    monkeypatch.setenv(RUN_ID_ENV, run_id)
    monkeypatch.setenv(JOURNAL_DIR_ENV, str(directory))
    return directory, run_id


class FakeResponse:
    def __init__(
        self,
        payload: object,
        status_code: int = 200,
        headers: dict[str, str] | None = None,
    ):
        self.status_code = status_code
        self.payload = payload
        self.headers = {} if headers is None else headers

    def json(self) -> object:
        return self.payload


def role_env(role: str = "READER") -> dict[str, str]:
    prefix = f"PLICO_{role}"
    return {
        f"{prefix}_PROVIDER": "deepseek",
        f"{prefix}_API_BASE": DEEPSEEK_API_BASE,
        f"{prefix}_MODEL": "deepseek-v4-flash",
        f"{prefix}_API_KEY": "test-key-never-log",
        f"{prefix}_TIMEOUT_SECONDS": "7.5",
        f"{prefix}_MAX_TOKENS": "64",
        f"{prefix}_MAX_ATTEMPTS": "2",
        f"{prefix}_THINKING": "disabled",
        f"{prefix}_REASONING_EFFORT": "none",
        f"{prefix}_TEMPERATURE": "0",
        f"{prefix}_TOP_P": "1",
        f"{prefix}_MAX_REQUESTS": "4",
        f"{prefix}_MAX_INPUT_TOKENS": "10000",
        f"{prefix}_MAX_OUTPUT_TOKENS": "256",
        f"{prefix}_MAX_USD": "1.25",
    }


def config(
    role: LlmRole = LlmRole.READER,
    *,
    max_requests: int = 4,
    max_attempts: int = 2,
    max_usd: str = "1.25",
) -> LlmRoleConfig:
    return LlmRoleConfig(
        role=role,
        provider="deepseek",
        api_base=DEEPSEEK_API_BASE,
        model="deepseek-v4-flash",
        api_key="test-key-never-log",
        timeout_seconds=7.5,
        max_tokens=64,
        max_attempts=max_attempts,
        thinking="disabled",
        reasoning_effort=None,
        temperature=0.0,
        top_p=1.0,
        budget=RoleBudget(
            max_requests=max_requests,
            max_input_tokens=10000,
            max_output_tokens=256,
            max_usd=Decimal(max_usd),
        ),
    )


def success_payload(content: str = "correct") -> dict[str, object]:
    return {
        "model": "DeepSeek-V4-Flash-0731",
        "system_fingerprint": "fp_v4_flash_0731",
        "choices": [
            {
                "finish_reason": "stop",
                "message": {"role": "assistant", "content": content},
            }
        ],
        "usage": {
            "prompt_tokens": 10,
            "prompt_cache_hit_tokens": 4,
            "prompt_cache_miss_tokens": 6,
            "completion_tokens": 2,
            "total_tokens": 12,
        },
    }


def test_role_config_is_exact_and_does_not_consume_legacy_fallbacks() -> None:
    legacy = {
        "LLM_BACKEND": "forged-backend",
        "OPENAI_API_BASE": "https://fallback.invalid/v1",
        "OPENAI_API_KEY": "legacy-secret",
        "LLM_MODEL": "fallback-model",
    }
    with pytest.raises(LlmConfigurationError, match="PLICO_READER_PROVIDER"):
        LlmRoleConfig.from_env(LlmRole.READER, legacy)

    env = role_env()
    parsed = LlmRoleConfig.from_env(LlmRole.READER, env)
    assert parsed.provider == "deepseek"
    assert parsed.api_base == DEEPSEEK_API_BASE
    assert parsed.model == "deepseek-v4-flash"
    assert parsed.generation_seed == "provider_unavailable"
    assert "test-key-never-log" not in repr(parsed)
    with pytest.raises(LlmConfigurationError, match="PLICO_JUDGE_PROVIDER"):
        LlmRoleConfig.from_env(LlmRole.JUDGE, env)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("PROVIDER", "openai"),
        ("API_BASE", "https://api.deepseek.com/v1"),
        ("MODEL", "deepseek-chat"),
    ],
)
def test_role_config_rejects_noncanonical_provider_base_and_model(field: str, value: str) -> None:
    env = role_env()
    env[f"PLICO_READER_{field}"] = value
    with pytest.raises(LlmConfigurationError):
        LlmRoleConfig.from_env(LlmRole.READER, env)


@pytest.mark.parametrize(
    ("instant", "schedule", "band", "flash_miss", "pro_output"),
    [
        (
            datetime(2026, 8, 16, 15, 59, 59, tzinfo=UTC),
            "deepseek-v4-0731-usd-2026-07-31",
            "standard",
            "0.14",
            "0.87",
        ),
        (
            datetime(2026, 8, 16, 16, 0, tzinfo=UTC),
            "deepseek-v4-usd-2026-08-16",
            "off_peak",
            "0.22",
            "1.98",
        ),
        (
            datetime(2026, 8, 17, 1, 0, tzinfo=UTC),
            "deepseek-v4-usd-2026-08-16",
            "peak",
            "0.44",
            "3.96",
        ),
        (
            datetime(2026, 8, 17, 4, 0, tzinfo=UTC),
            "deepseek-v4-usd-2026-08-16",
            "off_peak",
            "0.22",
            "1.98",
        ),
        (
            datetime(2026, 8, 17, 6, 0, tzinfo=UTC),
            "deepseek-v4-usd-2026-08-16",
            "peak",
            "0.44",
            "3.96",
        ),
        (
            datetime(2026, 8, 17, 10, 0, tzinfo=UTC),
            "deepseek-v4-usd-2026-08-16",
            "off_peak",
            "0.22",
            "1.98",
        ),
    ],
)
def test_price_schedule_uses_utc_effective_and_peak_boundaries(
    instant: datetime,
    schedule: str,
    band: str,
    flash_miss: str,
    pro_output: str,
) -> None:
    flash = select_deepseek_price("deepseek-v4-flash", instant)
    pro = select_deepseek_price("deepseek-v4-pro", instant)
    assert (flash.pricing_schedule_id, flash.billing_band) == (schedule, band)
    assert flash.prices.cache_miss_per_million == Decimal(flash_miss)
    assert pro.prices.output_per_million == Decimal(pro_output)
    assert flash.source_url == "https://api-docs.deepseek.com/quick_start/pricing/"
    assert flash.source_retrieved_at.endswith("Z")
    assert flash.source_reviewed_at.endswith("Z")
    assert len(flash.local_frozen_schedule_record_sha256) == 64
    frozen_record = json.dumps(
        {
            "billing_band": flash.billing_band,
            "cache_hit_per_million_usd": str(flash.prices.cache_hit_per_million),
            "cache_miss_per_million_usd": str(flash.prices.cache_miss_per_million),
            "effective_at": flash.effective_at,
            "model": "deepseek-v4-flash",
            "output_per_million_usd": str(flash.prices.output_per_million),
            "schedule_id": flash.pricing_schedule_id,
            "source_retrieved_at": flash.source_retrieved_at,
            "source_reviewed_at": flash.source_reviewed_at,
            "source_url": flash.source_url,
            "review_not_after": flash.review_not_after,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    assert flash.local_frozen_schedule_record_sha256 == hashlib.sha256(frozen_record).hexdigest()


@pytest.mark.parametrize(
    "instant",
    [
        datetime(2026, 7, 30, 23, 59, 59, tzinfo=UTC),
        datetime(2026, 9, 15, 16, 0, tzinfo=UTC),
    ],
)
def test_price_schedule_fails_closed_outside_reviewed_validity(instant: datetime) -> None:
    with pytest.raises(LlmPricingError):
        select_deepseek_price("deepseek-v4-flash", instant)


def test_success_records_content_free_attempt_and_exact_usage_cost() -> None:
    calls: list[dict[str, object]] = []

    def transport(url: str, **kwargs: object) -> FakeResponse:
        calls.append({"url": url, **kwargs})
        return FakeResponse(success_payload("response-secret-body"))

    llm = DeepSeekLlm(config=config(), transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    messages = [{"role": "user", "content": "prompt-secret /private/vault"}]
    assert llm.chat(messages, max_tokens=16) == "response-secret-body"
    assert calls[0]["url"] == "https://api.deepseek.com/chat/completions"
    assert calls[0]["timeout"] == 7.5

    evidence = llm.attempts()[0]
    assert evidence.status == "ok"
    assert evidence.requested_model_alias == "deepseek-v4-flash"
    assert evidence.official_model_version == "DeepSeek-V4-Flash-0731"
    assert evidence.model_revision_attestation == "attested_exact_version"
    assert evidence.response_model == "DeepSeek-V4-Flash-0731"
    assert evidence.system_fingerprint == "fp_v4_flash_0731"
    assert evidence.finish_reason == "stop"
    assert evidence.thinking == "disabled"
    assert evidence.reasoning_effort is None
    assert evidence.temperature == 0.0
    assert evidence.top_p == 1.0
    assert evidence.max_tokens == 16
    assert evidence.generation_seed == "provider_unavailable"
    assert evidence.started_at_utc == "2026-08-15T00:00:00.000000Z"
    assert evidence.usage is not None
    assert evidence.usage.cache_accounting == "provider_reported"
    expected_usd = (
        Decimal(4) * Decimal("0.0028") + Decimal(6) * Decimal("0.14") + Decimal(2) * Decimal("0.28")
    ) / Decimal(1_000_000)
    assert Decimal(evidence.usd_accounted) == expected_usd
    recomputed_usd = (
        Decimal(evidence.usage.prompt_cache_hit_tokens)
        * Decimal(evidence.pricing_cache_hit_per_million_usd)
        + Decimal(evidence.usage.prompt_cache_miss_tokens)
        * Decimal(evidence.pricing_cache_miss_per_million_usd)
        + Decimal(evidence.usage.completion_tokens)
        * Decimal(evidence.pricing_output_per_million_usd)
    ) / Decimal(1_000_000)
    assert recomputed_usd == Decimal(evidence.usd_accounted)
    canonical = json.dumps(
        [{"content": "prompt-secret /private/vault", "role": "user"}],
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    assert evidence.prompt_sha256 == hashlib.sha256(canonical).hexdigest()

    encoded = json.dumps(evidence.to_dict(), sort_keys=True)
    for forbidden in (
        "test-key-never-log",
        "prompt-secret",
        "response-secret-body",
        "/private/vault",
        "chat/completions",
    ):
        assert forbidden not in encoded


def test_missing_cache_split_is_costed_as_all_cache_miss() -> None:
    payload = success_payload()
    usage = payload["usage"]
    assert isinstance(usage, dict)
    del usage["prompt_cache_hit_tokens"]
    del usage["prompt_cache_miss_tokens"]
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(payload),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    evidence = llm.attempts()[0]
    assert evidence.usage is not None
    assert evidence.usage.cache_accounting == "all_miss_conservative"
    assert evidence.usage.prompt_cache_miss_tokens == 10


def test_response_alias_does_not_claim_an_attested_official_revision() -> None:
    payload = success_payload()
    payload["model"] = "deepseek-v4-flash"
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(payload),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    evidence = llm.attempts()[0]
    assert evidence.response_model == "deepseek-v4-flash"
    assert evidence.official_model_version is None
    assert evidence.model_revision_attestation == "unattested_alias"
    assert evidence.cross_run_comparability == (
        "requires_same_system_fingerprint_and_five_run_variance_ci"
    )


def test_response_model_mismatch_is_finalized_as_nonretryable_protocol_evidence(
    durable_attempt_journal,
) -> None:
    directory, run_id = durable_attempt_journal
    payload = success_payload()
    payload["model"] = "safe-observed-mismatch"
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(payload)

    llm = DeepSeekLlm(
        config=config(max_attempts=2), transport=transport, clock=lambda: OLD_PRICE_INSTANT
    )
    with pytest.raises(LlmProtocolError, match="does not match"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert calls == 1
    evidence = llm.attempts()[0]
    assert evidence.status == "protocol_error"
    assert evidence.response_model == "safe-observed-mismatch"
    assert evidence.official_model_version is None
    assert evidence.model_revision_attestation == "unattested_mismatch"
    assert evidence.cross_run_comparability == "not_comparable_model_mismatch"
    snapshot = read_attempt_journal(directory, run_id)
    assert snapshot.finalized_attempt_count == 1
    assert snapshot.incomplete_prepared_attempts == 0


def test_generation_seed_is_not_fabricated_or_sent_to_deepseek() -> None:
    sent: dict[str, object] = {}

    def transport(_url: str, **kwargs: object) -> FakeResponse:
        sent.update(kwargs)
        return FakeResponse(success_payload())

    llm = DeepSeekLlm(config=config(), transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    payload = sent["json"]
    assert isinstance(payload, dict)
    assert "seed" not in payload
    evidence = llm.attempts()[0]
    assert evidence.generation_seed == "provider_unavailable"
    assert evidence.cross_run_comparability == "requires_five_run_variance_ci"


def test_transport_failure_is_indeterminate_costed_and_not_retried() -> None:
    responses: list[object] = [RuntimeError("secret endpoint failure"), success_payload()]

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        next_response = responses.pop(0)
        if isinstance(next_response, Exception):
            raise next_response
        return FakeResponse(next_response)

    llm = DeepSeekLlm(
        config=config(role=LlmRole.JUDGE, max_attempts=2),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(RuntimeError, match="judge evaluation failed"):
        Judge(llm=llm, max_tokens=16).evaluate("question", "answer", "answer")
    evidence = llm.attempts()
    assert [item.status for item in evidence] == ["indeterminate_transport"]
    assert evidence[0].usd_basis == "reserved_upper_bound"
    reserved_recomputed = (
        Decimal(evidence[0].reserved_input_tokens_upper_bound)
        * Decimal(evidence[0].reservation_cache_miss_per_million_usd)
        + Decimal(evidence[0].reserved_output_tokens)
        * Decimal(evidence[0].reservation_output_per_million_usd)
    ) / Decimal(1_000_000)
    assert reserved_recomputed == Decimal(evidence[0].usd_accounted)
    assert evidence[0].reservation_pricing_schedule_id
    assert evidence[0].reservation_billing_band
    assert evidence[0].pricing_cache_hit_per_million_usd
    assert evidence[0].pricing_cache_miss_per_million_usd
    assert evidence[0].pricing_output_per_million_usd
    assert len(responses) == 1
    assert Decimal(llm.budget_snapshot().usd_accounted) == sum(
        (Decimal(item.usd_accounted) for item in evidence), Decimal(0)
    )


def test_retryable_http_attempt_preserves_first_usage_cost() -> None:
    retryable = success_payload("retryable")
    valid = success_payload("5")
    responses = [FakeResponse(retryable, status_code=503), FakeResponse(valid)]
    sleeps: list[float] = []
    llm = DeepSeekLlm(
        config=config(role=LlmRole.JUDGE, max_attempts=2),
        transport=lambda *_args, **_kwargs: responses.pop(0),
        clock=lambda: OLD_PRICE_INSTANT,
        sleeper=sleeps.append,
    )
    score, _ = Judge(llm=llm, max_tokens=4).evaluate_scored("q", "a", "a")
    assert score == 5
    assert len(llm.attempts()) == 2
    assert [item.status for item in llm.attempts()] == ["http_error", "ok"]
    assert len({item.role_request_id for item in llm.attempts()}) == 1
    assert [item.attempt_in_request for item in llm.attempts()] == [1, 2]
    assert sleeps == [0.25]
    assert Decimal(llm.budget_snapshot().usd_accounted) == sum(
        (Decimal(item.usd_accounted) for item in llm.attempts()), Decimal(0)
    )


def test_retry_after_is_safely_parsed_capped_and_does_not_retry_transport_errors() -> None:
    sleeps: list[float] = []
    responses = [
        FakeResponse(success_payload(), status_code=429, headers={"Retry-After": "999"}),
        FakeResponse(success_payload()),
    ]
    llm = DeepSeekLlm(
        config=config(max_attempts=2),
        transport=lambda *_args, **_kwargs: responses.pop(0),
        clock=lambda: OLD_PRICE_INSTANT,
        sleeper=sleeps.append,
    )
    assert llm.chat([{"role": "user", "content": "q"}], max_tokens=4) == "correct"
    assert sleeps == [5.0]
    assert [item.status for item in llm.attempts()] == ["http_error", "ok"]


def test_request_budget_rejects_before_second_network_attempt() -> None:
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload())

    llm = DeepSeekLlm(
        config=config(max_requests=1, max_attempts=1),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat([{"role": "user", "content": "first"}], max_tokens=4)
    with pytest.raises(LlmBudgetError, match="request budget"):
        llm.chat([{"role": "user", "content": "second"}], max_tokens=4)
    assert calls == 1
    assert len(llm.attempts()) == 1


def test_http_failure_records_status_without_body_or_endpoint() -> None:
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(
            {"error": "server-secret /private/path"}, status_code=503
        ),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmRemoteError, match="status 503"):
        llm.chat([{"role": "user", "content": "prompt-secret"}], max_tokens=4)
    encoded = json.dumps(llm.attempts()[0].to_dict(), sort_keys=True)
    assert llm.attempts()[0].status == "http_error"
    assert llm.attempts()[0].usd_basis == "reserved_upper_bound"
    for forbidden in ("server-secret", "/private/path", "prompt-secret", "chat/completions"):
        assert forbidden not in encoded


def test_unsafe_response_evidence_is_rejected_without_recording_it() -> None:
    payload = success_payload()
    payload["system_fingerprint"] = "fp\nsecret-sentinel"
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(payload),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmProtocolError, match="system fingerprint"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    encoded = json.dumps(llm.attempts()[0].to_dict(), sort_keys=True)
    assert llm.attempts()[0].status == "protocol_error"
    assert "secret-sentinel" not in encoded


@pytest.mark.parametrize("status_code", [400, 401, 403, 404])
def test_nonretryable_http_status_is_attempted_once(status_code: int) -> None:
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse({"error": "do-not-record"}, status_code=status_code)

    llm = DeepSeekLlm(
        config=config(max_attempts=2), transport=transport, clock=lambda: OLD_PRICE_INSTANT
    )
    with pytest.raises(LlmRemoteError):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert calls == 1
    assert [item.status for item in llm.attempts()] == ["http_error"]


def test_non_stop_finish_is_paid_but_not_accepted_or_retried() -> None:
    payload = success_payload("partial answer")
    choices = payload["choices"]
    assert isinstance(choices, list)
    assert isinstance(choices[0], dict)
    choices[0]["finish_reason"] = "length"
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(payload)

    llm = DeepSeekLlm(
        config=config(max_attempts=2), transport=transport, clock=lambda: OLD_PRICE_INSTANT
    )
    with pytest.raises(LlmProtocolError, match="did not finish normally"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert calls == 1
    evidence = llm.attempts()[0]
    assert evidence.status == "incomplete"
    assert evidence.finish_reason == "length"
    assert evidence.usd_basis == "actual_usage"


def test_usage_over_reservation_always_records_accounting_error() -> None:
    payload = success_payload()
    usage = payload["usage"]
    assert isinstance(usage, dict)
    usage.update(
        {
            "prompt_tokens": 100000,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 100000,
            "completion_tokens": 2,
            "total_tokens": 100002,
        }
    )
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(payload),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmProtocolError, match="reserved upper bound"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    evidence = llm.attempts()[0]
    assert evidence.status == "accounting_error"
    assert evidence.usd_basis == "reserved_upper_bound"
    assert llm.budget_snapshot().requests == 1


def test_thinking_mode_is_explicit_and_omits_ignored_sampling_parameters() -> None:
    sent: dict[str, object] = {}

    def transport(_url: str, **kwargs: object) -> FakeResponse:
        sent.update(kwargs)
        return FakeResponse(success_payload())

    thinking_config = replace(
        config(),
        thinking="enabled",
        reasoning_effort="max",
        temperature=None,
        top_p=None,
    )
    llm = DeepSeekLlm(config=thinking_config, transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    payload = sent["json"]
    assert isinstance(payload, dict)
    assert payload["thinking"] == {"type": "enabled"}
    assert payload["reasoning_effort"] == "max"
    assert "temperature" not in payload
    assert "top_p" not in payload
    evidence = llm.attempts()[0]
    assert evidence.thinking == "enabled"
    assert evidence.reasoning_effort == "max"
    assert evidence.temperature is None
    assert evidence.top_p is None


def test_usd_budget_rejects_before_network_io() -> None:
    called = False

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal called
        called = True
        return FakeResponse(success_payload())

    llm = DeepSeekLlm(
        config=config(max_usd="0.0000000001"),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmBudgetError, match="USD budget"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=64)
    assert not called
    assert llm.attempts() == ()


def test_evidence_since_can_isolate_one_role_request() -> None:
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat(
        [{"role": "user", "content": "one"}],
        max_tokens=4,
        request_id="request-one",
        sample_id="sample-one",
    )
    watermark = llm.attempts()[-1].attempt_sequence
    llm.chat(
        [{"role": "user", "content": "two"}],
        max_tokens=4,
        request_id="request-two",
        sample_id="sample-two",
    )
    assert llm.evidence_since(watermark, role_request_id="request-one") == ()
    isolated = llm.evidence_since(watermark, role_request_id="request-two")
    assert len(isolated) == 1
    assert isolated[0].sample_id == "sample-two"


@pytest.mark.parametrize(
    (
        "started",
        "completed",
        "expected_band",
        "expected_hit",
        "expected_miss",
        "expected_output",
    ),
    [
        (
            datetime(2026, 8, 17, 0, 59, 59, tzinfo=UTC),
            datetime(2026, 8, 17, 1, 0, 1, tzinfo=UTC),
            "max_of[off_peak,peak]",
            Decimal("0.014"),
            Decimal("0.44"),
            Decimal("1.32"),
        ),
        (
            datetime(2026, 8, 16, 15, 59, 59, tzinfo=UTC),
            datetime(2026, 8, 16, 16, 0, 1, tzinfo=UTC),
            "max_of[off_peak,standard]",
            Decimal("0.007"),
            Decimal("0.22"),
            Decimal("0.66"),
        ),
    ],
)
def test_attempt_crossing_price_boundary_uses_highest_involved_rates(
    started: datetime,
    completed: datetime,
    expected_band: str,
    expected_hit: Decimal,
    expected_miss: Decimal,
    expected_output: Decimal,
) -> None:
    instants = iter([started, completed])
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: next(instants),
    )
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    evidence = llm.attempts()[0]
    assert evidence.billing_band == expected_band
    assert evidence.started_at_utc == started.isoformat(timespec="microseconds").replace(
        "+00:00", "Z"
    )
    expected_usd = (
        Decimal(4) * expected_hit + Decimal(6) * expected_miss + Decimal(2) * expected_output
    ) / Decimal(1_000_000)
    assert Decimal(evidence.usd_accounted) == expected_usd
    assert evidence.usd_basis == "actual_usage"


def test_request_window_crossing_price_review_horizon_fails_before_io() -> None:
    called = False

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal called
        called = True
        return FakeResponse(success_payload())

    llm = DeepSeekLlm(
        config=config(),
        transport=transport,
        clock=lambda: datetime(2026, 9, 15, 15, 59, 59, tzinfo=UTC),
    )
    with pytest.raises(LlmPricingError, match="stale"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert not called
    assert llm.attempts() == ()
    assert llm.budget_snapshot().requests == 0


@pytest.mark.parametrize("raw", ["15", "score=5", "5 points", "05"])
def test_scored_judge_rejects_noncanonical_scores_without_retry(raw: str) -> None:
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload(raw))

    llm = DeepSeekLlm(
        config=config(role=LlmRole.JUDGE, max_attempts=2),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(RuntimeError, match="scored judge evaluation failed"):
        Judge(llm=llm, max_tokens=4).evaluate_scored("q", "a", "a")
    assert calls == 1
    assert llm.attempts()[0].status == "semantic_rejected"


@pytest.mark.parametrize("raw", ["11", "10.0", "score=10", "-1"])
def test_ragas_proxy_rejects_noncanonical_or_out_of_range_scores(raw: str) -> None:
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload(raw))

    llm = DeepSeekLlm(
        config=config(role=LlmRole.JUDGE, max_attempts=2),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(RuntimeError, match="RAGAS-style judge evaluation failed"):
        Judge(llm=llm, max_tokens=4).evaluate_ragas_style_proxy("q", "a", "context")
    assert calls == 1
    assert llm.attempts()[0].status == "semantic_rejected"


def test_journal_is_required_and_validated_before_provider_io(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv(RUN_ID_ENV)
    monkeypatch.delenv(JOURNAL_DIR_ENV)
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload())

    llm = DeepSeekLlm(config=config(), transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    with pytest.raises(LlmJournalError, match=RUN_ID_ENV):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert calls == 0


def test_finalized_attempt_is_durable_and_run_completion_binds_inventory(
    durable_attempt_journal,
) -> None:
    directory, run_id = durable_attempt_journal
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat(
        [{"role": "user", "content": "journal-secret-prompt"}],
        max_tokens=4,
        request_id="request-journal",
        sample_id="sample-journal",
    )

    snapshot = read_attempt_journal(directory, run_id)
    assert snapshot.attempt_count == 1
    assert snapshot.finalized_attempt_count == 1
    assert snapshot.incomplete_prepared_attempts == 0
    assert not snapshot.run_complete
    assert snapshot.entries[0].phase == "finalized"
    assert snapshot.entries[0].finalized == llm.attempts()[0].to_dict()
    assert snapshot.total_usd_accounted == llm.attempts()[0].usd_accounted
    assert len(snapshot.inventory_sha256) == 64

    completed = mark_attempt_journal_complete(directory, run_id)
    assert completed.run_complete
    assert completed.inventory_sha256 == snapshot.inventory_sha256
    assert completed.total_usd_accounted == snapshot.total_usd_accounted
    assert read_attempt_journal(directory, run_id) == completed
    assert mark_attempt_journal_complete(directory, run_id) == completed
    for path in directory.iterdir():
        assert path.stat().st_mode & 0o777 == 0o600
    encoded = b"".join(path.read_bytes() for path in directory.iterdir() if path.is_file())
    for forbidden in (
        b"journal-secret-prompt",
        b"test-key-never-log",
        str(directory).encode(),
        b"chat/completions",
    ):
        assert forbidden not in encoded


def test_process_abort_during_paid_call_preserves_prepared_reserved_cost(
    durable_attempt_journal,
) -> None:
    directory, run_id = durable_attempt_journal
    child = textwrap.dedent(
        """
        import os
        from datetime import UTC, datetime
        from decimal import Decimal
        from plico_benchmarks.core.llm import DeepSeekLlm
        from plico_benchmarks.core.llm_roles import (
            DEEPSEEK_API_BASE, LlmRole, LlmRoleConfig, RoleBudget,
        )

        def abort_transport(*_args, **_kwargs):
            os._exit(23)

        cfg = LlmRoleConfig(
            role=LlmRole.READER,
            provider="deepseek",
            api_base=DEEPSEEK_API_BASE,
            model="deepseek-v4-flash",
            api_key="child-api-key-sentinel",
            timeout_seconds=7.5,
            max_tokens=64,
            max_attempts=1,
            thinking="disabled",
            reasoning_effort=None,
            temperature=0.0,
            top_p=1.0,
            budget=RoleBudget(1, 10000, 64, Decimal("1.25")),
        )
        llm = DeepSeekLlm(
            config=cfg,
            transport=abort_transport,
            clock=lambda: datetime(2026, 8, 15, 0, 0, tzinfo=UTC),
        )
        llm.chat(
            [{"role": "user", "content": "child-paid-prompt-sentinel"}],
            max_tokens=4,
            request_id="child-request",
            sample_id="child-sample",
        )
        """
    )
    completed = subprocess.run(
        [sys.executable, "-c", child],
        env=os.environ.copy(),
        check=False,
        capture_output=True,
        text=True,
    )
    assert completed.returncode == 23
    assert completed.stdout == ""
    assert completed.stderr == ""

    snapshot = read_attempt_journal(directory, run_id)
    assert snapshot.attempt_count == 1
    assert snapshot.finalized_attempt_count == 0
    assert snapshot.incomplete_prepared_attempts == 1
    assert snapshot.entries[0].phase == "prepared_indeterminate"
    assert snapshot.entries[0].finalized is None
    assert Decimal(snapshot.total_usd_accounted) > 0
    assert not snapshot.run_complete
    with pytest.raises(LlmJournalError, match="indeterminate"):
        mark_attempt_journal_complete(directory, run_id)
    calls = 0

    def must_not_call(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload())

    aborted_config = replace(
        config(max_requests=1, max_attempts=1),
        budget=RoleBudget(1, 10000, 64, Decimal("1.25")),
    )
    reopened = DeepSeekLlm(
        config=aborted_config,
        transport=must_not_call,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmJournalError, match="indeterminate prepared"):
        reopened.chat([{"role": "user", "content": "must-not-send"}], max_tokens=4)
    assert calls == 0
    assert read_attempt_journal(directory, run_id) == snapshot
    encoded = b"".join(path.read_bytes() for path in directory.iterdir() if path.is_file())
    for forbidden in (
        b"child-paid-prompt-sentinel",
        b"child-api-key-sentinel",
        str(directory).encode(),
    ):
        assert forbidden not in encoded


@pytest.mark.parametrize(
    "tamper",
    ["extra", "missing", "negative_usage", "false_usd", "unsafe_url", "rebind"],
)
def test_journal_reader_rejects_self_consistent_typed_tampering(
    durable_attempt_journal,
    tamper: str,
) -> None:
    directory, run_id = durable_attempt_journal
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    finalized_path = next(directory.glob("*.finalized.json"))
    record = json.loads(finalized_path.read_text())
    evidence = record["evidence"]
    if tamper == "extra":
        evidence["raw_body"] = "secret-body"
    elif tamper == "missing":
        del evidence["role"]
    elif tamper == "negative_usage":
        evidence["usage"]["completion_tokens"] = -1
    elif tamper == "false_usd":
        evidence["usd_accounted"] = "0"
    elif tamper == "unsafe_url":
        evidence["pricing_source_url"] = "https://attacker.invalid/body-secret"
    else:
        record["prepared_record_sha256"] = "0" * 64
    finalized_path.write_bytes(canonical_json(record))

    with pytest.raises(LlmJournalError):
        read_attempt_journal(directory, run_id)


@pytest.mark.parametrize(
    "tamper",
    [
        "ok_non_2xx",
        "ok_non_stop",
        "ok_without_usage",
        "false_attestation",
        "completion_before_start",
        "recomputed_false_price",
    ],
)
def test_journal_reader_rejects_resigned_cross_field_falsehoods(
    durable_attempt_journal,
    tamper: str,
) -> None:
    directory, run_id = durable_attempt_journal
    llm = DeepSeekLlm(
        config=config(),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: OLD_PRICE_INSTANT,
    )
    llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    finalized_path = next(directory.glob("*.finalized.json"))
    record = json.loads(finalized_path.read_text())
    evidence = record["evidence"]
    if tamper == "ok_non_2xx":
        evidence["http_status"] = 503
    elif tamper == "ok_non_stop":
        evidence["finish_reason"] = "length"
    elif tamper == "ok_without_usage":
        evidence["usage"] = None
        evidence["usd_basis"] = "reserved_upper_bound"
        evidence["usd_accounted"] = format(
            (
                Decimal(evidence["reserved_input_tokens_upper_bound"])
                * Decimal(evidence["reservation_cache_miss_per_million_usd"])
                + Decimal(evidence["reserved_output_tokens"])
                * Decimal(evidence["reservation_output_per_million_usd"])
            )
            / Decimal(1_000_000),
            "f",
        )
    elif tamper == "false_attestation":
        evidence["response_model"] = "deepseek-v4-flash"
    elif tamper == "completion_before_start":
        evidence["completed_at_utc"] = "2026-08-14T00:00:00.000000Z"
    else:
        evidence["pricing_cache_miss_per_million_usd"] = "0.15"
        usage = evidence["usage"]
        evidence["usd_accounted"] = format(
            (
                Decimal(usage["prompt_cache_hit_tokens"])
                * Decimal(evidence["pricing_cache_hit_per_million_usd"])
                + Decimal(usage["prompt_cache_miss_tokens"]) * Decimal("0.15")
                + Decimal(usage["completion_tokens"])
                * Decimal(evidence["pricing_output_per_million_usd"])
            )
            / Decimal(1_000_000),
            "f",
        )
    finalized_path.write_bytes(canonical_json(record))
    with pytest.raises(LlmJournalError):
        read_attempt_journal(directory, run_id)


def test_pricing_horizon_crossing_finalizes_conservative_reserved_cost(
    durable_attempt_journal,
) -> None:
    directory, run_id = durable_attempt_journal
    instants = iter(
        [
            datetime(2026, 9, 15, 15, 59, 50, tzinfo=UTC),
            datetime(2026, 9, 15, 16, 0, 1, tzinfo=UTC),
        ]
    )
    llm = DeepSeekLlm(
        config=config(max_attempts=1),
        transport=lambda *_args, **_kwargs: FakeResponse(success_payload()),
        clock=lambda: next(instants),
    )
    with pytest.raises(LlmProtocolError, match="unpriced UTC"):
        llm.chat([{"role": "user", "content": "q"}], max_tokens=4)
    snapshot = read_attempt_journal(directory, run_id)
    assert snapshot.finalized_attempt_count == 1
    assert snapshot.entries[0].finalized is not None
    assert snapshot.entries[0].finalized["status"] == "pricing_error"
    assert snapshot.entries[0].finalized["usd_basis"] == "reserved_upper_bound"


def test_role_retry_limit_is_hard_bounded() -> None:
    env = role_env()
    env["PLICO_READER_MAX_ATTEMPTS"] = "4"
    with pytest.raises(LlmConfigurationError, match="must not exceed 3"):
        LlmRoleConfig.from_env(LlmRole.READER, env)


def test_durable_budget_survives_provider_reopen_without_an_extra_call() -> None:
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload())

    first = DeepSeekLlm(
        config=config(max_requests=1, max_attempts=1),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    first.chat([{"role": "user", "content": "first"}], max_tokens=4)
    reopened = DeepSeekLlm(
        config=config(max_requests=1, max_attempts=1),
        transport=transport,
        clock=lambda: OLD_PRICE_INSTANT,
    )
    with pytest.raises(LlmBudgetError, match="role request budget"):
        reopened.chat([{"role": "user", "content": "second"}], max_tokens=4)
    assert calls == 1


def test_role_configuration_is_frozen_before_any_provider_io() -> None:
    first = DeepSeekLlm(config=config(), clock=lambda: OLD_PRICE_INSTANT)
    assert first.is_available()
    changed = replace(config(), model="deepseek-v4-pro")
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        return FakeResponse(success_payload())

    second = DeepSeekLlm(config=changed, transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    with pytest.raises(LlmJournalError, match="configuration changed"):
        second.chat([{"role": "user", "content": "q"}], max_tokens=4)
    assert calls == 0


def test_two_provider_instances_share_one_durable_request_budget() -> None:
    entered = threading.Event()
    release = threading.Event()
    calls = 0

    def transport(*_args: object, **_kwargs: object) -> FakeResponse:
        nonlocal calls
        calls += 1
        entered.set()
        assert release.wait(timeout=2)
        return FakeResponse(success_payload())

    exact = config(max_requests=1, max_attempts=1)
    first = DeepSeekLlm(config=exact, transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    second = DeepSeekLlm(config=exact, transport=transport, clock=lambda: OLD_PRICE_INSTANT)
    errors: list[Exception] = []

    def run_first() -> None:
        try:
            first.chat([{"role": "user", "content": "first"}], max_tokens=4)
        except Exception as error:
            errors.append(error)

    thread = threading.Thread(target=run_first)
    thread.start()
    assert entered.wait(timeout=2)
    with pytest.raises(LlmJournalError, match="indeterminate prepared"):
        second.chat([{"role": "user", "content": "second"}], max_tokens=4)
    release.set()
    thread.join(timeout=2)
    assert not thread.is_alive()
    assert errors == []
    assert calls == 1
