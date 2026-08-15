"""Durable, owner-only journal for paid DeepSeek attempt accounting."""

from __future__ import annotations

import fcntl
import hashlib
import os
import re
import stat
import uuid
from collections.abc import Mapping
from dataclasses import dataclass
from datetime import UTC, datetime, timedelta
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any

from plico_benchmarks.core.dogfood_io import (
    canonical_json,
    open_directory_no_follow,
    strict_json_object,
)
from plico_benchmarks.core.llm_roles import (
    DEEPSEEK_API_BASE,
    DEEPSEEK_OFFICIAL_MODEL_VERSIONS,
    DEEPSEEK_PRICING_POLICY_SHA256,
    MAX_ROLE_INPUT_TOKENS,
    MAX_ROLE_OUTPUT_TOKENS,
    MAX_ROLE_REQUESTS,
    MAX_ROLE_USD,
    LlmPricingError,
    select_deepseek_interval_price,
)

RUN_ID_ENV = "PLICO_LLM_RUN_ID"
JOURNAL_DIR_ENV = "PLICO_LLM_ATTEMPT_JOURNAL_DIR"

_RUN_FILE = "RUN.json"
_LOCK_FILE = "JOURNAL.LOCK"
_COMPLETE_FILE = "RUN_COMPLETE.json"
_RUN_SCHEMA = "plico.benchmark.llm-attempt-journal-run/v1"
_PREPARED_SCHEMA = "plico.benchmark.llm-attempt-prepared/v1"
_FINALIZED_SCHEMA = "plico.benchmark.llm-attempt-finalized/v1"
_COMPLETE_SCHEMA = "plico.benchmark.llm-attempt-journal-complete/v1"
_INVENTORY_SCHEMA = "plico.benchmark.llm-attempt-journal-inventory/v1"
_ROLE_CONFIG_SCHEMA = "plico.benchmark.llm-attempt-role-config/v1"
_PREPARED_RE = re.compile(r"attempt-([0-9]{20})\.prepared\.json\Z")
_FINALIZED_RE = re.compile(r"attempt-([0-9]{20})\.finalized\.json\Z")
_PENDING_RE = re.compile(r"\.attempt-pending\.([0-9a-f]{32})\.tmp\Z")
_ROLE_CONFIG_RE = re.compile(r"RUN_ROLE\.(reader|judge|compiler)\.json\Z")
_SAFE_IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:-]{0,127}\Z")
_HEX_64 = re.compile(r"[0-9a-f]{64}\Z")
_MAX_JOURNAL_ATTEMPTS = 100_000
_MAX_ENTRY_BYTES = 128 * 1024
_DEEPSEEK_MODELS = frozenset({"deepseek-v4-flash", "deepseek-v4-pro"})
_BILLING_BANDS = frozenset(
    {
        "standard",
        "off_peak",
        "peak",
        "max_of[off_peak,peak]",
        "max_of[off_peak,standard]",
        "max_of[peak,standard]",
        "max_of[off_peak,peak,standard]",
    }
)
_PRICING_SCHEDULE_IDS = frozenset(
    {
        "deepseek-v4-0731-usd-2026-07-31",
        "deepseek-v4-usd-2026-08-16",
        "max_of[deepseek-v4-usd-2026-08-16]",
        "max_of[deepseek-v4-0731-usd-2026-07-31,deepseek-v4-usd-2026-08-16]",
    }
)
_STATUSES = frozenset(
    {
        "ok",
        "pricing_error",
        "indeterminate_transport",
        "protocol_error",
        "http_error",
        "accounting_error",
        "incomplete",
        "semantic_rejected",
    }
)
_PREPARED_FIELDS = frozenset(
    {
        "schema",
        "role",
        "role_request_id",
        "sample_id",
        "attempt_sequence",
        "attempt_in_request",
        "prompt_sha256",
        "requested_model_alias",
        "thinking",
        "reasoning_effort",
        "temperature",
        "top_p",
        "timeout_seconds",
        "max_tokens",
        "generation_seed",
        "started_at_utc",
        "reservation_pricing_schedule_id",
        "reservation_billing_band",
        "reservation_cache_hit_per_million_usd",
        "reservation_cache_miss_per_million_usd",
        "reservation_output_per_million_usd",
        "reserved_input_tokens_upper_bound",
        "reserved_output_tokens",
        "budget_max_requests",
        "budget_max_input_tokens",
        "budget_max_output_tokens",
        "budget_max_usd",
        "usd_accounted",
        "usd_basis",
    }
)
_FINAL_FIELDS = frozenset(
    {
        "schema",
        "role",
        "role_request_id",
        "sample_id",
        "attempt_sequence",
        "attempt_in_request",
        "status",
        "http_status",
        "prompt_sha256",
        "requested_model_alias",
        "official_model_version",
        "model_revision_attestation",
        "response_model",
        "system_fingerprint",
        "cross_run_comparability",
        "usage",
        "finish_reason",
        "thinking",
        "reasoning_effort",
        "temperature",
        "top_p",
        "timeout_seconds",
        "max_tokens",
        "generation_seed",
        "started_at_utc",
        "completed_at_utc",
        "latency_ms",
        "pricing_schedule_id",
        "pricing_effective_at",
        "pricing_review_not_after",
        "billing_band",
        "pricing_cache_hit_per_million_usd",
        "pricing_cache_miss_per_million_usd",
        "pricing_output_per_million_usd",
        "pricing_source_url",
        "pricing_source_retrieved_at",
        "pricing_source_reviewed_at",
        "pricing_local_frozen_schedule_record_sha256",
        "reservation_pricing_schedule_id",
        "reservation_billing_band",
        "reservation_cache_hit_per_million_usd",
        "reservation_cache_miss_per_million_usd",
        "reservation_output_per_million_usd",
        "reserved_input_tokens_upper_bound",
        "reserved_output_tokens",
        "budget_max_requests",
        "budget_max_input_tokens",
        "budget_max_output_tokens",
        "budget_max_usd",
        "usd_accounted",
        "usd_basis",
    }
)
_USAGE_FIELDS = frozenset(
    {
        "prompt_tokens",
        "prompt_cache_hit_tokens",
        "prompt_cache_miss_tokens",
        "completion_tokens",
        "total_tokens",
        "cache_accounting",
    }
)
_ROLE_CONFIG_FIELDS = frozenset(
    {
        "schema",
        "run_id",
        "role",
        "provider",
        "api_base_origin",
        "requested_model_alias",
        "official_model_version",
        "thinking",
        "reasoning_effort",
        "temperature",
        "top_p",
        "timeout_seconds",
        "max_tokens",
        "max_attempts",
        "generation_seed",
        "budget_max_requests",
        "budget_max_input_tokens",
        "budget_max_output_tokens",
        "budget_max_usd",
        "pricing_policy_sha256",
    }
)


class LlmJournalError(RuntimeError):
    """The paid-attempt journal could not prove a durable accounting state."""


