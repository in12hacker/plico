"""Fail-closed DeepSeek chat boundary for benchmark roles."""

from __future__ import annotations

import hashlib
import json
import re
import threading
import time
import uuid
from collections.abc import Callable, Mapping
from dataclasses import asdict, dataclass, replace
from datetime import UTC, datetime, timedelta
from decimal import Decimal
from typing import Any, Protocol

import requests

from plico_benchmarks.core.llm_journal import (
    AttemptJournal,
    LlmJournalError,
    PreparedAttempt,
)
from plico_benchmarks.core.llm_roles import (
    DEEPSEEK_MODELS,
    DEEPSEEK_OFFICIAL_MODEL_VERSIONS,
    DEEPSEEK_PRICING_POLICY_SHA256,
    LlmConfigurationError,
    LlmPricingError,
    LlmRole,
    LlmRoleConfig,
    PriceSelection,
    RoleBudget,
    select_deepseek_interval_price,
    select_deepseek_price,
)


class LlmProvider(Protocol):
    """Abstract LLM provider."""

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str: ...

    def is_available(self) -> bool: ...


class LlmBudgetError(RuntimeError):
    """A request was rejected before I/O because its role budget is exhausted."""


class LlmRemoteError(RuntimeError):
    """A DeepSeek attempt failed without exposing response content or endpoint paths."""

    def __init__(
        self,
        message: str,
        *,
        retryable: bool,
        retry_after_seconds: float | None = None,
    ):
        super().__init__(message)
        self.retryable = retryable
        self.retry_after_seconds = retry_after_seconds


class LlmProtocolError(RuntimeError):
    """A DeepSeek response did not satisfy the evidence protocol."""


@dataclass(frozen=True)
class LlmUsage:
    prompt_tokens: int
    prompt_cache_hit_tokens: int
    prompt_cache_miss_tokens: int
    completion_tokens: int
    total_tokens: int
    cache_accounting: str


@dataclass(frozen=True)
class LlmAttemptEvidence:
    schema: str
    role: str
    role_request_id: str
    sample_id: str | None
    attempt_sequence: int
    attempt_in_request: int
    status: str
    http_status: int | None
    prompt_sha256: str
    requested_model_alias: str
    official_model_version: str | None
    model_revision_attestation: str
    response_model: str | None
    system_fingerprint: str | None
    cross_run_comparability: str
    usage: LlmUsage | None
    finish_reason: str | None
    thinking: str
    reasoning_effort: str | None
    temperature: float | None
    top_p: float | None
    timeout_seconds: float
    max_tokens: int
    generation_seed: str
    started_at_utc: str
    completed_at_utc: str
    latency_ms: float
    pricing_schedule_id: str
    pricing_effective_at: str
    pricing_review_not_after: str
    billing_band: str
    pricing_cache_hit_per_million_usd: str
    pricing_cache_miss_per_million_usd: str
    pricing_output_per_million_usd: str
    pricing_source_url: str
    pricing_source_retrieved_at: str
    pricing_source_reviewed_at: str
    pricing_local_frozen_schedule_record_sha256: str
    reservation_pricing_schedule_id: str
    reservation_billing_band: str
    reservation_cache_hit_per_million_usd: str
    reservation_cache_miss_per_million_usd: str
    reservation_output_per_million_usd: str
    reserved_input_tokens_upper_bound: int
    reserved_output_tokens: int
    budget_max_requests: int
    budget_max_input_tokens: int
    budget_max_output_tokens: int
    budget_max_usd: str
    usd_accounted: str
    usd_basis: str

    def to_dict(self) -> dict[str, Any]:
        """Return the content-free, secret-free evidence record."""
        return asdict(self)


@dataclass(frozen=True)
class LlmBudgetSnapshot:
    requests: int
    input_tokens_accounted: int
    output_tokens_accounted: int
    usd_accounted: str


@dataclass(frozen=True)
class _Reservation:
    attempt: int
    input_tokens: int
    output_tokens: int
    usd: Decimal
    pricing: PriceSelection
    journal_attempt: PreparedAttempt | None = None


