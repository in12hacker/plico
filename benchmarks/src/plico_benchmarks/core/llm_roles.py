"""Exact, fail-closed DeepSeek role configuration and price schedules."""

from __future__ import annotations

import hashlib
import json
import os
from dataclasses import dataclass, field
from datetime import UTC, datetime, time, timedelta
from decimal import Decimal, InvalidOperation
from enum import Enum
from typing import Mapping

DEEPSEEK_API_BASE = "https://api.deepseek.com"
DEEPSEEK_MODELS = frozenset({"deepseek-v4-flash", "deepseek-v4-pro"})
DEEPSEEK_OFFICIAL_MODEL_VERSIONS = {
    "deepseek-v4-flash": "DeepSeek-V4-Flash-0731",
    "deepseek-v4-pro": "DeepSeek-V4-Pro-0813",
}
MAX_ROLE_ATTEMPTS = 3
MAX_ROLE_REQUESTS = 100_000
MAX_ROLE_INPUT_TOKENS = 1_000_000_000
MAX_ROLE_OUTPUT_TOKENS = 100_000_000
MAX_ROLE_USD = Decimal("100000")


class LlmRole(str, Enum):
    READER = "reader"
    JUDGE = "judge"
    COMPILER = "compiler"

    @property
    def env_prefix(self) -> str:
        return f"PLICO_{self.value.upper()}"


class LlmConfigurationError(ValueError):
    """The paid LLM boundary is not configured exactly enough to call."""


class LlmPricingError(RuntimeError):
    """No verified price schedule covers the requested call."""


@dataclass(frozen=True)
class RoleBudget:
    max_requests: int
    max_input_tokens: int
    max_output_tokens: int
    max_usd: Decimal