@dataclass(frozen=True)
class PreparedAttempt:
    run_id: str
    sequence: int
    prepared_record_sha256: str


@dataclass(frozen=True)
class AttemptJournalEntry:
    run_id: str
    sequence: int
    phase: str
    prepared: dict[str, Any]
    finalized: dict[str, Any] | None
    prepared_record_sha256: str
    finalized_record_sha256: str | None

    @property
    def usd_accounted(self) -> str:
        source = self.prepared if self.finalized is None else self.finalized
        return str(source["usd_accounted"])


@dataclass(frozen=True)
class AttemptJournalSnapshot:
    run_id: str
    entries: tuple[AttemptJournalEntry, ...]
    role_configs: tuple[dict[str, Any], ...]
    inventory_sha256: str
    total_usd_accounted: str
    run_complete: bool
    incomplete_prepared_attempts: int
    incomplete_pending_files: int

    @property
    def attempt_count(self) -> int:
        return len(self.entries)

    @property
    def finalized_attempt_count(self) -> int:
        return self.attempt_count - self.incomplete_prepared_attempts


@dataclass(frozen=True)
class JournalRoleAccounting:
    requests: int
    input_tokens_accounted: int
    output_tokens_accounted: int
    usd_accounted: str


class AttemptJournal:
    """Append-only two-phase journal bound to one existing 0700 run directory."""

    def __init__(self, directory: Path, run_id: str):
        self._run_id = _validate_run_id(run_id)
        self._directory_fd: int | None = None
        self._lock_fd: int | None = None
        try:
            self._directory_fd = open_directory_no_follow(directory)
            _assert_private_directory(self._directory_fd)
            self._lock_fd = _open_lock(self._directory_fd)
            with _locked(self._lock_fd):
                _open_or_validate_run(self._directory_fd, self._run_id)
        except Exception as error:
            self.close()
            if isinstance(error, LlmJournalError):
                raise
            raise LlmJournalError("paid-attempt journal is unavailable") from error

    @classmethod
    def from_env(cls, environ: Mapping[str, str] | None = None) -> AttemptJournal:
        source = os.environ if environ is None else environ
        raw_run_id = source.get(RUN_ID_ENV)
        raw_directory = source.get(JOURNAL_DIR_ENV)
        if raw_run_id is None or not raw_run_id:
            raise LlmJournalError(f"missing required {RUN_ID_ENV}")
        if raw_directory is None or not raw_directory:
            raise LlmJournalError(f"missing required {JOURNAL_DIR_ENV}")
        if raw_run_id != raw_run_id.strip() or raw_directory != raw_directory.strip():
            raise LlmJournalError("paid-attempt journal configuration contains whitespace")
        directory = Path(raw_directory)
        if not directory.is_absolute():
            raise LlmJournalError("paid-attempt journal directory must be absolute")
        return cls(directory, raw_run_id)

    @property
    def run_id(self) -> str:
        return self._run_id

    def register_role_config(self, config: Mapping[str, Any]) -> None:
        """Freeze one role's complete non-secret provider and budget policy."""
        directory_fd, lock_fd = self._descriptors()
        record = dict(config)
        record["run_id"] = self._run_id
        _validate_role_config(record, self._run_id)
        role = str(record["role"])
        name = f"RUN_ROLE.{role}.json"
        expected = _bounded_canonical(record)
        try:
            with _locked(lock_fd):
                snapshot = _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=True,
                )
                existing = next(
                    (item for item in snapshot.role_configs if item["role"] == role), None
                )
                if existing is not None:
                    if existing != record:
                        raise LlmJournalError(
                            "paid-attempt role configuration changed within one run"
                        )
                    return
                if snapshot.run_complete:
                    raise LlmJournalError("paid-attempt journal is already complete")
                _persist_no_clobber(directory_fd, name, expected)
        except LlmJournalError:
            raise
        except Exception as error:
            raise LlmJournalError("paid-attempt role policy durability is indeterminate") from error

    def role_accounting(self, role: str) -> JournalRoleAccounting:
        if role not in {"reader", "judge", "compiler"}:
            raise LlmJournalError("paid-attempt role is invalid")
        snapshot = self.snapshot()
        return _role_accounting(snapshot.entries, role)

    def assert_can_start_attempt(self) -> None:
        snapshot = self.snapshot()
        if snapshot.run_complete:
            raise LlmJournalError("paid-attempt journal is already complete")
        if snapshot.incomplete_prepared_attempts:
            raise LlmJournalError("paid-attempt journal has an indeterminate prepared attempt")
        if snapshot.incomplete_pending_files:
            raise LlmJournalError("paid-attempt journal has incomplete write evidence")

    def prepare(self, evidence: Mapping[str, Any]) -> PreparedAttempt:
        """Commit the worst-case reservation before any provider I/O starts."""
        directory_fd, lock_fd = self._descriptors()
        try:
            with _locked(lock_fd):
                snapshot = _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=True,
                )
                if snapshot.run_complete:
                    raise LlmJournalError("paid-attempt journal is already complete")
                if snapshot.incomplete_prepared_attempts:
                    raise LlmJournalError(
                        "paid-attempt journal has an indeterminate prepared attempt"
                    )
                sequence = snapshot.attempt_count + 1
                if sequence > _MAX_JOURNAL_ATTEMPTS:
                    raise LlmJournalError("paid-attempt journal inventory limit exceeded")
                prepared = dict(evidence)
                prepared["attempt_sequence"] = sequence
                _validate_prepared_evidence(prepared, sequence)
                _validate_durable_budget(snapshot.entries, snapshot.role_configs, prepared)
                previous = (
                    None if not snapshot.entries else snapshot.entries[-1].prepared_record_sha256
                )
                record = {
                    "schema": _PREPARED_SCHEMA,
                    "run_id": self._run_id,
                    "sequence": sequence,
                    "previous_prepared_record_sha256": previous,
                    "prepared": prepared,
                }
                payload = _bounded_canonical(record)
                name = f"attempt-{sequence:020d}.prepared.json"
                _persist_no_clobber(directory_fd, name, payload)
                return PreparedAttempt(
                    run_id=self._run_id,
                    sequence=sequence,
                    prepared_record_sha256=hashlib.sha256(payload).hexdigest(),
                )
        except LlmJournalError:
            raise
        except Exception as error:
            raise LlmJournalError("paid-attempt reservation durability is indeterminate") from error

    def finalize(
        self,
        prepared_attempt: PreparedAttempt,
        evidence: Mapping[str, Any],
    ) -> dict[str, Any]:
        """Commit final accounting before returning or raising from a paid attempt."""
        directory_fd, lock_fd = self._descriptors()
        if prepared_attempt.run_id != self._run_id:
            raise LlmJournalError("prepared attempt belongs to a different run")
        try:
            with _locked(lock_fd):
                snapshot = _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=True,
                )
                if snapshot.run_complete:
                    raise LlmJournalError("paid-attempt journal is already complete")
                if prepared_attempt.sequence <= 0 or prepared_attempt.sequence > len(
                    snapshot.entries
                ):
                    raise LlmJournalError("prepared attempt sequence is absent")
                entry = snapshot.entries[prepared_attempt.sequence - 1]
                if (
                    entry.prepared_record_sha256 != prepared_attempt.prepared_record_sha256
                    or entry.finalized is not None
                ):
                    raise LlmJournalError("prepared attempt identity or phase is invalid")
                finalized = dict(evidence)
                finalized["attempt_sequence"] = prepared_attempt.sequence
                _validate_final_evidence(finalized, entry.prepared)
                record = {
                    "schema": _FINALIZED_SCHEMA,
                    "run_id": self._run_id,
                    "sequence": prepared_attempt.sequence,
                    "prepared_record_sha256": prepared_attempt.prepared_record_sha256,
                    "evidence": finalized,
                }
                payload = _bounded_canonical(record)
                name = f"attempt-{prepared_attempt.sequence:020d}.finalized.json"
                _persist_no_clobber(directory_fd, name, payload)
                return finalized
        except LlmJournalError:
            raise
        except Exception as error:
            raise LlmJournalError("paid-attempt final accounting is indeterminate") from error

    def snapshot(self) -> AttemptJournalSnapshot:
        directory_fd, lock_fd = self._descriptors()
        try:
            with _locked(lock_fd, exclusive=False):
                return _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=False,
                )
        except LlmJournalError:
            raise
        except Exception as error:
            raise LlmJournalError("paid-attempt journal cannot be verified") from error

    def mark_complete(self) -> AttemptJournalSnapshot:
        """Seal an exact finalized inventory; absence means the run is incomplete."""
        directory_fd, lock_fd = self._descriptors()
        try:
            with _locked(lock_fd):
                snapshot = _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=True,
                )
                if snapshot.run_complete:
                    return snapshot
                if snapshot.incomplete_prepared_attempts:
                    raise LlmJournalError("prepared paid attempts remain indeterminate")
                marker = _bounded_canonical(
                    {
                        "schema": _COMPLETE_SCHEMA,
                        "run_id": self._run_id,
                        "attempt_count": snapshot.attempt_count,
                        "inventory_sha256": snapshot.inventory_sha256,
                        "total_usd_accounted": snapshot.total_usd_accounted,
                    }
                )
                _persist_no_clobber(directory_fd, _COMPLETE_FILE, marker)
                return _read_snapshot_at(
                    directory_fd,
                    expected_run_id=self._run_id,
                    require_no_pending=True,
                )
        except LlmJournalError:
            raise
        except Exception as error:
            raise LlmJournalError("paid-attempt completion is indeterminate") from error

    def close(self) -> None:
        if self._lock_fd is not None:
            os.close(self._lock_fd)
            self._lock_fd = None
        if self._directory_fd is not None:
            os.close(self._directory_fd)
            self._directory_fd = None

    def _descriptors(self) -> tuple[int, int]:
        if self._directory_fd is None or self._lock_fd is None:
            raise LlmJournalError("paid-attempt journal is closed")
        return self._directory_fd, self._lock_fd

    def __del__(self) -> None:
        self.close()