class _BudgetLedger:
    def __init__(
        self,
        limits: RoleBudget,
        *,
        requests: int = 0,
        input_tokens: int = 0,
        output_tokens: int = 0,
        usd: Decimal = Decimal(0),
    ):
        self._limits = limits
        self._lock = threading.Lock()
        self._requests = requests
        self._input_tokens = input_tokens
        self._output_tokens = output_tokens
        self._usd = usd

    def reserve(
        self,
        prompt_tokens_upper_bound: int,
        output_tokens: int,
        pricing: PriceSelection,
    ) -> _Reservation:
        prices = pricing.prices
        usd = (
            Decimal(prompt_tokens_upper_bound) * prices.cache_miss_per_million
            + Decimal(output_tokens) * prices.output_per_million
        ) / Decimal(1_000_000)
        with self._lock:
            if self._requests + 1 > self._limits.max_requests:
                raise LlmBudgetError("role request budget exhausted")
            if self._input_tokens + prompt_tokens_upper_bound > self._limits.max_input_tokens:
                raise LlmBudgetError("role input-token budget exhausted")
            if self._output_tokens + output_tokens > self._limits.max_output_tokens:
                raise LlmBudgetError("role output-token budget exhausted")
            if self._usd + usd > self._limits.max_usd:
                raise LlmBudgetError("role USD budget exhausted")
            self._requests += 1
            self._input_tokens += prompt_tokens_upper_bound
            self._output_tokens += output_tokens
            self._usd += usd
            return _Reservation(
                attempt=self._requests,
                input_tokens=prompt_tokens_upper_bound,
                output_tokens=output_tokens,
                usd=usd,
                pricing=pricing,
            )

    def finalize(
        self,
        reservation: _Reservation,
        usage: LlmUsage,
        pricing: PriceSelection,
    ) -> Decimal:
        if usage.prompt_tokens > reservation.input_tokens:
            raise LlmProtocolError("reported prompt usage exceeds the reserved upper bound")
        if usage.completion_tokens > reservation.output_tokens:
            raise LlmProtocolError("reported completion usage exceeds requested max_tokens")
        prices = pricing.prices
        usd = (
            Decimal(usage.prompt_cache_hit_tokens) * prices.cache_hit_per_million
            + Decimal(usage.prompt_cache_miss_tokens) * prices.cache_miss_per_million
            + Decimal(usage.completion_tokens) * prices.output_per_million
        ) / Decimal(1_000_000)
        with self._lock:
            self._input_tokens += usage.prompt_tokens - reservation.input_tokens
            self._output_tokens += usage.completion_tokens - reservation.output_tokens
            self._usd += usd - reservation.usd
        return usd

    def snapshot(self) -> LlmBudgetSnapshot:
        with self._lock:
            return LlmBudgetSnapshot(
                requests=self._requests,
                input_tokens_accounted=self._input_tokens,
                output_tokens_accounted=self._output_tokens,
                usd_accounted=_decimal_text(self._usd),
            )