@dataclass(frozen=True)
class LlmRoleConfig:
    role: LlmRole
    provider: str
    api_base: str
    model: str
    api_key: str = field(repr=False, compare=False)
    timeout_seconds: float
    max_tokens: int
    max_attempts: int
    thinking: str
    reasoning_effort: str | None
    temperature: float | None
    top_p: float | None
    budget: RoleBudget
    generation_seed: str = field(init=False, default="provider_unavailable")

    def __post_init__(self) -> None:
        if not 1 <= self.max_attempts <= MAX_ROLE_ATTEMPTS:
            raise LlmConfigurationError(
                f"role max_attempts must be between 1 and {MAX_ROLE_ATTEMPTS}"
            )
        if not 1 <= self.budget.max_requests <= MAX_ROLE_REQUESTS:
            raise LlmConfigurationError("role max_requests exceeds its hard limit")
        if not 1 <= self.budget.max_input_tokens <= MAX_ROLE_INPUT_TOKENS:
            raise LlmConfigurationError("role max_input_tokens exceeds its hard limit")
        if not 1 <= self.budget.max_output_tokens <= MAX_ROLE_OUTPUT_TOKENS:
            raise LlmConfigurationError("role max_output_tokens exceeds its hard limit")
        if not Decimal(0) < self.budget.max_usd <= MAX_ROLE_USD:
            raise LlmConfigurationError("role max_usd exceeds its hard limit")
        if self.max_attempts > self.budget.max_requests:
            raise LlmConfigurationError("role max_attempts exceeds its request budget")

    @classmethod
    def from_env(
        cls,
        role: LlmRole | str,
        environ: Mapping[str, str] | None = None,
    ) -> LlmRoleConfig:
        role = LlmRole(role)
        source = os.environ if environ is None else environ
        prefix = role.env_prefix

        def required(suffix: str) -> str:
            name = f"{prefix}_{suffix}"
            value = source.get(name)
            if value is None or not value:
                raise LlmConfigurationError(f"missing required {name}")
            if value != value.strip():
                raise LlmConfigurationError(f"{name} must not contain surrounding whitespace")
            return value

        provider = required("PROVIDER")
        if provider != "deepseek":
            raise LlmConfigurationError(f"{prefix}_PROVIDER must be exactly deepseek")
        api_base = required("API_BASE")
        if api_base != DEEPSEEK_API_BASE:
            raise LlmConfigurationError(
                f"{prefix}_API_BASE is not the canonical DeepSeek OpenAI endpoint"
            )
        model = required("MODEL")
        if model not in DEEPSEEK_MODELS:
            raise LlmConfigurationError(f"{prefix}_MODEL is not an allowed exact DeepSeek model")
        api_key = required("API_KEY")

        timeout_seconds = _positive_float(required("TIMEOUT_SECONDS"), "TIMEOUT_SECONDS")
        max_tokens = _positive_int(required("MAX_TOKENS"), "MAX_TOKENS")
        max_attempts = _positive_int(required("MAX_ATTEMPTS"), "MAX_ATTEMPTS")
        if max_attempts > MAX_ROLE_ATTEMPTS:
            raise LlmConfigurationError(
                f"{prefix}_MAX_ATTEMPTS must not exceed {MAX_ROLE_ATTEMPTS}"
            )
        thinking = required("THINKING")
        if thinking not in {"enabled", "disabled"}:
            raise LlmConfigurationError(f"{prefix}_THINKING must be enabled or disabled")
        raw_effort = required("REASONING_EFFORT")
        raw_temperature = required("TEMPERATURE")
        raw_top_p = required("TOP_P")
        if thinking == "enabled":
            if raw_effort not in {"high", "max"}:
                raise LlmConfigurationError(
                    f"{prefix}_REASONING_EFFORT must be high or max in thinking mode"
                )
            if raw_temperature != "none" or raw_top_p != "none":
                raise LlmConfigurationError(
                    f"{prefix}_TEMPERATURE and {prefix}_TOP_P must be none in thinking mode"
                )
            reasoning_effort = raw_effort
            temperature = None
            top_p = None
        else:
            if raw_effort != "none":
                raise LlmConfigurationError(
                    f"{prefix}_REASONING_EFFORT must be none in non-thinking mode"
                )
            reasoning_effort = None
            temperature = _bounded_float(raw_temperature, "TEMPERATURE", 0.0, 2.0)
            top_p = _bounded_float(raw_top_p, "TOP_P", 0.0, 1.0, exclusive_min=True)
        max_requests = _positive_int(required("MAX_REQUESTS"), "MAX_REQUESTS")
        max_input_tokens = _positive_int(required("MAX_INPUT_TOKENS"), "MAX_INPUT_TOKENS")
        max_output_tokens = _positive_int(required("MAX_OUTPUT_TOKENS"), "MAX_OUTPUT_TOKENS")
        max_usd = _positive_decimal(required("MAX_USD"), "MAX_USD")
        if max_requests > MAX_ROLE_REQUESTS:
            raise LlmConfigurationError(
                f"{prefix}_MAX_REQUESTS exceeds the bounded journal inventory"
            )
        if max_input_tokens > MAX_ROLE_INPUT_TOKENS:
            raise LlmConfigurationError(f"{prefix}_MAX_INPUT_TOKENS exceeds the hard limit")
        if max_output_tokens > MAX_ROLE_OUTPUT_TOKENS:
            raise LlmConfigurationError(f"{prefix}_MAX_OUTPUT_TOKENS exceeds the hard limit")
        if max_usd > MAX_ROLE_USD:
            raise LlmConfigurationError(f"{prefix}_MAX_USD exceeds the hard limit")
        if max_tokens > max_output_tokens:
            raise LlmConfigurationError(
                f"{prefix}_MAX_TOKENS must not exceed {prefix}_MAX_OUTPUT_TOKENS"
            )
        if max_attempts > max_requests:
            raise LlmConfigurationError(
                f"{prefix}_MAX_ATTEMPTS must not exceed {prefix}_MAX_REQUESTS"
            )
        return cls(
            role=role,
            provider=provider,
            api_base=api_base,
            model=model,
            api_key=api_key,
            timeout_seconds=timeout_seconds,
            max_tokens=max_tokens,
            max_attempts=max_attempts,
            thinking=thinking,
            reasoning_effort=reasoning_effort,
            temperature=temperature,
            top_p=top_p,
            budget=RoleBudget(
                max_requests=max_requests,
                max_input_tokens=max_input_tokens,
                max_output_tokens=max_output_tokens,
                max_usd=max_usd,
            ),
        )