def read_attempt_journal(directory: Path, expected_run_id: str) -> AttemptJournalSnapshot:
    journal = AttemptJournal(directory, expected_run_id)
    try:
        return journal.snapshot()
    finally:
        journal.close()


def mark_attempt_journal_complete(directory: Path, expected_run_id: str) -> AttemptJournalSnapshot:
    journal = AttemptJournal(directory, expected_run_id)
    try:
        return journal.mark_complete()
    finally:
        journal.close()


class _locked:
    def __init__(self, descriptor: int, *, exclusive: bool = True):
        self._descriptor = descriptor
        self._operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH

    def __enter__(self) -> None:
        fcntl.flock(self._descriptor, self._operation)

    def __exit__(self, _kind: object, _value: object, _traceback: object) -> None:
        fcntl.flock(self._descriptor, fcntl.LOCK_UN)


def _validate_run_id(value: str) -> str:
    try:
        parsed = uuid.UUID(value)
    except (ValueError, AttributeError) as error:
        raise LlmJournalError("paid-attempt run ID is not a canonical UUID") from error
    if parsed.version != 4 or str(parsed) != value:
        raise LlmJournalError("paid-attempt run ID is not a canonical v4 UUID")
    return value


def _assert_private_directory(descriptor: int) -> None:
    metadata = os.fstat(descriptor)
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise LlmJournalError("paid-attempt journal directory must be owner-only mode 0700")


def _open_lock(directory_fd: int) -> int:
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(_LOCK_FILE, flags, 0o600, dir_fd=directory_fd)
    metadata = os.fstat(descriptor)
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) != 0o600
    ):
        os.close(descriptor)
        raise LlmJournalError("paid-attempt journal lock is not private")
    os.fsync(descriptor)
    os.fsync(directory_fd)
    return descriptor


def _open_or_validate_run(directory_fd: int, run_id: str) -> None:
    expected = canonical_json({"schema": _RUN_SCHEMA, "run_id": run_id})
    try:
        actual = _read_private_at(directory_fd, _RUN_FILE)
    except FileNotFoundError:
        _persist_no_clobber(directory_fd, _RUN_FILE, expected)
        return
    expected_record = {"schema": _RUN_SCHEMA, "run_id": run_id}
    if actual != expected or strict_json_object(actual) != expected_record:
        raise LlmJournalError("paid-attempt journal belongs to a different run")