class OpenAiCompatibleLlm:
    """Compatibility name for the benchmark's DeepSeek-only OpenAI boundary.

    Construction may remain lazy so offline suites can replace the provider,
    but the first availability check or chat requires one exact PLICO_* role
    configuration. Legacy OPENAI/LLAMA/LLM_BACKEND variables are never read.
    """

    def __init__(
        self,
        api_base: str | None = None,
        model: str | None = None,
        api_key: str | None = None,
        timeout: float | None = None,
        *,
        role: LlmRole | str = LlmRole.READER,
        config: LlmRoleConfig | None = None,
        transport: Callable[..., Any] | None = None,
        clock: Callable[[], datetime] | None = None,
        sleeper: Callable[[float], None] | None = None,
        monotonic: Callable[[], float] | None = None,
    ):
        self.role = config.role if config is not None else LlmRole(role)
        self._config = config
        self._legacy_exact = (api_base, model, api_key, timeout)
        self._transport = transport or requests.post
        self._clock = clock or (lambda: datetime.now(UTC))
        self._sleeper = sleeper or time.sleep
        self._monotonic = monotonic or time.monotonic
        self._budget: _BudgetLedger | None = None
        self._journal: AttemptJournal | None = None
        self._configuration_lock = threading.Lock()
        self._attempts: list[LlmAttemptEvidence] = []
        self._attempts_lock = threading.Lock()

    @property
    def model(self) -> str:
        if self._config is not None:
            return self._config.model
        return "unconfigured"

    def _configured(self) -> LlmRoleConfig:
        with self._configuration_lock:
            if self._config is None:
                config = LlmRoleConfig.from_env(self.role)
                api_base, model, api_key, timeout = self._legacy_exact
                exact_overrides = (
                    (api_base, config.api_base, "api_base"),
                    (model, config.model, "model"),
                    (api_key, config.api_key, "api_key"),
                    (timeout, config.timeout_seconds, "timeout"),
                )
                for supplied, expected, name in exact_overrides:
                    if supplied is not None and supplied != expected:
                        raise LlmConfigurationError(
                            f"legacy {name} override does not match the exact role configuration"
                        )
                self._config = config
            if self._journal is None:
                self._journal = AttemptJournal.from_env()
                self._journal.register_role_config(_role_journal_config(self._config))
            if self._budget is None:
                durable = self._journal.role_accounting(self.role.value)
                self._budget = _BudgetLedger(
                    self._config.budget,
                    requests=durable.requests,
                    input_tokens=durable.input_tokens_accounted,
                    output_tokens=durable.output_tokens_accounted,
                    usd=Decimal(durable.usd_accounted),
                )
        return self._config

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str:
        return self._request(messages, validator=None, **kwargs)

    def chat_validated(
        self,
        messages: list[dict[str, str]],
        validator: Callable[[str], bool],
        **kwargs: Any,
    ) -> str:
        """Run one role-owned request; semantic rejection is not retried."""
        return self._request(messages, validator=validator, **kwargs)

    def _request(
        self,
        messages: list[dict[str, str]],
        *,
        validator: Callable[[str], bool] | None,
        **kwargs: Any,
    ) -> str:
        config = self._configured()
        assert self._journal is not None
        self._journal.assert_can_start_attempt()
        unknown = set(kwargs) - {
            "temperature",
            "max_tokens",
            "top_p",
            "request_id",
            "sample_id",
        }
        if unknown:
            raise LlmConfigurationError("unsupported DeepSeek chat option")
        raw_request_id = kwargs.pop("request_id", None)
        role_request_id = (
            str(uuid.uuid4())
            if raw_request_id is None
            else _safe_correlation(raw_request_id, "request_id")
        )
        raw_sample_id = kwargs.pop("sample_id", None)
        sample_id = None if raw_sample_id is None else _safe_correlation(raw_sample_id, "sample_id")
        max_tokens = kwargs.get("max_tokens", config.max_tokens)
        if not isinstance(max_tokens, int) or isinstance(max_tokens, bool) or max_tokens <= 0:
            raise LlmConfigurationError("max_tokens must be a positive integer")
        if max_tokens > config.max_tokens:
            raise LlmBudgetError("request max_tokens exceeds the exact role limit")
        temperature = kwargs.get("temperature", config.temperature)
        top_p = kwargs.get("top_p", config.top_p)
        if temperature != config.temperature or top_p != config.top_p:
            raise LlmConfigurationError(
                "temperature and top_p must match the exact role configuration"
            )
        request_deadline = self._monotonic() + config.timeout_seconds * config.max_attempts
        for attempt_in_request in range(1, config.max_attempts + 1):
            try:
                return self._chat_once(
                    messages,
                    config=config,
                    role_request_id=role_request_id,
                    sample_id=sample_id,
                    attempt_in_request=attempt_in_request,
                    validator=validator,
                    max_tokens=max_tokens,
                    temperature=temperature,
                    top_p=top_p,
                )
            except LlmRemoteError as error:
                if not error.retryable or attempt_in_request == config.max_attempts:
                    raise
                delay = (
                    error.retry_after_seconds
                    if error.retry_after_seconds is not None
                    else min(0.25 * (2 ** (attempt_in_request - 1)), 2.0)
                )
                if self._monotonic() + delay + config.timeout_seconds > request_deadline:
                    raise LlmRemoteError(
                        "DeepSeek retry would exceed the bounded request deadline",
                        retryable=False,
                    ) from None
                self._sleeper(delay)
        raise LlmRemoteError("DeepSeek attempts exhausted", retryable=False)

    def _chat_once(
        self,
        messages: list[dict[str, str]],
        *,
        config: LlmRoleConfig,
        role_request_id: str,
        sample_id: str | None,
        attempt_in_request: int,
        validator: Callable[[str], bool] | None,
        max_tokens: int,
        temperature: float | None,
        top_p: float | None,
    ) -> str:
        canonical_prompt, prompt_upper_bound = _canonical_prompt(messages)
        prompt_sha256 = hashlib.sha256(canonical_prompt).hexdigest()
        started_at_utc = self._clock()
        reservation_pricing = select_deepseek_interval_price(
            config.model,
            started_at_utc,
            started_at_utc + timedelta(seconds=config.timeout_seconds),
        )
        assert self._budget is not None
        reservation = self._budget.reserve(prompt_upper_bound, max_tokens, reservation_pricing)
        payload: dict[str, Any] = {
            "model": config.model,
            "messages": messages,
            "thinking": {"type": config.thinking},
            "max_tokens": max_tokens,
        }
        if config.reasoning_effort is not None:
            payload["reasoning_effort"] = config.reasoning_effort
        if temperature is not None:
            payload["temperature"] = temperature
        if top_p is not None:
            payload["top_p"] = top_p
        headers = {
            "Authorization": f"Bearer {config.api_key}",
            "Content-Type": "application/json",
        }
        assert self._journal is not None
        reservation = replace(
            reservation,
            journal_attempt=self._journal.prepare(
                self._prepared_evidence(
                    reservation,
                    prompt_sha256,
                    config=config,
                    role_request_id=role_request_id,
                    sample_id=sample_id,
                    attempt_in_request=attempt_in_request,
                    max_tokens=max_tokens,
                    started_at_utc=started_at_utc,
                )
            ),
        )
        started = time.monotonic()
        try:
            response = self._transport(
                f"{config.api_base}/chat/completions",
                json=payload,
                headers=headers,
                timeout=config.timeout_seconds,
            )
        except Exception:
            completed_at_utc = self._clock()
            try:
                pricing = select_deepseek_interval_price(
                    config.model, started_at_utc, completed_at_utc
                )
            except LlmPricingError:
                self._record(
                    reservation,
                    reservation_pricing,
                    prompt_sha256,
                    config=config,
                    role_request_id=role_request_id,
                    sample_id=sample_id,
                    attempt_in_request=attempt_in_request,
                    max_tokens=max_tokens,
                    started_at_utc=started_at_utc,
                    completed_at_utc=completed_at_utc,
                    status="pricing_error",
                    http_status=None,
                    latency_ms=_elapsed_ms(started),
                )
                raise LlmProtocolError("request crossed unpriced UTC time") from None
            self._record(
                reservation,
                pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="indeterminate_transport",
                http_status=None,
                latency_ms=_elapsed_ms(started),
            )
            raise LlmRemoteError(
                "DeepSeek request outcome is indeterminate", retryable=False
            ) from None

        completed_at_utc = self._clock()
        try:
            pricing = select_deepseek_interval_price(config.model, started_at_utc, completed_at_utc)
        except LlmPricingError:
            self._record(
                reservation,
                reservation_pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="pricing_error",
                http_status=None,
                latency_ms=_elapsed_ms(started),
            )
            raise LlmProtocolError("request crossed unpriced UTC time") from None

        try:
            status_code = _http_status(response)
        except LlmProtocolError as error:
            self._record(
                reservation,
                pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="protocol_error",
                http_status=None,
                latency_ms=_elapsed_ms(started),
            )
            raise error from None
        if status_code < 200 or status_code >= 300:
            usage = _optional_usage(response)
            usd, basis, _ = self._account_usage(reservation, pricing, usage)
            self._record(
                reservation,
                pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="http_error",
                http_status=status_code,
                latency_ms=_elapsed_ms(started),
                usage=usage,
                usd=usd,
                usd_basis=basis,
            )
            retryable = status_code == 429 or status_code in {500, 502, 503, 504}
            raise LlmRemoteError(
                f"DeepSeek HTTP status {status_code}",
                retryable=retryable,
                retry_after_seconds=(_retry_after_seconds(response) if retryable else None),
            ) from None

        usage: LlmUsage | None = None
        response_model: str | None = None
        fingerprint: str | None = None
        finish_reason: str | None = None
        try:
            data = response.json()
            usage = _parse_usage(data)
            response_model = _safe_identifier(data.get("model"), "returned model")
            if response_model not in _response_models_for(config.model):
                raise LlmProtocolError("response model does not match the requested model")
            fingerprint = _safe_identifier(data.get("system_fingerprint"), "system fingerprint")
            choices = data.get("choices")
            if not isinstance(choices, list) or len(choices) != 1:
                raise LlmProtocolError("response must contain exactly one choice")
            choice = choices[0]
            if not isinstance(choice, Mapping):
                raise LlmProtocolError("response choice must be an object")
            finish_reason = _safe_identifier(choice.get("finish_reason"), "finish reason")
            message = choice.get("message")
            if not isinstance(message, Mapping) or not isinstance(message.get("content"), str):
                raise LlmProtocolError("response content is missing")
            content = message["content"].strip()
        except LlmProtocolError as error:
            usd, basis, accounting_error = self._account_usage(reservation, pricing, usage)
            self._record(
                reservation,
                pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="protocol_error",
                http_status=status_code,
                latency_ms=_elapsed_ms(started),
                response_model=response_model,
                system_fingerprint=fingerprint,
                usage=usage,
                finish_reason=finish_reason,
                usd=usd,
                usd_basis=basis,
            )
            raise (accounting_error or error) from None
        except Exception:
            self._record(
                reservation,
                pricing,
                prompt_sha256,
                config=config,
                role_request_id=role_request_id,
                sample_id=sample_id,
                attempt_in_request=attempt_in_request,
                max_tokens=max_tokens,
                started_at_utc=started_at_utc,
                completed_at_utc=completed_at_utc,
                status="protocol_error",
                http_status=status_code,
                latency_ms=_elapsed_ms(started),
            )
            raise LlmProtocolError("DeepSeek response is not valid JSON evidence") from None

        usd, basis, accounting_error = self._account_usage(reservation, pricing, usage)
        terminal_error = accounting_error
        status = "accounting_error" if accounting_error is not None else "ok"
        if accounting_error is None and finish_reason != "stop":
            status = "incomplete"
            terminal_error = LlmProtocolError("DeepSeek response did not finish normally")
        elif accounting_error is None and validator is not None and not validator(content):
            status = "semantic_rejected"
            terminal_error = LlmProtocolError("DeepSeek response failed role validation")
        self._record(
            reservation,
            pricing,
            prompt_sha256,
            config=config,
            role_request_id=role_request_id,
            sample_id=sample_id,
            attempt_in_request=attempt_in_request,
            max_tokens=max_tokens,
            started_at_utc=started_at_utc,
            completed_at_utc=completed_at_utc,
            status=status,
            http_status=status_code,
            latency_ms=_elapsed_ms(started),
            response_model=response_model,
            system_fingerprint=fingerprint,
            usage=usage,
            finish_reason=finish_reason,
            usd=usd,
            usd_basis=basis,
        )
        if terminal_error is not None:
            raise terminal_error from None
        return content

    def _account_usage(
        self,
        reservation: _Reservation,
        pricing: PriceSelection,
        usage: LlmUsage | None,
    ) -> tuple[Decimal, str, LlmProtocolError | None]:
        if usage is None:
            return reservation.usd, "reserved_upper_bound", None
        assert self._budget is not None
        try:
            usd = self._budget.finalize(reservation, usage, pricing)
        except LlmProtocolError as error:
            return reservation.usd, "reserved_upper_bound", error
        return usd, "actual_usage", None

    def _record(
        self,
        reservation: _Reservation,
        pricing: PriceSelection,
        prompt_sha256: str,
        *,
        config: LlmRoleConfig,
        role_request_id: str,
        sample_id: str | None,
        attempt_in_request: int,
        max_tokens: int,
        started_at_utc: datetime,
        completed_at_utc: datetime,
        status: str,
        http_status: int | None,
        latency_ms: float,
        response_model: str | None = None,
        system_fingerprint: str | None = None,
        usage: LlmUsage | None = None,
        finish_reason: str | None = None,
        usd: Decimal | None = None,
        usd_basis: str = "reserved_upper_bound",
    ) -> None:
        official_model_version, revision_attestation = _model_revision_evidence(
            config.model, response_model
        )
        if revision_attestation == "attested_exact_version":
            cross_run_comparability = "requires_five_run_variance_ci"
        elif revision_attestation == "unattested_alias":
            cross_run_comparability = "requires_same_system_fingerprint_and_five_run_variance_ci"
        elif revision_attestation == "unattested_mismatch":
            cross_run_comparability = "not_comparable_model_mismatch"
        else:
            cross_run_comparability = "not_comparable_no_response"
        if reservation.journal_attempt is None or self._journal is None:
            raise LlmJournalError("paid attempt lacks its durable reservation")
        evidence = LlmAttemptEvidence(
            schema="plico.benchmark.llm-attempt-evidence/v1",
            role=self.role.value,
            role_request_id=role_request_id,
            sample_id=sample_id,
            attempt_sequence=reservation.journal_attempt.sequence,
            attempt_in_request=attempt_in_request,
            status=status,
            http_status=http_status,
            prompt_sha256=prompt_sha256,
            requested_model_alias=config.model,
            official_model_version=official_model_version,
            model_revision_attestation=revision_attestation,
            response_model=response_model,
            system_fingerprint=system_fingerprint,
            cross_run_comparability=cross_run_comparability,
            usage=usage,
            finish_reason=finish_reason,
            thinking=config.thinking,
            reasoning_effort=config.reasoning_effort,
            temperature=config.temperature,
            top_p=config.top_p,
            timeout_seconds=config.timeout_seconds,
            max_tokens=max_tokens,
            generation_seed=config.generation_seed,
            started_at_utc=_utc_text(started_at_utc),
            completed_at_utc=_utc_text(completed_at_utc),
            latency_ms=latency_ms,
            pricing_schedule_id=pricing.pricing_schedule_id,
            pricing_effective_at=pricing.effective_at,
            pricing_review_not_after=pricing.review_not_after,
            billing_band=pricing.billing_band,
            pricing_cache_hit_per_million_usd=_decimal_text(pricing.prices.cache_hit_per_million),
            pricing_cache_miss_per_million_usd=_decimal_text(pricing.prices.cache_miss_per_million),
            pricing_output_per_million_usd=_decimal_text(pricing.prices.output_per_million),
            pricing_source_url=pricing.source_url,
            pricing_source_retrieved_at=pricing.source_retrieved_at,
            pricing_source_reviewed_at=pricing.source_reviewed_at,
            pricing_local_frozen_schedule_record_sha256=(
                pricing.local_frozen_schedule_record_sha256
            ),
            reservation_pricing_schedule_id=reservation.pricing.pricing_schedule_id,
            reservation_billing_band=reservation.pricing.billing_band,
            reservation_cache_hit_per_million_usd=_decimal_text(
                reservation.pricing.prices.cache_hit_per_million
            ),
            reservation_cache_miss_per_million_usd=_decimal_text(
                reservation.pricing.prices.cache_miss_per_million
            ),
            reservation_output_per_million_usd=_decimal_text(
                reservation.pricing.prices.output_per_million
            ),
            reserved_input_tokens_upper_bound=reservation.input_tokens,
            reserved_output_tokens=reservation.output_tokens,
            budget_max_requests=config.budget.max_requests,
            budget_max_input_tokens=config.budget.max_input_tokens,
            budget_max_output_tokens=config.budget.max_output_tokens,
            budget_max_usd=_decimal_text(config.budget.max_usd),
            usd_accounted=_decimal_text(reservation.usd if usd is None else usd),
            usd_basis=usd_basis,
        )
        finalized = self._journal.finalize(reservation.journal_attempt, evidence.to_dict())
        committed = replace(evidence, attempt_sequence=int(finalized["attempt_sequence"]))
        with self._attempts_lock:
            self._attempts.append(committed)

    def _prepared_evidence(
        self,
        reservation: _Reservation,
        prompt_sha256: str,
        *,
        config: LlmRoleConfig,
        role_request_id: str,
        sample_id: str | None,
        attempt_in_request: int,
        max_tokens: int,
        started_at_utc: datetime,
    ) -> dict[str, Any]:
        pricing = reservation.pricing
        return {
            "schema": "plico.benchmark.llm-attempt-reservation/v1",
            "role": self.role.value,
            "role_request_id": role_request_id,
            "sample_id": sample_id,
            "attempt_sequence": 0,
            "attempt_in_request": attempt_in_request,
            "prompt_sha256": prompt_sha256,
            "requested_model_alias": config.model,
            "thinking": config.thinking,
            "reasoning_effort": config.reasoning_effort,
            "temperature": config.temperature,
            "top_p": config.top_p,
            "timeout_seconds": config.timeout_seconds,
            "max_tokens": max_tokens,
            "generation_seed": config.generation_seed,
            "started_at_utc": _utc_text(started_at_utc),
            "reservation_pricing_schedule_id": pricing.pricing_schedule_id,
            "reservation_billing_band": pricing.billing_band,
            "reservation_cache_hit_per_million_usd": _decimal_text(
                pricing.prices.cache_hit_per_million
            ),
            "reservation_cache_miss_per_million_usd": _decimal_text(
                pricing.prices.cache_miss_per_million
            ),
            "reservation_output_per_million_usd": _decimal_text(pricing.prices.output_per_million),
            "reserved_input_tokens_upper_bound": reservation.input_tokens,
            "reserved_output_tokens": reservation.output_tokens,
            "budget_max_requests": config.budget.max_requests,
            "budget_max_input_tokens": config.budget.max_input_tokens,
            "budget_max_output_tokens": config.budget.max_output_tokens,
            "budget_max_usd": _decimal_text(config.budget.max_usd),
            "usd_accounted": _decimal_text(reservation.usd),
            "usd_basis": "reserved_upper_bound",
        }

    def attempts(self) -> tuple[LlmAttemptEvidence, ...]:
        with self._attempts_lock:
            return tuple(self._attempts)

    def evidence_since(
        self,
        attempt_sequence: int,
        *,
        role_request_id: str | None = None,
    ) -> tuple[LlmAttemptEvidence, ...]:
        """Return immutable role evidence after a caller's prior sequence watermark."""
        if attempt_sequence < 0:
            raise ValueError("attempt_sequence must be non-negative")
        if role_request_id is not None:
            role_request_id = _safe_correlation(role_request_id, "role_request_id")
        with self._attempts_lock:
            return tuple(
                evidence
                for evidence in self._attempts
                if evidence.attempt_sequence > attempt_sequence
                and (role_request_id is None or evidence.role_request_id == role_request_id)
            )

    def budget_snapshot(self) -> LlmBudgetSnapshot:
        self._configured()
        assert self._journal is not None
        durable = self._journal.role_accounting(self.role.value)
        return LlmBudgetSnapshot(
            requests=durable.requests,
            input_tokens_accounted=durable.input_tokens_accounted,
            output_tokens_accounted=durable.output_tokens_accounted,
            usd_accounted=durable.usd_accounted,
        )

    def configured_max_attempts(self) -> int:
        return self._configured().max_attempts

    def is_available(self) -> bool:
        """Return configuration/pricing availability without network I/O."""
        try:
            config = self._configured()
            select_deepseek_price(config.model, self._clock())
        except (LlmConfigurationError, RuntimeError):
            return False
        return True