@dataclass(frozen=True)
class TokenPrices:
    cache_hit_per_million: Decimal
    cache_miss_per_million: Decimal
    output_per_million: Decimal


@dataclass(frozen=True)
class PriceSelection:
    pricing_schedule_id: str
    effective_at: str
    review_not_after: str
    billing_band: str
    prices: TokenPrices
    source_url: str
    source_retrieved_at: str
    source_reviewed_at: str
    local_frozen_schedule_record_sha256: str


_OLD_SCHEDULE_EFFECTIVE_AT = datetime(2026, 7, 31, 0, 0, tzinfo=UTC)
_NEW_SCHEDULE_EFFECTIVE_AT = datetime(2026, 8, 16, 16, 0, tzinfo=UTC)
_NEW_SCHEDULE_REVIEW_NOT_AFTER = datetime(2026, 9, 15, 16, 0, tzinfo=UTC)
_PRICE_SOURCE_URL = "https://api-docs.deepseek.com/quick_start/pricing/"
_PRICE_SOURCE_RETRIEVED_AT = "2026-08-14T17:14:17Z"
_PRICE_SOURCE_REVIEWED_AT = "2026-08-14T17:14:17Z"
_OLD_PRICES = {
    "deepseek-v4-flash": TokenPrices(Decimal("0.0028"), Decimal("0.14"), Decimal("0.28")),
    "deepseek-v4-pro": TokenPrices(Decimal("0.003625"), Decimal("0.435"), Decimal("0.87")),
}
_NEW_OFF_PEAK_PRICES = {
    "deepseek-v4-flash": TokenPrices(Decimal("0.007"), Decimal("0.22"), Decimal("0.66")),
    "deepseek-v4-pro": TokenPrices(Decimal("0.022"), Decimal("0.66"), Decimal("1.98")),
}
_NEW_PEAK_PRICES = {
    "deepseek-v4-flash": TokenPrices(Decimal("0.014"), Decimal("0.44"), Decimal("1.32")),
    "deepseek-v4-pro": TokenPrices(Decimal("0.044"), Decimal("1.32"), Decimal("3.96")),
}


def _pricing_policy_sha256() -> str:
    record = {
        "schema": "plico.benchmark.deepseek-pricing-policy/v1",
        "source_url": _PRICE_SOURCE_URL,
        "source_retrieved_at": _PRICE_SOURCE_RETRIEVED_AT,
        "source_reviewed_at": _PRICE_SOURCE_REVIEWED_AT,
        "old_effective_at": _OLD_SCHEDULE_EFFECTIVE_AT.isoformat(),
        "new_effective_at": _NEW_SCHEDULE_EFFECTIVE_AT.isoformat(),
        "new_review_not_after": _NEW_SCHEDULE_REVIEW_NOT_AFTER.isoformat(),
        "old_prices": _prices_record(_OLD_PRICES),
        "new_off_peak_prices": _prices_record(_NEW_OFF_PEAK_PRICES),
        "new_peak_prices": _prices_record(_NEW_PEAK_PRICES),
    }
    payload = json.dumps(record, sort_keys=True, separators=(",", ":")).encode()
    return hashlib.sha256(payload).hexdigest()