def _read_snapshot_at(
    directory_fd: int,
    *,
    expected_run_id: str,
    require_no_pending: bool,
) -> AttemptJournalSnapshot:
    _assert_private_directory(directory_fd)
    run_payload = _read_private_at(directory_fd, _RUN_FILE)
    expected_run = {"schema": _RUN_SCHEMA, "run_id": expected_run_id}
    if (
        run_payload != canonical_json(expected_run)
        or strict_json_object(run_payload) != expected_run
    ):
        raise LlmJournalError("paid-attempt journal run evidence is invalid")

    names = {entry.name for entry in os.scandir(directory_fd)}
    prepared_names: dict[int, str] = {}
    finalized_names: dict[int, str] = {}
    role_config_names: dict[str, str] = {}
    pending = 0
    for name in names:
        prepared_match = _PREPARED_RE.fullmatch(name)
        finalized_match = _FINALIZED_RE.fullmatch(name)
        if prepared_match is not None:
            sequence = int(prepared_match.group(1))
            if sequence in prepared_names:
                raise LlmJournalError("paid-attempt journal has a duplicate sequence")
            prepared_names[sequence] = name
        elif finalized_match is not None:
            sequence = int(finalized_match.group(1))
            if sequence in finalized_names:
                raise LlmJournalError("paid-attempt journal has a duplicate finalization")
            finalized_names[sequence] = name
        elif _PENDING_RE.fullmatch(name) is not None:
            pending += 1
        elif (role_match := _ROLE_CONFIG_RE.fullmatch(name)) is not None:
            role = role_match.group(1)
            if role in role_config_names:
                raise LlmJournalError("paid-attempt journal has duplicate role policy")
            role_config_names[role] = name
        elif name not in {_RUN_FILE, _LOCK_FILE, _COMPLETE_FILE}:
            raise LlmJournalError("paid-attempt journal contains an unexpected entry")
    if pending and require_no_pending:
        raise LlmJournalError("paid-attempt journal contains incomplete write evidence")
    if len(prepared_names) > _MAX_JOURNAL_ATTEMPTS or pending > _MAX_JOURNAL_ATTEMPTS:
        raise LlmJournalError("paid-attempt journal inventory limit exceeded")
    if set(prepared_names) != set(range(1, len(prepared_names) + 1)):
        raise LlmJournalError("paid-attempt journal sequence is not contiguous")
    if not set(finalized_names).issubset(prepared_names):
        raise LlmJournalError("paid-attempt finalization lacks a reservation")

    role_configs: list[dict[str, Any]] = []
    role_config_hashes: list[dict[str, str]] = []
    for role, name in sorted(role_config_names.items()):
        payload = _read_private_at(directory_fd, name)
        record = strict_json_object(payload)
        if payload != canonical_json(record):
            raise LlmJournalError("paid-attempt role policy is not canonical")
        _validate_role_config(record, expected_run_id)
        if record["role"] != role:
            raise LlmJournalError("paid-attempt role policy locator is invalid")
        role_configs.append(record)
        role_config_hashes.append(
            {"role": role, "record_sha256": hashlib.sha256(payload).hexdigest()}
        )

    entries: list[AttemptJournalEntry] = []
    inventory: list[dict[str, Any]] = []
    previous_prepared_hash: str | None = None
    total_usd = Decimal(0)
    incomplete_prepared = 0
    for sequence in range(1, len(prepared_names) + 1):
        prepared_payload = _read_private_at(directory_fd, prepared_names[sequence])
        prepared_record = strict_json_object(prepared_payload)
        if prepared_payload != canonical_json(prepared_record):
            raise LlmJournalError("paid-attempt reservation is not canonical")
        if set(prepared_record) != {
            "schema",
            "run_id",
            "sequence",
            "previous_prepared_record_sha256",
            "prepared",
        }:
            raise LlmJournalError("paid-attempt reservation has unknown fields")
        prepared = prepared_record.get("prepared")
        if (
            prepared_record.get("schema") != _PREPARED_SCHEMA
            or prepared_record.get("run_id") != expected_run_id
            or prepared_record.get("sequence") != sequence
            or prepared_record.get("previous_prepared_record_sha256") != previous_prepared_hash
            or not isinstance(prepared, dict)
        ):
            raise LlmJournalError("paid-attempt reservation binding is invalid")
        _validate_prepared_evidence(prepared, sequence)
        _validate_prepared_against_role_config(prepared, role_configs)
        prepared_hash = hashlib.sha256(prepared_payload).hexdigest()
        finalized: dict[str, Any] | None = None
        finalized_hash: str | None = None
        finalized_name = finalized_names.get(sequence)
        if finalized_name is not None:
            finalized_payload = _read_private_at(directory_fd, finalized_name)
            finalized_record = strict_json_object(finalized_payload)
            if finalized_payload != canonical_json(finalized_record):
                raise LlmJournalError("paid-attempt finalization is not canonical")
            if set(finalized_record) != {
                "schema",
                "run_id",
                "sequence",
                "prepared_record_sha256",
                "evidence",
            }:
                raise LlmJournalError("paid-attempt finalization has unknown fields")
            finalized = finalized_record.get("evidence")
            if (
                finalized_record.get("schema") != _FINALIZED_SCHEMA
                or finalized_record.get("run_id") != expected_run_id
                or finalized_record.get("sequence") != sequence
                or finalized_record.get("prepared_record_sha256") != prepared_hash
                or not isinstance(finalized, dict)
            ):
                raise LlmJournalError("paid-attempt finalization binding is invalid")
            _validate_final_evidence(finalized, prepared)
            finalized_hash = hashlib.sha256(finalized_payload).hexdigest()
        else:
            incomplete_prepared += 1
        cost_source = prepared if finalized is None else finalized
        total_usd += _decimal(cost_source.get("usd_accounted"), "usd_accounted")
        inventory.append(
            {
                "sequence": sequence,
                "prepared_record_sha256": prepared_hash,
                "finalized_record_sha256": finalized_hash,
            }
        )
        entries.append(
            AttemptJournalEntry(
                run_id=expected_run_id,
                sequence=sequence,
                phase="prepared_indeterminate" if finalized is None else "finalized",
                prepared=prepared,
                finalized=finalized,
                prepared_record_sha256=prepared_hash,
                finalized_record_sha256=finalized_hash,
            )
        )
        previous_prepared_hash = prepared_hash

    inventory_payload = canonical_json(
        {
            "schema": _INVENTORY_SCHEMA,
            "run_id": expected_run_id,
            "role_configs": role_config_hashes,
            "attempts": inventory,
        }
    )
    inventory_sha256 = hashlib.sha256(inventory_payload).hexdigest()
    total_usd_text = format(total_usd, "f")
    complete = _COMPLETE_FILE in names
    if complete:
        if incomplete_prepared or pending:
            raise LlmJournalError("completed paid-attempt journal is incomplete")
        complete_payload = _read_private_at(directory_fd, _COMPLETE_FILE)
        expected_complete = {
            "schema": _COMPLETE_SCHEMA,
            "run_id": expected_run_id,
            "attempt_count": len(entries),
            "inventory_sha256": inventory_sha256,
            "total_usd_accounted": total_usd_text,
        }
        if (
            complete_payload != canonical_json(expected_complete)
            or strict_json_object(complete_payload) != expected_complete
        ):
            raise LlmJournalError("paid-attempt completion marker is invalid")
    return AttemptJournalSnapshot(
        run_id=expected_run_id,
        entries=tuple(entries),
        role_configs=tuple(role_configs),
        inventory_sha256=inventory_sha256,
        total_usd_accounted=total_usd_text,
        run_complete=complete,
        incomplete_prepared_attempts=incomplete_prepared,
        incomplete_pending_files=pending,
    )


def _validate_prepared_evidence(value: Mapping[str, Any], sequence: int) -> None:
    if set(value) != _PREPARED_FIELDS:
        raise LlmJournalError("paid-attempt reservation has missing or unknown evidence fields")
    if value.get("schema") != "plico.benchmark.llm-attempt-reservation/v1":
        raise LlmJournalError("paid-attempt reservation schema is unsupported")
    _validate_common(value, sequence)
    if value.get("usd_basis") != "reserved_upper_bound":
        raise LlmJournalError("paid-attempt reservation cost basis is invalid")
    expected = _reserved_cost(value)
    if _decimal(value.get("usd_accounted"), "usd_accounted") != expected:
        raise LlmJournalError("paid-attempt reserved cost is inconsistent")