DeepSeekLlm = OpenAiCompatibleLlm


class StubLlm:
    """Explicitly injected offline stub; never selected from environment."""

    def __init__(self, response: str = "stub"):
        self.response = response

    def chat(self, messages: list[dict[str, str]], **kwargs: Any) -> str:
        return self.response

    def is_available(self) -> bool:
        return True


def default_llm(role: LlmRole | str = LlmRole.READER) -> LlmProvider:
    """Return a lazy, exact DeepSeek role provider with no fallback."""
    return DeepSeekLlm(role=role)


def _role_journal_config(config: LlmRoleConfig) -> dict[str, Any]:
    return {
        "schema": "plico.benchmark.llm-attempt-role-config/v1",
        "role": config.role.value,
        "provider": config.provider,
        "api_base_origin": config.api_base,
        "requested_model_alias": config.model,
        "official_model_version": DEEPSEEK_OFFICIAL_MODEL_VERSIONS[config.model],
        "thinking": config.thinking,
        "reasoning_effort": config.reasoning_effort,
        "temperature": config.temperature,
        "top_p": config.top_p,
        "timeout_seconds": config.timeout_seconds,
        "max_tokens": config.max_tokens,
        "max_attempts": config.max_attempts,
        "generation_seed": config.generation_seed,
        "budget_max_requests": config.budget.max_requests,
        "budget_max_input_tokens": config.budget.max_input_tokens,
        "budget_max_output_tokens": config.budget.max_output_tokens,
        "budget_max_usd": _decimal_text(config.budget.max_usd),
        "pricing_policy_sha256": DEEPSEEK_PRICING_POLICY_SHA256,
    }