def _prices_record(prices: Mapping[str, TokenPrices]) -> dict[str, dict[str, str]]:
    return {
        model: {
            "cache_hit_per_million_usd": format(value.cache_hit_per_million, "f"),
            "cache_miss_per_million_usd": format(value.cache_miss_per_million, "f"),
            "output_per_million_usd": format(value.output_per_million, "f"),
        }
        for model, value in sorted(prices.items())
    }


DEEPSEEK_PRICING_POLICY_SHA256 = _pricing_policy_sha256()


def select_deepseek_price(model: str, at: datetime) -> PriceSelection:
    """Select the versioned USD schedule using an aware UTC instant."""
    if model not in DEEPSEEK_MODELS:
        raise LlmPricingError("no price for unapproved model")
    if at.tzinfo is None:
        raise LlmPricingError("pricing instant must be timezone-aware")
    at = at.astimezone(UTC)
    if at < _OLD_SCHEDULE_EFFECTIVE_AT:
        raise LlmPricingError("no verified DeepSeek price schedule covers this instant")
    if at < _NEW_SCHEDULE_EFFECTIVE_AT:
        schedule_id = "deepseek-v4-0731-usd-2026-07-31"
        prices = _OLD_PRICES[model]
        return _price_selection(
            schedule_id=schedule_id,
            model=model,
            effective_at="2026-07-31T00:00:00Z",
            review_not_after="2026-08-16T16:00:00Z",
            billing_band="standard",
            prices=prices,
        )

    if at >= _NEW_SCHEDULE_REVIEW_NOT_AFTER:
        raise LlmPricingError("verified DeepSeek price schedule is stale")
    current = at.time().replace(tzinfo=None)
    is_peak = time(1, 0) <= current < time(4, 0) or time(6, 0) <= current < time(10, 0)
    schedule_id = "deepseek-v4-usd-2026-08-16"
    band = "peak" if is_peak else "off_peak"
    prices = (_NEW_PEAK_PRICES if is_peak else _NEW_OFF_PEAK_PRICES)[model]
    return _price_selection(
        schedule_id=schedule_id,
        model=model,
        effective_at="2026-08-16T16:00:00Z",
        review_not_after="2026-09-15T16:00:00Z",
        billing_band=band,
        prices=prices,
    )


def select_deepseek_interval_price(
    model: str, started_at: datetime, completed_at: datetime
) -> PriceSelection:
    """Charge each token at the highest verified rate touched by an interval."""
    if started_at.tzinfo is None or completed_at.tzinfo is None:
        raise LlmPricingError("pricing interval must use timezone-aware instants")
    started_at = started_at.astimezone(UTC)
    completed_at = completed_at.astimezone(UTC)
    if completed_at < started_at:
        raise LlmPricingError("pricing interval completion precedes its start")
    if completed_at - started_at > timedelta(minutes=10):
        raise LlmPricingError("pricing interval exceeds the bounded request timeout")

    checkpoints = {started_at, completed_at}
    if started_at < _NEW_SCHEDULE_EFFECTIVE_AT <= completed_at:
        checkpoints.add(_NEW_SCHEDULE_EFFECTIVE_AT)
    cursor = started_at.date()
    while cursor <= completed_at.date():
        for hour in (1, 4, 6, 10):
            boundary = datetime.combine(cursor, time(hour, 0), tzinfo=UTC)
            if started_at < boundary <= completed_at:
                checkpoints.add(boundary)
        cursor += timedelta(days=1)
    selections = [select_deepseek_price(model, instant) for instant in sorted(checkpoints)]
    unique = {
        (selection.pricing_schedule_id, selection.billing_band): selection
        for selection in selections
    }
    if len(unique) == 1:
        return next(iter(unique.values()))

    involved = list(unique.values())
    prices = TokenPrices(
        cache_hit_per_million=max(selection.prices.cache_hit_per_million for selection in involved),
        cache_miss_per_million=max(
            selection.prices.cache_miss_per_million for selection in involved
        ),
        output_per_million=max(selection.prices.output_per_million for selection in involved),
    )
    schedule_ids = sorted({selection.pricing_schedule_id for selection in involved})
    bands = sorted({selection.billing_band for selection in involved})
    return _price_selection(
        schedule_id=f"max_of[{','.join(schedule_ids)}]",
        model=model,
        effective_at=min(selection.effective_at for selection in involved),
        review_not_after=max(selection.review_not_after for selection in involved),
        billing_band=f"max_of[{','.join(bands)}]",
        prices=prices,
    )