def _validate_role_config(value: Mapping[str, Any], run_id: str) -> None:
    if set(value) != _ROLE_CONFIG_FIELDS:
        raise LlmJournalError("paid-attempt role policy has missing or unknown fields")
    if value.get("schema") != _ROLE_CONFIG_SCHEMA or value.get("run_id") != run_id:
        raise LlmJournalError("paid-attempt role policy schema or run binding is invalid")
    role = value.get("role")
    model = value.get("requested_model_alias")
    if role not in {"reader", "judge", "compiler"} or model not in _DEEPSEEK_MODELS:
        raise LlmJournalError("paid-attempt role policy identity is invalid")
    if value.get("provider") != "deepseek" or value.get("api_base_origin") != DEEPSEEK_API_BASE:
        raise LlmJournalError("paid-attempt role policy provider is invalid")
    if value.get("official_model_version") != DEEPSEEK_OFFICIAL_MODEL_VERSIONS[model]:
        raise LlmJournalError("paid-attempt role policy model version is invalid")
    if value.get("pricing_policy_sha256") != DEEPSEEK_PRICING_POLICY_SHA256:
        raise LlmJournalError("paid-attempt role policy pricing digest is invalid")
    common = {
        "role": role,
        "role_request_id": "policy-validation",
        "sample_id": None,
        "attempt_sequence": 1,
        "attempt_in_request": 1,
        "prompt_sha256": "0" * 64,
        "requested_model_alias": model,
        "thinking": value.get("thinking"),
        "reasoning_effort": value.get("reasoning_effort"),
        "temperature": value.get("temperature"),
        "top_p": value.get("top_p"),
        "timeout_seconds": value.get("timeout_seconds"),
        "max_tokens": value.get("max_tokens"),
        "generation_seed": value.get("generation_seed"),
        "started_at_utc": "2026-08-15T00:00:00.000000Z",
        "reservation_pricing_schedule_id": "deepseek-v4-0731-usd-2026-07-31",
        "reservation_billing_band": "standard",
        "reservation_cache_hit_per_million_usd": "0",
        "reservation_cache_miss_per_million_usd": "0",
        "reservation_output_per_million_usd": "0",
        "reserved_input_tokens_upper_bound": 1,
        "reserved_output_tokens": value.get("max_tokens"),
        "budget_max_requests": value.get("budget_max_requests"),
        "budget_max_input_tokens": value.get("budget_max_input_tokens"),
        "budget_max_output_tokens": value.get("budget_max_output_tokens"),
        "budget_max_usd": value.get("budget_max_usd"),
    }
    # Reuse the common configuration bounds without treating policy data as an attempt.
    _validate_common_configuration(common)
    max_attempts = value.get("max_attempts")
    if (
        not isinstance(max_attempts, int)
        or isinstance(max_attempts, bool)
        or not 1 <= max_attempts <= 3
        or max_attempts > value["budget_max_requests"]
    ):
        raise LlmJournalError("paid-attempt role retry policy is invalid")


def _validate_prepared_against_role_config(
    prepared: Mapping[str, Any], role_configs: list[dict[str, Any]] | tuple[dict[str, Any], ...]
) -> None:
    policy = next((item for item in role_configs if item["role"] == prepared["role"]), None)
    if policy is None:
        raise LlmJournalError("paid-attempt reservation lacks its frozen role policy")
    exact = {
        "requested_model_alias": "requested_model_alias",
        "thinking": "thinking",
        "reasoning_effort": "reasoning_effort",
        "temperature": "temperature",
        "top_p": "top_p",
        "timeout_seconds": "timeout_seconds",
        "generation_seed": "generation_seed",
        "budget_max_requests": "budget_max_requests",
        "budget_max_input_tokens": "budget_max_input_tokens",
        "budget_max_output_tokens": "budget_max_output_tokens",
        "budget_max_usd": "budget_max_usd",
    }
    if any(prepared[field] != policy[policy_field] for field, policy_field in exact.items()):
        raise LlmJournalError("paid-attempt reservation differs from its frozen role policy")
    if prepared["max_tokens"] > policy["max_tokens"]:
        raise LlmJournalError("paid-attempt output exceeds its frozen role policy")
    if prepared["attempt_in_request"] > policy["max_attempts"]:
        raise LlmJournalError("paid-attempt retry exceeds its frozen role policy")


def _validate_final_evidence(value: Mapping[str, Any], prepared: Mapping[str, Any]) -> None:
    if set(value) != _FINAL_FIELDS:
        raise LlmJournalError("paid-attempt finalization has missing or unknown evidence fields")
    if value.get("schema") != "plico.benchmark.llm-attempt-evidence/v1":
        raise LlmJournalError("paid-attempt finalization schema is unsupported")
    sequence = prepared["attempt_sequence"]
    if not isinstance(sequence, int):
        raise LlmJournalError("paid-attempt sequence is invalid")
    _validate_common(value, sequence)
    for field in _PREPARED_FIELDS - {"schema", "usd_accounted", "usd_basis"}:
        if value.get(field) != prepared.get(field):
            raise LlmJournalError("paid-attempt finalization changed its reservation")
    status = value.get("status")
    if status not in _STATUSES:
        raise LlmJournalError("paid-attempt status is invalid")
    http_status = value.get("http_status")
    if http_status is not None and (
        not isinstance(http_status, int)
        or isinstance(http_status, bool)
        or not 100 <= http_status <= 599
    ):
        raise LlmJournalError("paid-attempt HTTP status is invalid")
    for field in (
        "official_model_version",
        "response_model",
        "system_fingerprint",
        "finish_reason",
    ):
        item = value.get(field)
        if item is not None and (not isinstance(item, str) or not _SAFE_IDENTIFIER.fullmatch(item)):
            raise LlmJournalError("paid-attempt response identity is unsafe")
    if value.get("official_model_version") not in {
        None,
        "DeepSeek-V4-Flash-0731",
        "DeepSeek-V4-Pro-0813",
    }:
        raise LlmJournalError("paid-attempt model version is invalid")
    if value.get("model_revision_attestation") not in {
        "attested_exact_version",
        "unattested_alias",
        "unattested_mismatch",
        "unattested_no_response",
    }:
        raise LlmJournalError("paid-attempt model attestation is invalid")
    if value.get("cross_run_comparability") not in {
        "requires_five_run_variance_ci",
        "requires_same_system_fingerprint_and_five_run_variance_ci",
        "not_comparable_model_mismatch",
        "not_comparable_no_response",
    }:
        raise LlmJournalError("paid-attempt comparability is invalid")
    _validate_model_attestation(value)
    latency = value.get("latency_ms")
    if (
        not isinstance(latency, (int, float))
        or isinstance(latency, bool)
        or latency < 0
        or latency > 3_600_000
    ):
        raise LlmJournalError("paid-attempt latency is invalid")
    _validate_pricing(value)
    usage = _validate_usage(value.get("usage"))
    basis = value.get("usd_basis")
    if basis == "actual_usage":
        if usage is None:
            raise LlmJournalError("paid-attempt actual cost lacks usage")
        expected = (
            Decimal(usage["prompt_cache_hit_tokens"])
            * _decimal(value.get("pricing_cache_hit_per_million_usd"), "cache hit price")
            + Decimal(usage["prompt_cache_miss_tokens"])
            * _decimal(value.get("pricing_cache_miss_per_million_usd"), "cache miss price")
            + Decimal(usage["completion_tokens"])
            * _decimal(value.get("pricing_output_per_million_usd"), "output price")
        ) / Decimal(1_000_000)
    elif basis == "reserved_upper_bound":
        expected = _reserved_cost(value)
    else:
        raise LlmJournalError("paid-attempt final cost basis is invalid")
    if _decimal(value.get("usd_accounted"), "usd_accounted") != expected:
        raise LlmJournalError("paid-attempt final cost is inconsistent")
    _validate_status_matrix(value, usage)