def _canonical_prompt(messages: list[dict[str, str]]) -> tuple[bytes, int]:
    if not isinstance(messages, list) or not messages:
        raise LlmConfigurationError("messages must be a non-empty list")
    normalized: list[dict[str, str]] = []
    byte_bound = 64
    for message in messages:
        if not isinstance(message, dict) or set(message) != {"role", "content"}:
            raise LlmConfigurationError("each message must contain only role and content")
        role = message["role"]
        content = message["content"]
        if role not in {"system", "user", "assistant"} or not isinstance(content, str):
            raise LlmConfigurationError("message role or content is invalid")
        normalized.append({"content": content, "role": role})
        byte_bound += len(role.encode("utf-8")) + len(content.encode("utf-8")) + 32
    canonical = json.dumps(
        normalized,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return canonical, max(byte_bound, len(canonical))


def _http_status(response: Any) -> int:
    status = getattr(response, "status_code", None)
    if not isinstance(status, int) or isinstance(status, bool):
        raise LlmProtocolError("DeepSeek response has no numeric HTTP status")
    return status


def _retry_after_seconds(response: Any) -> float | None:
    headers = getattr(response, "headers", None)
    if not isinstance(headers, Mapping):
        return None
    raw = headers.get("Retry-After")
    if not isinstance(raw, str) or not re.fullmatch(r"[0-9]{1,3}", raw):
        return None
    return min(float(int(raw)), 5.0)


def _optional_usage(response: Any) -> LlmUsage | None:
    try:
        data = response.json()
        return _parse_usage(data)
    except Exception:
        return None


def _parse_usage(data: Any) -> LlmUsage:
    if not isinstance(data, Mapping) or not isinstance(data.get("usage"), Mapping):
        raise LlmProtocolError("response usage is missing")
    usage = data["usage"]
    prompt = _usage_int(usage, "prompt_tokens")
    completion = _usage_int(usage, "completion_tokens")
    total = _usage_int(usage, "total_tokens")
    if total != prompt + completion:
        raise LlmProtocolError("response total_tokens is inconsistent")
    raw_hit = usage.get("prompt_cache_hit_tokens")
    raw_miss = usage.get("prompt_cache_miss_tokens")
    if raw_hit is None and raw_miss is None:
        hit = 0
        miss = prompt
        cache_accounting = "all_miss_conservative"
    else:
        hit = _usage_int(usage, "prompt_cache_hit_tokens")
        miss = _usage_int(usage, "prompt_cache_miss_tokens")
        if hit + miss != prompt:
            raise LlmProtocolError("cache token split is inconsistent")
        cache_accounting = "provider_reported"
    return LlmUsage(
        prompt_tokens=prompt,
        prompt_cache_hit_tokens=hit,
        prompt_cache_miss_tokens=miss,
        completion_tokens=completion,
        total_tokens=total,
        cache_accounting=cache_accounting,
    )


def _usage_int(usage: Mapping[str, Any], name: str) -> int:
    value = usage.get(name)
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise LlmProtocolError(f"response {name} is invalid")
    return value


_SAFE_IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z")


def _safe_identifier(value: Any, field: str) -> str:
    if not isinstance(value, str) or not _SAFE_IDENTIFIER.fullmatch(value):
        raise LlmProtocolError(f"response {field} is missing or unsafe")
    return value


def _response_models_for(requested: str) -> frozenset[str]:
    if requested not in DEEPSEEK_MODELS:
        return frozenset()
    pinned = {
        "deepseek-v4-flash": "DeepSeek-V4-Flash-0731",
        "deepseek-v4-pro": "DeepSeek-V4-Pro-0813",
    }
    return frozenset({requested, pinned[requested]})


def _official_model_version(requested: str) -> str:
    versions = {
        "deepseek-v4-flash": "DeepSeek-V4-Flash-0731",
        "deepseek-v4-pro": "DeepSeek-V4-Pro-0813",
    }
    try:
        return versions[requested]
    except KeyError:
        raise LlmConfigurationError("unapproved DeepSeek model") from None


def _model_revision_evidence(
    requested_alias: str, response_model: str | None
) -> tuple[str | None, str]:
    pinned = _official_model_version(requested_alias)
    if response_model == pinned:
        return pinned, "attested_exact_version"
    if response_model == requested_alias:
        return None, "unattested_alias"
    if response_model is not None:
        return None, "unattested_mismatch"
    return None, "unattested_no_response"


def _safe_correlation(value: Any, field: str) -> str:
    if not isinstance(value, str) or not _SAFE_IDENTIFIER.fullmatch(value):
        raise LlmConfigurationError(f"{field} must be a safe opaque identifier")
    return value


def _elapsed_ms(started: float) -> float:
    return round((time.monotonic() - started) * 1000, 3)


def _utc_text(value: datetime) -> str:
    if value.tzinfo is None:
        raise LlmPricingError("attempt start must be timezone-aware")
    return value.astimezone(UTC).isoformat(timespec="microseconds").replace("+00:00", "Z")


def _decimal_text(value: Decimal) -> str:
    return format(value, "f")