def _price_selection(
    *,
    schedule_id: str,
    model: str,
    effective_at: str,
    review_not_after: str,
    billing_band: str,
    prices: TokenPrices,
) -> PriceSelection:
    return PriceSelection(
        pricing_schedule_id=schedule_id,
        effective_at=effective_at,
        review_not_after=review_not_after,
        billing_band=billing_band,
        prices=prices,
        source_url=_PRICE_SOURCE_URL,
        source_retrieved_at=_PRICE_SOURCE_RETRIEVED_AT,
        source_reviewed_at=_PRICE_SOURCE_REVIEWED_AT,
        local_frozen_schedule_record_sha256=_price_schedule_digest(
            schedule_id,
            model,
            effective_at,
            review_not_after,
            billing_band,
            prices,
        ),
    )


def _price_schedule_digest(
    schedule_id: str,
    model: str,
    effective_at: str,
    review_not_after: str,
    billing_band: str,
    prices: TokenPrices,
) -> str:
    frozen_record = json.dumps(
        {
            "billing_band": billing_band,
            "cache_hit_per_million_usd": str(prices.cache_hit_per_million),
            "cache_miss_per_million_usd": str(prices.cache_miss_per_million),
            "effective_at": effective_at,
            "model": model,
            "output_per_million_usd": str(prices.output_per_million),
            "schedule_id": schedule_id,
            "source_retrieved_at": _PRICE_SOURCE_RETRIEVED_AT,
            "source_reviewed_at": _PRICE_SOURCE_REVIEWED_AT,
            "source_url": _PRICE_SOURCE_URL,
            "review_not_after": review_not_after,
        },
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return hashlib.sha256(frozen_record).hexdigest()


def _positive_int(raw: str, suffix: str) -> int:
    try:
        value = int(raw)
    except ValueError as error:
        raise LlmConfigurationError(f"{suffix} must be a positive integer") from error
    if value <= 0 or str(value) != raw:
        raise LlmConfigurationError(f"{suffix} must be a canonical positive integer")
    return value


def _positive_float(raw: str, suffix: str) -> float:
    try:
        value = float(raw)
    except ValueError as error:
        raise LlmConfigurationError(f"{suffix} must be a positive finite number") from error
    if value <= 0 or value == float("inf") or value != value:
        raise LlmConfigurationError(f"{suffix} must be a positive finite number")
    return value


def _bounded_float(
    raw: str,
    suffix: str,
    minimum: float,
    maximum: float,
    *,
    exclusive_min: bool = False,
) -> float:
    value = _positive_float(raw, suffix) if exclusive_min else _nonnegative_float(raw, suffix)
    if value > maximum or (exclusive_min and value <= minimum) or value < minimum:
        raise LlmConfigurationError(f"{suffix} is outside the supported range")
    return value


def _nonnegative_float(raw: str, suffix: str) -> float:
    try:
        value = float(raw)
    except ValueError as error:
        raise LlmConfigurationError(f"{suffix} must be a finite number") from error
    if value < 0 or value == float("inf") or value != value:
        raise LlmConfigurationError(f"{suffix} must be a finite non-negative number")
    return value


def _positive_decimal(raw: str, suffix: str) -> Decimal:
    try:
        value = Decimal(raw)
    except InvalidOperation as error:
        raise LlmConfigurationError(f"{suffix} must be a positive decimal") from error
    if not value.is_finite() or value <= 0:
        raise LlmConfigurationError(f"{suffix} must be a positive finite decimal")
    return value