def _validate_common(value: Mapping[str, Any], sequence: int) -> None:
    if value.get("attempt_sequence") != sequence:
        raise LlmJournalError("paid-attempt sequence is invalid")
    if value.get("role") not in {"reader", "judge", "compiler"}:
        raise LlmJournalError("paid-attempt role is invalid")
    for field in ("role_request_id", "sample_id"):
        item = value.get(field)
        if field == "sample_id" and item is None:
            continue
        if not isinstance(item, str) or not _SAFE_IDENTIFIER.fullmatch(item):
            raise LlmJournalError("paid-attempt correlation is unsafe")
    attempt_in_request = value.get("attempt_in_request")
    if (
        not isinstance(attempt_in_request, int)
        or isinstance(attempt_in_request, bool)
        or not 1 <= attempt_in_request <= 3
    ):
        raise LlmJournalError("paid-attempt retry ordinal is invalid")
    if not isinstance(value.get("prompt_sha256"), str) or not _HEX_64.fullmatch(
        value["prompt_sha256"]
    ):
        raise LlmJournalError("paid-attempt prompt digest is invalid")
    _validate_common_configuration(value)
    started_at = _utc_instant(value.get("started_at_utc"), "start")
    _validate_reservation_pricing(value)
    try:
        reservation = select_deepseek_interval_price(
            value["requested_model_alias"],
            started_at,
            started_at + timedelta(seconds=float(value["timeout_seconds"])),
        )
    except LlmPricingError as error:
        raise LlmJournalError("paid-attempt reservation pricing is unverifiable") from error
    if not _selection_matches(value, reservation, prefix="reservation_"):
        raise LlmJournalError("paid-attempt reservation pricing is inconsistent")


def _validate_common_configuration(value: Mapping[str, Any]) -> None:
    if value.get("requested_model_alias") not in _DEEPSEEK_MODELS:
        raise LlmJournalError("paid-attempt requested model is invalid")
    thinking = value.get("thinking")
    effort = value.get("reasoning_effort")
    temperature = value.get("temperature")
    top_p = value.get("top_p")
    if thinking == "enabled":
        if effort not in {"high", "max"} or temperature is not None or top_p is not None:
            raise LlmJournalError("paid-attempt thinking configuration is invalid")
    elif thinking == "disabled":
        if effort is not None or not _bounded_number(temperature, 0, 2):
            raise LlmJournalError("paid-attempt sampling configuration is invalid")
        if not _bounded_number(top_p, 0, 1, exclusive_min=True):
            raise LlmJournalError("paid-attempt sampling configuration is invalid")
    else:
        raise LlmJournalError("paid-attempt thinking mode is invalid")
    if value.get("generation_seed") != "provider_unavailable":
        raise LlmJournalError("paid-attempt generation seed evidence is invalid")
    max_tokens = _positive_int(value.get("max_tokens"), "max_tokens")
    timeout = value.get("timeout_seconds")
    if not isinstance(timeout, (int, float)) or isinstance(timeout, bool) or not 0 < timeout <= 600:
        raise LlmJournalError("paid-attempt timeout is invalid")
    _positive_int(value.get("reserved_input_tokens_upper_bound"), "reserved input")
    reserved_output = _positive_int(value.get("reserved_output_tokens"), "reserved output")
    if reserved_output != value.get("max_tokens"):
        raise LlmJournalError("paid-attempt output reservation is inconsistent")
    if _positive_int(value.get("budget_max_requests"), "request budget") > MAX_ROLE_REQUESTS:
        raise LlmJournalError("paid-attempt request budget exceeds the hard limit")
    if (
        _positive_int(value.get("budget_max_input_tokens"), "input-token budget")
        > MAX_ROLE_INPUT_TOKENS
    ):
        raise LlmJournalError("paid-attempt input-token budget exceeds the hard limit")
    if (
        _positive_int(value.get("budget_max_output_tokens"), "output-token budget")
        > MAX_ROLE_OUTPUT_TOKENS
    ):
        raise LlmJournalError("paid-attempt output-token budget exceeds the hard limit")
    if max_tokens > value["budget_max_output_tokens"]:
        raise LlmJournalError("paid-attempt max_tokens exceeds its output-token budget")
    if _decimal(value.get("budget_max_usd"), "USD budget") > MAX_ROLE_USD:
        raise LlmJournalError("paid-attempt USD budget exceeds the hard limit")


def _validate_reservation_pricing(value: Mapping[str, Any]) -> None:
    schedule_id = value.get("reservation_pricing_schedule_id")
    if schedule_id not in _PRICING_SCHEDULE_IDS:
        raise LlmJournalError("paid-attempt reservation pricing identity is invalid")
    if value.get("reservation_billing_band") not in _BILLING_BANDS:
        raise LlmJournalError("paid-attempt reservation pricing band is invalid")
    for field in (
        "reservation_cache_hit_per_million_usd",
        "reservation_cache_miss_per_million_usd",
        "reservation_output_per_million_usd",
    ):
        _decimal(value.get(field), field)


def _validate_pricing(value: Mapping[str, Any]) -> None:
    schedule_id = value.get("pricing_schedule_id")
    if schedule_id not in _PRICING_SCHEDULE_IDS:
        raise LlmJournalError("paid-attempt pricing identity is invalid")
    if value.get("billing_band") not in _BILLING_BANDS:
        raise LlmJournalError("paid-attempt pricing band is invalid")
    for field in (
        "pricing_cache_hit_per_million_usd",
        "pricing_cache_miss_per_million_usd",
        "pricing_output_per_million_usd",
    ):
        _decimal(value.get(field), field)
    for field in (
        "pricing_effective_at",
        "pricing_review_not_after",
        "pricing_source_retrieved_at",
        "pricing_source_reviewed_at",
    ):
        item = value.get(field)
        if not isinstance(item, str) or len(item) > 40 or not item.endswith("Z"):
            raise LlmJournalError("paid-attempt pricing timestamp is invalid")
    if value.get("pricing_source_url") != "https://api-docs.deepseek.com/quick_start/pricing/":
        raise LlmJournalError("paid-attempt pricing source is invalid")
    digest = value.get("pricing_local_frozen_schedule_record_sha256")
    if not isinstance(digest, str) or not _HEX_64.fullmatch(digest):
        raise LlmJournalError("paid-attempt pricing record digest is invalid")
    started_at = _utc_instant(value.get("started_at_utc"), "start")
    completed_at = _utc_instant(value.get("completed_at_utc"), "completion")
    if value.get("status") == "pricing_error":
        try:
            conservative = select_deepseek_interval_price(
                value["requested_model_alias"],
                started_at,
                started_at + timedelta(seconds=float(value["timeout_seconds"])),
            )
        except LlmPricingError as error:
            raise LlmJournalError("paid-attempt fallback pricing is unverifiable") from error
        if not _selection_matches(value, conservative, prefix="pricing_"):
            raise LlmJournalError("paid-attempt fallback pricing is inconsistent")
        if completed_at < started_at:
            raise LlmJournalError("paid-attempt completion precedes its start")
        return
    try:
        pricing = select_deepseek_interval_price(
            value["requested_model_alias"], started_at, completed_at
        )
    except LlmPricingError as error:
        raise LlmJournalError("paid-attempt observed pricing is unverifiable") from error
    if not _selection_matches(value, pricing, prefix="pricing_"):
        raise LlmJournalError("paid-attempt observed pricing is inconsistent")


def _selection_matches(value: Mapping[str, Any], selection: Any, *, prefix: str) -> bool:
    expected = {
        f"{prefix}pricing_schedule_id"
        if prefix == "reservation_"
        else "pricing_schedule_id": selection.pricing_schedule_id,
        f"{prefix}billing_band" if prefix == "reservation_" else "billing_band": (
            selection.billing_band
        ),
        f"{prefix}cache_hit_per_million_usd": format(selection.prices.cache_hit_per_million, "f"),
        f"{prefix}cache_miss_per_million_usd": format(selection.prices.cache_miss_per_million, "f"),
        f"{prefix}output_per_million_usd": format(selection.prices.output_per_million, "f"),
    }
    if prefix == "pricing_":
        expected.update(
            {
                "pricing_effective_at": selection.effective_at,
                "pricing_review_not_after": selection.review_not_after,
                "pricing_source_url": selection.source_url,
                "pricing_source_retrieved_at": selection.source_retrieved_at,
                "pricing_source_reviewed_at": selection.source_reviewed_at,
                "pricing_local_frozen_schedule_record_sha256": (
                    selection.local_frozen_schedule_record_sha256
                ),
            }
        )
    return all(value.get(field) == expected_value for field, expected_value in expected.items())


def _validate_model_attestation(value: Mapping[str, Any]) -> None:
    requested = value["requested_model_alias"]
    pinned = {
        "deepseek-v4-flash": "DeepSeek-V4-Flash-0731",
        "deepseek-v4-pro": "DeepSeek-V4-Pro-0813",
    }[requested]
    response = value.get("response_model")
    if response == pinned:
        expected = (
            pinned,
            "attested_exact_version",
            "requires_five_run_variance_ci",
        )
    elif response == requested:
        expected = (
            None,
            "unattested_alias",
            "requires_same_system_fingerprint_and_five_run_variance_ci",
        )
    elif response is None:
        expected = (None, "unattested_no_response", "not_comparable_no_response")
    else:
        expected = (None, "unattested_mismatch", "not_comparable_model_mismatch")
    actual = (
        value.get("official_model_version"),
        value.get("model_revision_attestation"),
        value.get("cross_run_comparability"),
    )
    if actual != expected:
        raise LlmJournalError("paid-attempt model attestation is inconsistent")
    if response not in {None, requested, pinned} and value.get("status") != "protocol_error":
        raise LlmJournalError("paid-attempt model mismatch is not a protocol error")


def _validate_status_matrix(value: Mapping[str, Any], usage: dict[str, Any] | None) -> None:
    status = value["status"]
    http_status = value.get("http_status")
    response_model = value.get("response_model")
    fingerprint = value.get("system_fingerprint")
    finish_reason = value.get("finish_reason")
    basis = value["usd_basis"]
    is_2xx = isinstance(http_status, int) and 200 <= http_status < 300
    if status in {"ok", "semantic_rejected"}:
        valid = (
            is_2xx
            and usage is not None
            and response_model is not None
            and fingerprint is not None
            and finish_reason == "stop"
            and basis == "actual_usage"
        )
    elif status == "incomplete":
        valid = (
            is_2xx
            and usage is not None
            and response_model is not None
            and fingerprint is not None
            and finish_reason is not None
            and finish_reason != "stop"
            and basis == "actual_usage"
        )
    elif status == "accounting_error":
        valid = (
            is_2xx
            and usage is not None
            and response_model is not None
            and basis == ("reserved_upper_bound")
        )
    elif status == "http_error":
        valid = (
            isinstance(http_status, int)
            and not is_2xx
            and response_model is None
            and fingerprint is None
            and finish_reason is None
        )
    elif status in {"indeterminate_transport", "pricing_error"}:
        valid = (
            http_status is None
            and usage is None
            and response_model is None
            and fingerprint is None
            and finish_reason is None
            and basis == "reserved_upper_bound"
        )
    else:
        valid = http_status is None or is_2xx
    if not valid:
        raise LlmJournalError("paid-attempt status evidence is inconsistent")


def _utc_instant(raw: Any, field: str) -> datetime:
    if not isinstance(raw, str) or not raw.endswith("Z"):
        raise LlmJournalError(f"paid-attempt {field} time is invalid")
    try:
        parsed = datetime.fromisoformat(raw[:-1] + "+00:00")
    except ValueError as error:
        raise LlmJournalError(f"paid-attempt {field} time is invalid") from error
    canonical = parsed.astimezone(UTC).isoformat(timespec="microseconds").replace("+00:00", "Z")
    if raw != canonical:
        raise LlmJournalError(f"paid-attempt {field} time is not canonical")
    return parsed.astimezone(UTC)


def _validate_usage(raw: Any) -> dict[str, Any] | None:
    if raw is None:
        return None
    if not isinstance(raw, dict) or set(raw) != _USAGE_FIELDS:
        raise LlmJournalError("paid-attempt usage has missing or unknown fields")
    for field in _USAGE_FIELDS - {"cache_accounting"}:
        if not isinstance(raw[field], int) or isinstance(raw[field], bool) or raw[field] < 0:
            raise LlmJournalError("paid-attempt usage contains an invalid token count")
    if raw["prompt_cache_hit_tokens"] + raw["prompt_cache_miss_tokens"] != raw["prompt_tokens"]:
        raise LlmJournalError("paid-attempt cache usage is inconsistent")
    if raw["prompt_tokens"] + raw["completion_tokens"] != raw["total_tokens"]:
        raise LlmJournalError("paid-attempt total usage is inconsistent")
    if raw["cache_accounting"] not in {"provider_reported", "all_miss_conservative"}:
        raise LlmJournalError("paid-attempt cache accounting is invalid")
    return raw


def _reserved_cost(value: Mapping[str, Any]) -> Decimal:
    return (
        Decimal(value["reserved_input_tokens_upper_bound"])
        * _decimal(value.get("reservation_cache_miss_per_million_usd"), "reserved miss price")
        + Decimal(value["reserved_output_tokens"])
        * _decimal(value.get("reservation_output_per_million_usd"), "reserved output price")
    ) / Decimal(1_000_000)


def _validate_durable_budget(
    entries: tuple[AttemptJournalEntry, ...],
    role_configs: tuple[dict[str, Any], ...],
    candidate: Mapping[str, Any],
) -> None:
    role = candidate["role"]
    _validate_prepared_against_role_config(candidate, role_configs)
    spent = _role_accounting(entries, role)
    requests = spent.requests + 1
    input_tokens = spent.input_tokens_accounted + int(
        candidate["reserved_input_tokens_upper_bound"]
    )
    output_tokens = spent.output_tokens_accounted + int(candidate["reserved_output_tokens"])
    usd = Decimal(spent.usd_accounted) + _decimal(candidate["usd_accounted"], "usd_accounted")
    if requests > candidate["budget_max_requests"]:
        raise LlmJournalError("durable role request budget is exhausted")
    if input_tokens > candidate["budget_max_input_tokens"]:
        raise LlmJournalError("durable role input-token budget is exhausted")
    if output_tokens > candidate["budget_max_output_tokens"]:
        raise LlmJournalError("durable role output-token budget is exhausted")
    if usd > _decimal(candidate["budget_max_usd"], "USD budget"):
        raise LlmJournalError("durable role USD budget is exhausted")


def _role_accounting(entries: tuple[AttemptJournalEntry, ...], role: str) -> JournalRoleAccounting:
    requests = 0
    input_tokens = 0
    output_tokens = 0
    usd = Decimal(0)
    for entry in entries:
        if entry.prepared["role"] != role:
            continue
        requests += 1
        source = entry.prepared if entry.finalized is None else entry.finalized
        assert source is not None
        usage = source.get("usage")
        if source.get("usd_basis") == "actual_usage" and isinstance(usage, dict):
            input_tokens += int(usage["prompt_tokens"])
            output_tokens += int(usage["completion_tokens"])
        else:
            input_tokens += int(source["reserved_input_tokens_upper_bound"])
            output_tokens += int(source["reserved_output_tokens"])
        usd += _decimal(source["usd_accounted"], "usd_accounted")
    return JournalRoleAccounting(
        requests=requests,
        input_tokens_accounted=input_tokens,
        output_tokens_accounted=output_tokens,
        usd_accounted=format(usd, "f"),
    )


def _decimal(raw: Any, field: str) -> Decimal:
    if not isinstance(raw, str):
        raise LlmJournalError(f"paid-attempt {field} is invalid")
    try:
        value = Decimal(raw)
    except InvalidOperation as error:
        raise LlmJournalError(f"paid-attempt {field} is invalid") from error
    if not value.is_finite() or value < 0 or format(value, "f") != raw:
        raise LlmJournalError(f"paid-attempt {field} is invalid")
    return value


def _positive_int(raw: Any, field: str) -> int:
    if not isinstance(raw, int) or isinstance(raw, bool) or raw <= 0:
        raise LlmJournalError(f"paid-attempt {field} is invalid")
    return raw


def _bounded_number(
    raw: Any,
    minimum: float,
    maximum: float,
    *,
    exclusive_min: bool = False,
) -> bool:
    if not isinstance(raw, (int, float)) or isinstance(raw, bool):
        return False
    if exclusive_min:
        return minimum < raw <= maximum
    return minimum <= raw <= maximum


def _bounded_canonical(value: Mapping[str, Any]) -> bytes:
    payload = canonical_json(value)
    if not payload or len(payload) > _MAX_ENTRY_BYTES:
        raise LlmJournalError("paid-attempt evidence is outside its byte limit")
    return payload


def _persist_no_clobber(directory_fd: int, final: str, payload: bytes) -> None:
    temporary = f".attempt-pending.{uuid.uuid4().hex}.tmp"
    _write_exclusive_at(directory_fd, temporary, payload)
    try:
        os.link(
            temporary,
            final,
            src_dir_fd=directory_fd,
            dst_dir_fd=directory_fd,
            follow_symlinks=False,
        )
        os.fsync(directory_fd)
        os.unlink(temporary, dir_fd=directory_fd)
        os.fsync(directory_fd)
    except Exception:
        # The pending inode and any linked final name remain discoverable after failure.
        raise


def _write_exclusive_at(directory_fd: int, name: str, payload: bytes) -> None:
    descriptor = os.open(
        name,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0),
        0o600,
        dir_fd=directory_fd,
    )
    try:
        _write_all(descriptor, payload)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _read_private_at(directory_fd: int, name: str) -> bytes:
    flags = (
        os.O_RDONLY
        | os.O_NONBLOCK
        | getattr(os, "O_CLOEXEC", 0)
        | getattr(os, "O_NOFOLLOW", 0)
        | getattr(os, "O_NOATIME", 0)
    )
    descriptor = os.open(name, flags, dir_fd=directory_fd)
    try:
        before = os.fstat(descriptor)
        if (
            not stat.S_ISREG(before.st_mode)
            or before.st_uid != os.geteuid()
            or stat.S_IMODE(before.st_mode) != 0o600
            or before.st_size <= 0
            or before.st_size > _MAX_ENTRY_BYTES
        ):
            raise LlmJournalError("paid-attempt journal entry is not a private bounded file")
        payload = _read_all(descriptor, before.st_size)
        after = os.fstat(descriptor)
        if len(payload) != before.st_size or _stable_stat(after) != _stable_stat(before):
            raise LlmJournalError("paid-attempt journal entry changed while being read")
        return payload
    finally:
        os.close(descriptor)


def _write_all(descriptor: int, payload: bytes) -> None:
    view = memoryview(payload)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise OSError("paid-attempt journal write made no progress")
        view = view[written:]


def _read_all(descriptor: int, size: int) -> bytes:
    chunks: list[bytes] = []
    remaining = size
    while remaining:
        chunk = os.read(descriptor, min(remaining, 64 * 1024))
        if not chunk:
            break
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def _stable_stat(metadata: os.stat_result) -> tuple[int, ...]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        stat.S_IFMT(metadata.st_mode),
        stat.S_IMODE(metadata.st_mode),
        metadata.st_uid,
        metadata.st_size,
        metadata.st_mtime_ns,
        metadata.st_ctime_ns,
    )
