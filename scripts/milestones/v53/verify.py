#!/usr/bin/env python3
"""Verify an architecture-owned v53 WP2 handoff packet.

The verifier deliberately uses only the Python standard library.  Packet JSON is
canonical, closed-schema JSON; repository bindings are checked against Git object
bytes, never against mutable worktree bytes.
"""

from __future__ import annotations

import argparse
import base64
import binascii
import csv
import datetime as dt
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
from decimal import ROUND_DOWN, Decimal
from pathlib import Path
from typing import Any

SPEC_SCHEMA = "plico.v53.wp2-r2-spec/v1"
HANDOFF_SCHEMA = "plico.v53.wp2-r2-handoff/v1"
DIGEST_SCHEMA = "plico.v53.wp2-r2-handoff-digest/v1"
COMMIT_SCHEMA = "plico.v53.wp2-r2-handoff-commit/v1"
LOCK_SCHEMA = "plico.v53.wp2-r2-handoff-lock/v1"
PACKET_FILES = ("LOCK", "handoff.json", "handoff.sha256.json", "COMMITTED")
MAX_SAFE_INTEGER = 9_007_199_254_740_991
MAX_PACKET_FILE_BYTES = 4 * 1024 * 1024
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
GIT_OBJECT_ID = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
PACKET_ID = re.compile(r"^wp2-r2-[0-9a-f]{32}$")
SPEC_PATH = "scripts/milestones/v53/wp2_spec.json"
REQUIRED_PREDECESSOR_COMMITS = [
    "5584b8e7b48247e503d9054bb3b3227c64c7ad94",
    "2c42b42dac601c9bb6f91ee7db019bf77012a017",
    "9a44c91fec3c870e6a9d8272379da9b748d183bc",
    "98de9bd2fa4eb6c6f2dbbb7171ba762124144104",
    "189f5cffa969903c0e4ec3259848b1405e924587",
    "8eb70d7f72a5fbbfd85c308234b561af2e22f676",
]
CONTROL = re.compile(r"[\x00-\x1f\x7f]")
CANONICAL_UUID = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
NIL_UUID = "00000000-0000-0000-0000-000000000000"
EXPECTED_RECORD_SCHEMAS = {
    "current_view": "plico.execution-observation.fixture-current-view/v1",
    "fixture_start_request": "plico.execution-observation.fixture-start-request/v1",
    "fixture_started": "plico.execution-observation.fixture-started/v1",
    "fixture_terminal_request": "plico.execution-observation.fixture-terminal-request/v1",
    "fixture_terminal": "plico.execution-observation.fixture-terminal/v1",
    "pointer": "plico.execution-observation.fixture-root-pointer/v1",
    "root": "plico.execution-observation.fixture-root/v1",
    "segment": "plico.execution-observation.fixture-segment/v1",
}
EXPECTED_HASH_DOMAINS = {
    "current_view": "plico.execution-observation.fixture.current-view.v1\0",
    "root": "plico.execution-observation.fixture.root.v1\0",
    "segment": "plico.execution-observation.fixture.segment.v1\0",
    "started_event": "plico.execution-observation.fixture.started-event.v1\0",
    "started_request": "plico.execution-observation.fixture.started-request.v1\0",
    "terminal_event": "plico.execution-observation.fixture.terminal-event.v1\0",
    "terminal_request": "plico.execution-observation.fixture.terminal-request.v1\0",
}
EXPECTED_GOLDEN_DOMAINS = {
    "genesis_current_view": "current_view",
    "genesis_root": "root",
    "started_current_view": "current_view",
    "started_event": "started_event",
    "started_request": "started_request",
    "started_root": "root",
    "started_segment": "segment",
    "terminal_current_view": "current_view",
    "terminal_event": "terminal_event",
    "terminal_request": "terminal_request",
    "terminal_root": "root",
    "terminal_segment": "segment",
}
EXPECTED_GOLDEN_SHA256 = {
    "genesis_current_view": "f0b5d12cde6534fdccf88a1e8ff915feaa0f6c4302c3f99453fd713de1c3e92d",
    "genesis_root": "1f1106793cdd964ef5c6b41644638ddc0c12b296b80c57fca13c98fc657a398f",
    "started_current_view": "4204a72e2366a15efeb9e8135979fcb883cc7323773d61385b43c30445e5aba0",
    "started_event": "96438232ef0aab25ad5f54b3082bc0ed0fb0dcabdfa78a1c3567d51b2026cfc0",
    "started_request": "160804b6003538aba7cf858993b2f3efdf830493875a9c03e5277db0225975ac",
    "started_root": "6c3e5154ae5e26f8a3e230d54391f3639ad7adce8c6848fd9d077a121d8a4936",
    "started_segment": "aeab7ab3e137f5b9a2a20fd945e970976c68df6639c4031869569c545d03674d",
    "terminal_current_view": "d2bbabf5a9b3ce6121b48bc3c599b83be7e8c7d4f5330374a8837e2c51799722",
    "terminal_event": "c178e5f3fc6c3570b655eccff18e337ccc579e09a3ad07b6586b1f4a5a27a858",
    "terminal_request": "f8dd59a4bdaeabe52b27b79f0f4c749e344f7483ec66588ef6f9efe55f9d5bf2",
    "terminal_root": "1a0a1c708d872579d387651509cf3383617f764faa44fc475f3a5798c1a85e8a",
    "terminal_segment": "d0a1ed026079be1dc59258d9d10f1fbc9e3f6ef1dd390682814751d8a9bd584f",
}


class VerificationError(RuntimeError):
    """A closed, user-actionable verification failure."""


def _reject_constant(value: str) -> None:
    raise VerificationError(f"non-finite JSON number is forbidden: {value}")


def _pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise VerificationError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def _walk_json(value: Any, location: str = "$") -> None:
    if isinstance(value, float):
        raise VerificationError(f"floating-point JSON value is forbidden at {location}")
    if isinstance(value, int) and not isinstance(value, bool):
        if not -MAX_SAFE_INTEGER <= value <= MAX_SAFE_INTEGER:
            raise VerificationError(
                f"JSON integer exceeds the exact range at {location}"
            )
    if isinstance(value, list):
        for index, item in enumerate(value):
            _walk_json(item, f"{location}[{index}]")
    if isinstance(value, dict):
        for key, item in value.items():
            if not isinstance(key, str) or CONTROL.search(key):
                raise VerificationError(f"invalid object key at {location}")
            _walk_json(item, f"{location}.{key}")


def strict_json_loads(data: bytes, label: str) -> Any:
    try:
        text = data.decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise VerificationError(f"{label}: JSON is not UTF-8") from error
    try:
        value = json.loads(
            text,
            object_pairs_hook=_pairs,
            parse_constant=_reject_constant,
        )
    except (json.JSONDecodeError, VerificationError) as error:
        if isinstance(error, VerificationError):
            raise
        raise VerificationError(f"{label}: invalid JSON: {error.msg}") from error
    _walk_json(value)
    return value


def canonical_json(value: Any) -> bytes:
    try:
        return (
            json.dumps(
                value,
                ensure_ascii=False,
                allow_nan=False,
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n"
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise VerificationError(
            f"value cannot be encoded as canonical JSON: {error}"
        ) from error


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def require_object(value: Any, location: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise VerificationError(f"{location} must be an object")
    return value


def require_exact_keys(value: dict[str, Any], keys: set[str], location: str) -> None:
    actual = set(value)
    if actual != keys:
        missing = sorted(keys - actual)
        unknown = sorted(actual - keys)
        raise VerificationError(
            f"{location} key mismatch; missing={missing}, unknown={unknown}"
        )


def require_string(value: Any, location: str, max_bytes: int = 4096) -> str:
    if not isinstance(value, str) or not value:
        raise VerificationError(f"{location} must be a non-empty string")
    if len(value.encode("utf-8")) > max_bytes or CONTROL.search(value):
        raise VerificationError(
            f"{location} is too long or contains a control character"
        )
    return value


def require_string_list(
    value: Any, location: str, *, sorted_unique: bool = False
) -> list[str]:
    if not isinstance(value, list):
        raise VerificationError(f"{location} must be a list")
    result = [
        require_string(item, f"{location}[{index}]") for index, item in enumerate(value)
    ]
    if len(set(result)) != len(result):
        raise VerificationError(f"{location} contains duplicates")
    if sorted_unique and result != sorted(result):
        raise VerificationError(f"{location} must be sorted")
    return result


def _require_sha(value: Any, location: str, pattern: re.Pattern[str]) -> str:
    result = require_string(value, location)
    if not pattern.fullmatch(result):
        raise VerificationError(f"{location} is not a canonical digest/object id")
    return result


def parse_utc(value: Any, location: str) -> dt.datetime:
    text = require_string(value, location, 32)
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", text):
        raise VerificationError(f"{location} must be second-precision UTC RFC3339")
    try:
        parsed = dt.datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError as error:
        raise VerificationError(f"{location} is not a valid UTC timestamp") from error
    return parsed.replace(tzinfo=dt.timezone.utc)


def _canonical_jcs_object(value: Any, location: str) -> dict[str, Any]:
    text = require_string(value, location, 256 * 1024)
    raw = text.encode("utf-8")
    parsed = require_object(strict_json_loads(raw, location), location)
    recomputed = json.dumps(
        parsed,
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    if recomputed != raw:
        raise VerificationError(f"{location} is not canonical JCS")
    return parsed


def _require_canonical_non_nil_uuid(value: Any, location: str) -> None:
    if (
        not isinstance(value, str)
        or not CANONICAL_UUID.fullmatch(value)
        or value == NIL_UUID
    ):
        raise VerificationError(f"{location} is not a canonical non-nil UUID")


def _validate_golden_chain(
    parsed: dict[str, dict[str, Any]], pointers: dict[str, Any]
) -> None:
    hashes = EXPECTED_GOLDEN_SHA256
    schemas = EXPECTED_RECORD_SCHEMAS
    key = {"attempt": 1, "execution_id": "123e4567-e89b-42d3-a456-426614174000"}
    _require_canonical_non_nil_uuid(key["execution_id"], "golden execution_id")

    started_request = parsed["started_request"]
    if (
        started_request.get("key") != key
        or started_request.get("fixture_role_ref") is not None
        or started_request.get("fixture_session_ref") is not None
        or started_request.get("fixture_origin")
        != {
            "request_id": "123e4567-e89b-42d3-a456-426614174001",
            "type": "public_request",
        }
    ):
        raise VerificationError("Started request golden wire contract differs")
    _require_canonical_non_nil_uuid(
        started_request["fixture_origin"]["request_id"],
        "golden FixtureOriginV1.request_id",
    )
    terminal_request = parsed["terminal_request"]
    if (
        terminal_request.get("key") != key
        or terminal_request.get("execution_elapsed_ms", "missing") is not None
        or terminal_request.get("outcome")
        != {"category": "tool_failed", "type": "failure"}
    ):
        raise VerificationError("Terminal request golden wire contract differs")

    expected_genesis_view = {
        "attempts": [],
        "attestation_state": "unverified_fixture",
        "event_watermark": 0,
        "generation": 0,
        "schema": schemas["current_view"],
    }
    if parsed["genesis_current_view"] != expected_genesis_view:
        raise VerificationError("genesis current-view golden differs")
    expected_genesis_root = {
        "committed_at_ms": 0,
        "current_view_sha256": hashes["genesis_current_view"],
        "event_segment_head_sha256": None,
        "event_watermark": 0,
        "generation": 0,
        "previous_root_sha256": None,
        "schema": schemas["root"],
        "trust_class": "unverified_fixture_only",
    }
    if parsed["genesis_root"] != expected_genesis_root:
        raise VerificationError("genesis root golden differs")

    expected_started_event = {
        "recorded_at_ms": 1_700_000_000_000,
        "request": started_request,
        "request_sha256": hashes["started_request"],
        "root_generation": 1,
        "schema": schemas["fixture_started"],
        "sequence": 1,
    }
    if parsed["started_event"] != expected_started_event:
        raise VerificationError("Started event golden binding differs")
    expected_started_segment = {
        "event_kind": "started",
        "event_sha256": hashes["started_event"],
        "first_sequence": 1,
        "last_sequence": 1,
        "previous_segment_sha256": None,
        "schema": schemas["segment"],
    }
    if parsed["started_segment"] != expected_started_segment:
        raise VerificationError("Started segment golden binding differs")
    started_attempt = {
        "attestation_state": "unverified_fixture",
        "key": key,
        "started_event_sha256": hashes["started_event"],
        "started_request_sha256": hashes["started_request"],
        "terminal_event_sha256": None,
        "terminal_request_sha256": None,
    }
    expected_started_view = {
        "attempts": [started_attempt],
        "attestation_state": "unverified_fixture",
        "event_watermark": 1,
        "generation": 1,
        "schema": schemas["current_view"],
    }
    if parsed["started_current_view"] != expected_started_view:
        raise VerificationError("Started current-view golden binding differs")
    expected_started_root = {
        "committed_at_ms": 1_700_000_000_000,
        "current_view_sha256": hashes["started_current_view"],
        "event_segment_head_sha256": hashes["started_segment"],
        "event_watermark": 1,
        "generation": 1,
        "previous_root_sha256": hashes["genesis_root"],
        "schema": schemas["root"],
        "trust_class": "unverified_fixture_only",
    }
    if parsed["started_root"] != expected_started_root:
        raise VerificationError("Started root golden binding differs")

    expected_terminal_event = {
        "recorded_at_ms": 1_700_000_000_042,
        "request": terminal_request,
        "request_sha256": hashes["terminal_request"],
        "root_generation": 2,
        "schema": schemas["fixture_terminal"],
        "sequence": 2,
    }
    if parsed["terminal_event"] != expected_terminal_event:
        raise VerificationError("Terminal event golden binding differs")
    expected_terminal_segment = {
        "event_kind": "terminal",
        "event_sha256": hashes["terminal_event"],
        "first_sequence": 2,
        "last_sequence": 2,
        "previous_segment_sha256": hashes["started_segment"],
        "schema": schemas["segment"],
    }
    if parsed["terminal_segment"] != expected_terminal_segment:
        raise VerificationError("Terminal segment golden binding differs")
    terminal_attempt = {
        **started_attempt,
        "terminal_event_sha256": hashes["terminal_event"],
        "terminal_request_sha256": hashes["terminal_request"],
    }
    expected_terminal_view = {
        "attempts": [terminal_attempt],
        "attestation_state": "unverified_fixture",
        "event_watermark": 2,
        "generation": 2,
        "schema": schemas["current_view"],
    }
    if parsed["terminal_current_view"] != expected_terminal_view:
        raise VerificationError("Terminal current-view golden binding differs")
    expected_terminal_root = {
        "committed_at_ms": 1_700_000_000_042,
        "current_view_sha256": hashes["terminal_current_view"],
        "event_segment_head_sha256": hashes["terminal_segment"],
        "event_watermark": 2,
        "generation": 2,
        "previous_root_sha256": hashes["started_root"],
        "schema": schemas["root"],
        "trust_class": "unverified_fixture_only",
    }
    if parsed["terminal_root"] != expected_terminal_root:
        raise VerificationError("Terminal root golden binding differs")

    require_exact_keys(
        pointers, {"genesis", "started", "terminal"}, "spec.golden_pointers"
    )
    for name, root_name in {
        "genesis": "genesis_root",
        "started": "started_root",
        "terminal": "terminal_root",
    }.items():
        pointer = _canonical_jcs_object(pointers[name], f"spec.golden_pointers.{name}")
        if pointer != {
            "root_sha256": hashes[root_name],
            "schema": schemas["pointer"],
        }:
            raise VerificationError(f"golden pointer binding differs: {name}")


SPEC_TOP_KEYS = {
    "accepted_adr",
    "canonicalization",
    "closed_enums",
    "collection_semantics",
    "contract",
    "contract_version",
    "coverage_contract",
    "developer_scope",
    "developer_self_preflight",
    "error_taxonomy",
    "field_provenance",
    "fixed_lifecycle_recipe",
    "golden_pointers",
    "golden_vectors",
    "hash_domains",
    "internal_api",
    "limits",
    "local_gate_contract",
    "lockfiles",
    "namespace",
    "product_baseline_sha",
    "predecessor_commits",
    "record_schemas",
    "required_bindings",
    "schema",
    "state_machine",
    "storage_topology",
    "test_contract",
    "toolchain",
    "unsupported",
    "wire_contract",
}


def validate_spec(value: Any) -> dict[str, Any]:
    spec = require_object(value, "spec")
    require_exact_keys(spec, SPEC_TOP_KEYS, "spec")
    if spec["schema"] != SPEC_SCHEMA:
        raise VerificationError("unsupported WP2 spec schema")
    if spec["contract_version"] != "plico.milestone.v53.wp2-r2/1":
        raise VerificationError("unexpected contract version")
    predecessors = require_string_list(
        spec["predecessor_commits"], "spec.predecessor_commits"
    )
    if predecessors != REQUIRED_PREDECESSOR_COMMITS:
        raise VerificationError("WP2 predecessor commit chain differs")
    _require_sha(
        spec["product_baseline_sha"], "spec.product_baseline_sha", GIT_OBJECT_ID
    )
    if spec["namespace"] != "execution-observation-fixture-ledger":
        raise VerificationError("unexpected observation namespace")

    adr = require_object(spec["accepted_adr"], "spec.accepted_adr")
    require_exact_keys(
        adr,
        {"path", "required_heading", "required_phrases", "required_status"},
        "spec.accepted_adr",
    )
    if adr["path"] != "docs/adr/0008-execution-observation-store-substrate-v1.md":
        raise VerificationError("ADR path is not frozen ADR-0008")
    require_string(adr["required_heading"], "spec.accepted_adr.required_heading")
    require_string(adr["required_status"], "spec.accepted_adr.required_status")
    require_string_list(adr["required_phrases"], "spec.accepted_adr.required_phrases")

    contract = require_object(spec["contract"], "spec.contract")
    require_exact_keys(contract, {"path", "required_state", "version"}, "spec.contract")
    if contract["path"] != "docs/milestones/v53-wp2-r2-checkpoint.md":
        raise VerificationError("contract path is not frozen")
    if contract["version"] != spec["contract_version"]:
        raise VerificationError("nested contract version differs from spec version")
    require_string(contract["required_state"], "spec.contract.required_state")

    canonicalization = require_object(spec["canonicalization"], "spec.canonicalization")
    require_exact_keys(
        canonicalization,
        {"algorithm", "crate", "crate_version", "digest", "formula"},
        "spec.canonicalization",
    )
    if canonicalization != {
        "algorithm": "RFC8785/JCS",
        "crate": "serde_json_canonicalizer",
        "crate_version": "0.3.2",
        "digest": "SHA-256",
        "formula": "sha256(domain || RFC8785_JCS(value))",
    }:
        raise VerificationError("canonicalization contract differs from ADR-0007")

    closed_enums = require_object(spec["closed_enums"], "spec.closed_enums")
    require_exact_keys(
        closed_enums,
        {"failure_categories", "fixture_origins", "terminal_outcomes"},
        "spec.closed_enums",
    )
    for key, item in closed_enums.items():
        require_string_list(item, f"spec.closed_enums.{key}")
    if closed_enums["fixture_origins"] != [
        "public_request",
        "intent_dispatch",
        "internal_task",
    ]:
        raise VerificationError("fixture origin enum differs from ADR-0007")
    if closed_enums["terminal_outcomes"] != [
        "success",
        "failure",
        "timeout",
        "cancelled",
        "indeterminate",
    ]:
        raise VerificationError("terminal outcome enum differs from ADR-0007")
    if closed_enums["failure_categories"] != [
        "invalid_input",
        "policy_denied",
        "dependency_unavailable",
        "executor_rejected",
        "executor_failed",
        "executor_panicked",
        "tool_failed",
        "internal",
    ]:
        raise VerificationError("failure category enum differs from ADR-0007")

    schemas = require_object(spec["record_schemas"], "spec.record_schemas")
    require_exact_keys(
        schemas,
        {
            "current_view",
            "fixture_start_request",
            "fixture_started",
            "fixture_terminal_request",
            "fixture_terminal",
            "pointer",
            "root",
            "segment",
        },
        "spec.record_schemas",
    )
    if schemas != EXPECTED_RECORD_SCHEMAS:
        raise VerificationError("record schemas differ from ADR-0007")

    wire = require_object(spec["wire_contract"], "spec.wire_contract")
    require_exact_keys(
        wire,
        {
            "attempt_observation_fields",
            "declared_field_presence",
            "enum_encoding",
            "nullable_encoding",
            "nullable_fields",
            "uuid_encoding",
            "uuid_fields",
        },
        "spec.wire_contract",
    )
    expected_wire = {
        "attempt_observation_fields": [
            "attestation_state",
            "key",
            "started_receipt",
            "terminal_receipt",
        ],
        "declared_field_presence": "all-declared-fields-required",
        "enum_encoding": "internally-tagged-object/type/snake_case",
        "nullable_encoding": "present-json-null",
        "nullable_fields": [
            "AppendStartedRequestV1.fixture_role_ref",
            "AppendStartedRequestV1.fixture_session_ref",
            "AppendTerminalRequestV1.execution_elapsed_ms",
            "FixtureAttemptObservationV1.terminal_receipt",
            "FixtureAttemptViewV1.terminal_event_sha256",
            "FixtureAttemptViewV1.terminal_request_sha256",
            "FixtureEventSegmentV1.previous_segment_sha256",
            "FixtureLedgerRootV1.event_segment_head_sha256",
            "FixtureLedgerRootV1.previous_root_sha256",
        ],
        "uuid_encoding": "lowercase-hyphenated-rfc4122-36-ascii/non-nil",
        "uuid_fields": [
            "ExecutionAttemptKeyV1.execution_id",
            "FixtureOriginV1.request_id|intent_id|task_id",
            "AppendStartedRequestV1.fixture_role_ref",
            "AppendStartedRequestV1.fixture_session_ref",
        ],
    }
    if wire != expected_wire:
        raise VerificationError("wire contract differs from ADR-0007")

    domains = require_object(spec["hash_domains"], "spec.hash_domains")
    require_exact_keys(
        domains,
        {
            "current_view",
            "root",
            "segment",
            "started_event",
            "started_request",
            "terminal_event",
            "terminal_request",
        },
        "spec.hash_domains",
    )
    if domains != EXPECTED_HASH_DOMAINS:
        raise VerificationError("hash domains differ from ADR-0007")

    vectors = require_object(spec["golden_vectors"], "spec.golden_vectors")
    require_exact_keys(vectors, set(EXPECTED_GOLDEN_DOMAINS), "spec.golden_vectors")
    parsed_vectors: dict[str, dict[str, Any]] = {}
    for name, vector_value in vectors.items():
        vector = require_object(vector_value, f"spec.golden_vectors.{name}")
        require_exact_keys(
            vector,
            {"canonical_jcs_utf8", "domain_key", "sha256"},
            f"spec.golden_vectors.{name}",
        )
        parsed_vector = _canonical_jcs_object(
            vector["canonical_jcs_utf8"],
            f"spec.golden_vectors.{name}.canonical_jcs_utf8",
        )
        parsed_vectors[name] = parsed_vector
        canonical_bytes = vector["canonical_jcs_utf8"].encode("utf-8")
        domain_key = require_string(
            vector["domain_key"], f"spec.golden_vectors.{name}.domain_key"
        )
        if domain_key != EXPECTED_GOLDEN_DOMAINS[name]:
            raise VerificationError(f"golden vector domain differs: {name}")
        expected_digest = hashlib.sha256(
            domains[domain_key].encode("utf-8") + canonical_bytes
        ).hexdigest()
        if vector["sha256"] != expected_digest:
            raise VerificationError(f"golden vector digest differs: {name}")
        if vector["sha256"] != EXPECTED_GOLDEN_SHA256[name]:
            raise VerificationError(f"golden vector identity differs: {name}")
    pointers = require_object(spec["golden_pointers"], "spec.golden_pointers")
    _validate_golden_chain(parsed_vectors, pointers)

    limits = require_object(spec["limits"], "spec.limits")
    require_exact_keys(
        limits,
        {
            "attempt_max",
            "attempt_min",
            "attempts_max",
            "canonical_request_max_bytes",
            "cid_ascii_bytes",
            "current_view_max_bytes",
            "evidence_items_per_list_max",
            "evidence_items_total_max",
            "events_max",
            "execution_elapsed_ms_max",
            "pointer_max_bytes",
            "recorded_at_ms_max",
            "root_max_bytes",
            "segment_max_bytes",
            "sequence_max",
            "sha256_ascii_bytes",
            "stored_event_max_bytes",
        },
        "spec.limits",
    )
    for key, item in limits.items():
        if not isinstance(item, int) or isinstance(item, bool) or item <= 0:
            raise VerificationError(f"invalid positive integer limit: {key}")
    if limits["attempt_min"] != 1 or limits["attempt_max"] != 4_294_967_295:
        raise VerificationError("attempt range must be NonZeroU32")
    expected_limits = {
        "attempts_max": 10_000,
        "canonical_request_max_bytes": 128 * 1024,
        "cid_ascii_bytes": 64,
        "current_view_max_bytes": 8 * 1024 * 1024,
        "evidence_items_per_list_max": 256,
        "evidence_items_total_max": 512,
        "events_max": 20_000,
        "execution_elapsed_ms_max": MAX_SAFE_INTEGER,
        "pointer_max_bytes": 4 * 1024,
        "recorded_at_ms_max": MAX_SAFE_INTEGER,
        "root_max_bytes": 64 * 1024,
        "segment_max_bytes": 64 * 1024,
        "sequence_max": MAX_SAFE_INTEGER,
        "sha256_ascii_bytes": 64,
        "stored_event_max_bytes": 135_168,
    }
    for key, expected in expected_limits.items():
        if limits[key] != expected:
            raise VerificationError(f"limit differs from ADR-0007: {key}")

    semantics = require_object(
        spec["collection_semantics"], "spec.collection_semantics"
    )
    require_exact_keys(
        semantics,
        {
            "context_evidence_cids",
            "input_evidence_cids",
            "output_evidence_cids",
            "set_fields",
        },
        "spec.collection_semantics",
    )
    ordered = "ordered-list/reject-duplicates/preserve-order"
    for key in ("context_evidence_cids", "input_evidence_cids", "output_evidence_cids"):
        if semantics[key] != ordered:
            raise VerificationError(f"collection semantics changed for {key}")
    if semantics["set_fields"] != []:
        raise VerificationError("v1 has no set fields")

    provenance = require_object(spec["field_provenance"], "spec.field_provenance")
    require_exact_keys(
        provenance,
        {
            "caller_fixture",
            "deterministic_hash_or_binding",
            "fixed_constant",
            "replay_derived",
            "request_copy",
            "writer_stamped",
        },
        "spec.field_provenance",
    )
    provenance_sets: dict[str, set[str]] = {}
    for key, item in provenance.items():
        provenance_sets[key] = set(
            require_string_list(item, f"spec.field_provenance.{key}")
        )
    seen: set[str] = set()
    for key, fields in provenance_sets.items():
        overlap = seen & fields
        if overlap:
            raise VerificationError(
                f"provenance field appears in multiple categories: {sorted(overlap)}"
            )
        seen |= fields
    expected_inventory = {
        "ExecutionAttemptKeyV1.execution_id",
        "ExecutionAttemptKeyV1.attempt",
        "FixtureOriginV1.type",
        "FixtureOriginV1.request_id|intent_id|task_id",
        "AppendStartedRequestV1.schema",
        "AppendStartedRequestV1.key",
        "AppendStartedRequestV1.fixture_origin",
        "AppendStartedRequestV1.attestation_state",
        "AppendStartedRequestV1.fixture_role_ref",
        "AppendStartedRequestV1.fixture_session_ref",
        "AppendStartedRequestV1.operation_contract_sha256",
        "AppendStartedRequestV1.input_evidence_cids",
        "AppendStartedRequestV1.context_evidence_cids",
        "AppendStartedRequestV1.policy_sha256",
        "AppendStartedRequestV1.runtime_sha256",
        "AppendTerminalRequestV1.schema",
        "AppendTerminalRequestV1.key",
        "AppendTerminalRequestV1.attestation_state",
        "AppendTerminalRequestV1.outcome",
        "AppendTerminalRequestV1.output_evidence_cids",
        "AppendTerminalRequestV1.execution_elapsed_ms",
        "AppendTerminalRequestV1.policy_sha256",
        "AppendTerminalRequestV1.runtime_sha256",
        "StoredStartedEventV1.schema",
        "StoredStartedEventV1.request",
        "StoredStartedEventV1.request_sha256",
        "StoredStartedEventV1.sequence",
        "StoredStartedEventV1.root_generation",
        "StoredStartedEventV1.recorded_at_ms",
        "StoredTerminalEventV1.schema",
        "StoredTerminalEventV1.request",
        "StoredTerminalEventV1.request_sha256",
        "StoredTerminalEventV1.sequence",
        "StoredTerminalEventV1.root_generation",
        "StoredTerminalEventV1.recorded_at_ms",
        "FixtureEventSegmentV1.schema",
        "FixtureEventSegmentV1.first_sequence",
        "FixtureEventSegmentV1.last_sequence",
        "FixtureEventSegmentV1.previous_segment_sha256",
        "FixtureEventSegmentV1.event_kind",
        "FixtureEventSegmentV1.event_sha256",
        "FixtureAttemptViewV1.key",
        "FixtureAttemptViewV1.attestation_state",
        "FixtureAttemptViewV1.started_request_sha256",
        "FixtureAttemptViewV1.started_event_sha256",
        "FixtureAttemptViewV1.terminal_request_sha256",
        "FixtureAttemptViewV1.terminal_event_sha256",
        "FixtureAttemptObservationV1.key",
        "FixtureAttemptObservationV1.attestation_state",
        "FixtureAttemptObservationV1.started_receipt",
        "FixtureAttemptObservationV1.terminal_receipt",
        "FixtureCurrentViewV1.schema",
        "FixtureCurrentViewV1.attestation_state",
        "FixtureCurrentViewV1.generation",
        "FixtureCurrentViewV1.event_watermark",
        "FixtureCurrentViewV1.attempts",
        "FixtureLedgerRootV1.schema",
        "FixtureLedgerRootV1.trust_class",
        "FixtureLedgerRootV1.generation",
        "FixtureLedgerRootV1.previous_root_sha256",
        "FixtureLedgerRootV1.event_segment_head_sha256",
        "FixtureLedgerRootV1.event_watermark",
        "FixtureLedgerRootV1.current_view_sha256",
        "FixtureLedgerRootV1.committed_at_ms",
        "FixtureActivePointerV1.schema",
        "FixtureActivePointerV1.root_sha256",
        "ObservationReceiptV1.request_sha256",
        "ObservationReceiptV1.event_sha256",
        "ObservationReceiptV1.sequence",
        "ObservationReceiptV1.root_generation",
        "ObservationReceiptV1.root_sha256",
        "ObservationReceiptV1.recorded_at_ms",
    }
    if seen != expected_inventory:
        raise VerificationError(
            f"provenance inventory mismatch; missing={sorted(expected_inventory - seen)}, "
            f"unknown={sorted(seen - expected_inventory)}"
        )
    if provenance_sets["request_copy"] != {
        "StoredStartedEventV1.request",
        "StoredTerminalEventV1.request",
    }:
        raise VerificationError(
            "stored request envelopes must be classified as request_copy"
        )
    required_writer_stamps = {
        "StoredStartedEventV1.sequence",
        "StoredStartedEventV1.root_generation",
        "StoredStartedEventV1.recorded_at_ms",
        "StoredTerminalEventV1.sequence",
        "StoredTerminalEventV1.root_generation",
        "StoredTerminalEventV1.recorded_at_ms",
        "FixtureEventSegmentV1.first_sequence",
        "FixtureLedgerRootV1.generation",
        "FixtureLedgerRootV1.event_watermark",
        "FixtureLedgerRootV1.committed_at_ms",
    }
    if provenance_sets["writer_stamped"] != required_writer_stamps:
        raise VerificationError("writer-stamped provenance differs from ADR-0007")

    api = require_object(spec["internal_api"], "spec.internal_api")
    require_exact_keys(
        api,
        {
            "commit_structural",
            "inject_post_exchange_sync_failure_once",
            "inject_pre_exchange_failure_once",
            "open_fixture",
            "stored_event_type",
            "store_type",
            "structural_commit_type",
            "structural_state",
            "structural_state_type",
        },
        "spec.internal_api",
    )
    expected_api = {
        "commit_structural": (
            "pub(super) fn commit_structural(&self, commit: FixtureStructuralCommitV1) "
            "-> Result<FixtureStructuralStateV1, ObservationStoreError>"
        ),
        "inject_post_exchange_sync_failure_once": (
            "#[cfg(test)] pub(super) fn inject_post_exchange_sync_failure_once(&self)"
        ),
        "inject_pre_exchange_failure_once": (
            "#[cfg(test)] pub(super) fn inject_pre_exchange_failure_once(&self)"
        ),
        "open_fixture": (
            "pub(super) fn open_fixture(vault: Arc<PersonalVaultStorage>) "
            "-> Result<Self, ObservationStoreError>"
        ),
        "stored_event_type": "pub(super) enum FixtureStoredEventV1",
        "store_type": "pub(super) struct FixtureObservationStoreV1",
        "structural_commit_type": "pub(super) struct FixtureStructuralCommitV1",
        "structural_state": (
            "pub(super) fn structural_state(&self) "
            "-> Result<FixtureStructuralStateV1, ObservationStoreError>"
        ),
        "structural_state_type": "pub(super) struct FixtureStructuralStateV1",
    }
    if api != expected_api:
        raise VerificationError("crate-private API differs from ADR-0008")

    errors = require_object(spec["error_taxonomy"], "spec.error_taxonomy")
    require_exact_keys(
        errors,
        {
            "corrupt_store",
            "invalid_request",
            "limit_exceeded",
            "terminal_variants",
            "transition_conflict",
        },
        "spec.error_taxonomy",
    )
    for key, item in errors.items():
        require_string_list(item, f"spec.error_taxonomy.{key}")
    expected_errors = {
        "invalid_request": [
            "unsupported_schema",
            "invalid_attestation",
            "nil_uuid",
            "zero_attempt",
            "invalid_digest",
            "invalid_cid",
            "duplicate_cid",
            "invalid_failure_category",
            "unsafe_integer",
            "size_limit_exceeded",
            "jcs_canonicalization_failed",
        ],
        "transition_conflict": [
            "started_already_bound",
            "terminal_without_started",
            "terminal_already_bound",
            "terminal_policy_rebind",
            "terminal_runtime_rebind",
        ],
        "limit_exceeded": [
            "attempt_limit",
            "event_limit",
            "evidence_list_limit",
            "evidence_total_limit",
            "request_bytes_limit",
            "object_bytes_limit",
        ],
        "corrupt_store": [
            "missing_active_pointer",
            "noncanonical_pointer",
            "unsupported_stored_schema",
            "object_hash_mismatch",
            "broken_root_chain",
            "broken_segment_chain",
            "sequence_gap",
            "generation_mismatch",
            "duplicate_started",
            "duplicate_terminal",
            "invalid_transition",
            "current_view_mismatch",
            "invalid_candidate_state",
            "stored_resource_limit",
        ],
        "terminal_variants": [
            "StorageUnavailable",
            "NamespaceAlreadyClaimed",
            "CommitIndeterminate",
            "Poisoned",
        ],
    }
    if errors != expected_errors:
        raise VerificationError("error taxonomy differs from ADR-0007")

    machine = require_object(spec["state_machine"], "spec.state_machine")
    require_exact_keys(
        machine, {"absent", "dual_slot", "open", "terminal"}, "spec.state_machine"
    )
    for key, item in machine.items():
        require_string_list(item, f"spec.state_machine.{key}")
    expected_dual_slot = [
        "E/E=fresh-or-genesis-prepublish",
        "E/P(G0)=resume-exact-genesis-publish",
        "P(G0)/E=accepted-genesis",
        "P(Rn)/P(Rn-1)=accepted-active-with-direct-parent",
        "P(Rn)/P(Rn+1)=accepted-active-with-unpromoted-direct-child",
        "malformed-slot-pointer=CorruptStore.noncanonical_pointer",
        "active-chain-alternate-g0=CorruptStore.broken_root_chain",
        "all-other-valid-pointer-relations=CorruptStore.invalid_candidate_state",
    ]
    if machine["dual_slot"] != expected_dual_slot:
        raise VerificationError(
            "dual-slot startup state machine differs from frozen contract"
        )

    topology = require_object(spec["storage_topology"], "spec.storage_topology")
    require_exact_keys(
        topology,
        {
            "cas_candidate_reader",
            "cas_namespace_enum",
            "directory",
            "lease_source",
            "object_directory",
            "pointer_slots",
            "writer_model",
        },
        "spec.storage_topology",
    )
    if topology != {
        "cas_candidate_reader": (
            "read_candidate_bounded(maximum_bytes: u64) -> std::io::Result<Option<Vec<u8>>>"
        ),
        "cas_namespace_enum": "ImmutableLedgerNamespace::ExecutionObservationFixture",
        "directory": "execution-observation-fixture-ledger",
        "lease_source": "Arc<PersonalVaultStorage>",
        "object_directory": "objects",
        "pointer_slots": ["roots/active", "roots/candidate"],
        "writer_model": "single-process/same-vault-lease",
    }:
        raise VerificationError("storage topology differs from ADR-0007")

    scope = require_object(spec["developer_scope"], "spec.developer_scope")
    require_exact_keys(
        scope,
        {
            "active_work_package",
            "architecture_owned",
            "forbidden_exact",
            "forbidden_prefixes",
            "observation_file_max_bytes",
            "observation_file_max_lines_exclusive",
            "work_packages",
        },
        "spec.developer_scope",
    )
    for key in (
        "architecture_owned",
        "forbidden_exact",
        "forbidden_prefixes",
    ):
        item = scope[key]
        require_string_list(item, f"spec.developer_scope.{key}", sorted_unique=True)
    if (
        scope["observation_file_max_bytes"] != 65_536
        or scope["observation_file_max_lines_exclusive"] != 300
    ):
        raise VerificationError(
            "observation source file limits differ from the frozen scope"
        )
    if scope["active_work_package"] != "WP2":
        raise VerificationError("WP2 checkpoint may authorize only WP2")
    work_packages = require_object(
        scope["work_packages"], "spec.developer_scope.work_packages"
    )
    require_exact_keys(work_packages, {"WP2"}, "spec.developer_scope.work_packages")
    wp1 = require_object(work_packages["WP2"], "spec.developer_scope.work_packages.WP2")
    require_exact_keys(
        wp1,
        {"allowed_exact", "allowed_globs", "allowed_prefixes"},
        "spec.developer_scope.work_packages.WP2",
    )
    for key in ("allowed_exact", "allowed_globs", "allowed_prefixes"):
        require_string_list(
            wp1[key],
            f"spec.developer_scope.work_packages.WP2.{key}",
            sorted_unique=True,
        )
    expected_wp1 = [
        "src/memory/execution_observation/mod.rs",
        "src/memory/execution_observation/store/loader.rs",
        "src/memory/execution_observation/store/mod.rs",
        "src/memory/execution_observation/store/publisher.rs",
        "src/memory/execution_observation/store/slots.rs",
        "src/memory/execution_observation/store/tests.rs",
    ]
    if (
        wp1["allowed_exact"] != expected_wp1
        or wp1["allowed_globs"] != []
        or wp1["allowed_prefixes"] != []
    ):
        raise VerificationError("WP2 exact allowlist differs from the frozen scope")
    preflight = require_object(
        spec["developer_self_preflight"], "spec.developer_self_preflight"
    )
    require_exact_keys(
        preflight,
        {
            "authorization",
            "command",
            "gate_eligible",
            "schema",
            "self_evidence_only",
        },
        "spec.developer_self_preflight",
    )
    if preflight != {
        "authorization": "unverified",
        "command": (
            "python3 -B scripts/milestones/v53/developer_preflight.py "
            "--repo <CHECKOUT> --base <A3_COMMIT> --candidate HEAD --require-clean"
        ),
        "gate_eligible": False,
        "schema": "plico.v53.wp2-r2-developer-self-preflight/v1",
        "self_evidence_only": True,
    }:
        raise VerificationError("developer self-preflight contract differs")
    forbidden_wp1 = {
        "src/cas/INDEX.md",
        "src/cas/execution_observation_store.rs",
        "src/cas/execution_observation_store/tests.rs",
        "src/cas/ledger_store.rs",
        "src/cas/mod.rs",
        "src/memory/INDEX.md",
        "src/memory/execution_observation/canonical.rs",
        "src/memory/execution_observation/canonical/tests.rs",
        "src/memory/execution_observation/counterexample_tests.rs",
        "src/memory/execution_observation/current_view.rs",
        "src/memory/execution_observation/error.rs",
        "src/memory/execution_observation/error/tests.rs",
        "src/memory/execution_observation/fault.rs",
        "src/memory/execution_observation/field_reject_tests.rs",
        "src/memory/execution_observation/hash.rs",
        "src/memory/execution_observation/hash/tests.rs",
        "src/memory/execution_observation/ids.rs",
        "src/memory/execution_observation/ids/tests.rs",
        "src/memory/execution_observation/model.rs",
        "src/memory/execution_observation/model/event.rs",
        "src/memory/execution_observation/model/ledger.rs",
        "src/memory/execution_observation/model/request.rs",
        "src/memory/execution_observation/tests.rs",
        "src/memory/execution_observation/tests/fixtures.rs",
        "src/memory/execution_observation/validation.rs",
        "src/memory/execution_observation/validation/tests.rs",
        "src/memory/mod.rs",
    }
    if not forbidden_wp1.issubset(scope["forbidden_exact"]):
        raise VerificationError(
            "WP2 accepted-model/frozen-CAS exclusions are incomplete"
        )
    if set(scope["architecture_owned"]) & set(wp1["allowed_exact"]):
        raise VerificationError("architecture-owned path is developer-allowed")

    lifecycle = require_object(
        spec["fixed_lifecycle_recipe"], "spec.fixed_lifecycle_recipe"
    )
    require_exact_keys(
        lifecycle,
        {
            "absence_assertions",
            "base_allowed_mutations",
            "comparison",
            "fixture",
            "normalization",
            "operations",
            "success_exit",
        },
        "spec.fixed_lifecycle_recipe",
    )
    for key in ("absence_assertions", "base_allowed_mutations", "operations"):
        require_string_list(lifecycle[key], f"spec.fixed_lifecycle_recipe.{key}")
    for key in ("comparison", "fixture", "normalization"):
        require_string(lifecycle[key], f"spec.fixed_lifecycle_recipe.{key}")
    if lifecycle["success_exit"] != 0:
        raise VerificationError("lifecycle success exit must be zero")

    tests = require_object(spec["test_contract"], "spec.test_contract")
    expected_ids = {f"F{index:02d}" for index in range(1, 17)}
    require_exact_keys(tests, expected_ids, "spec.test_contract")
    for test_id in sorted(tests):
        item = require_object(tests[test_id], f"spec.test_contract.{test_id}")
        require_exact_keys(
            item,
            {"minimum_tests", "prefix", "work_package"},
            f"spec.test_contract.{test_id}",
        )
        if item["minimum_tests"] != 1:
            raise VerificationError(f"{test_id} minimum test count must be one")
        if item["prefix"] != f"execution_observation_{test_id.lower()}_":
            raise VerificationError(f"{test_id} test prefix changed")
        if item["work_package"] not in {f"WP{index}" for index in range(1, 6)}:
            raise VerificationError(f"{test_id} has invalid work package")

    coverage = require_object(spec["coverage_contract"], "spec.coverage_contract")
    require_exact_keys(
        coverage,
        {
            "baseline_global",
            "environment",
            "exact_lcov_command",
            "global_minimum_percent",
            "observation_minimum_percent",
            "required_work_packages",
            "timeout_seconds",
        },
        "spec.coverage_contract",
    )
    baseline_coverage = require_object(
        coverage["baseline_global"], "spec.coverage_contract.baseline_global"
    )
    require_exact_keys(
        baseline_coverage,
        {"executable_lines", "hit_lines", "percent"},
        "spec.coverage_contract.baseline_global",
    )
    if baseline_coverage != {
        "executable_lines": 63_776,
        "hit_lines": 54_742,
        "percent": "85.83",
    }:
        raise VerificationError("coverage baseline differs from the frozen measurement")
    measured = (Decimal(54_742) * Decimal(100) / Decimal(63_776)).quantize(
        Decimal("0.01"), rounding=ROUND_DOWN
    )
    if str(measured) != baseline_coverage["percent"]:
        raise VerificationError(
            "coverage baseline numerator/denominator does not yield frozen percent"
        )
    if (
        coverage["exact_lcov_command"]
        != "cargo llvm-cov --locked --lib --all-features --lcov --output-path <LCOV>"
    ):
        raise VerificationError("LCOV command differs from the frozen command")
    if (
        coverage["global_minimum_percent"] != "85.83"
        or coverage["observation_minimum_percent"] != "95.00"
    ):
        raise VerificationError("coverage threshold differs from the frozen threshold")
    if coverage["required_work_packages"] != ["WP5", "WP6"]:
        raise VerificationError("coverage-required work packages differ")
    environment = require_object(
        coverage["environment"], "spec.coverage_contract.environment"
    )
    require_exact_keys(
        environment,
        {"EMBEDDING_BACKEND", "LLM_BACKEND"},
        "spec.coverage_contract.environment",
    )
    if environment != {"EMBEDDING_BACKEND": "stub", "LLM_BACKEND": "stub"}:
        raise VerificationError(
            "coverage environment differs from the frozen stub environment"
        )
    if coverage["timeout_seconds"] != 1800:
        raise VerificationError("coverage timeout differs from the frozen timeout")

    gate = require_object(spec["local_gate_contract"], "spec.local_gate_contract")
    require_exact_keys(
        gate,
        {
            "approval",
            "authorized_work_package",
            "execution",
            "external_services",
            "freshness",
            "integration_branch",
            "required_commands",
            "required_environment",
        },
        "spec.local_gate_contract",
    )
    if (
        gate["authorized_work_package"] != "WP2"
        or gate["execution"] != "local_only"
        or gate["external_services"] != "forbidden"
        or gate["integration_branch"] != "v53-integration"
    ):
        raise VerificationError("local gate execution boundary differs")
    gate_approval = require_object(
        gate["approval"], "spec.local_gate_contract.approval"
    )
    require_exact_keys(
        gate_approval,
        {
            "approval_path",
            "attestation",
            "decision",
            "default_ref",
            "manual_review_required",
            "packet_authorization",
            "review_method",
            "tag_prefix",
        },
        "spec.local_gate_contract.approval",
    )
    if gate_approval != {
        "approval_path": "docs/milestones/v53-wp2-r2-approval.json",
        "attestation": "unsigned_repository_control",
        "decision": "GO",
        "default_ref": "refs/remotes/origin/v53-integration",
        "manual_review_required": True,
        "packet_authorization": "unverified",
        "review_method": "manual_review",
        "tag_prefix": "v53-wp2-r2-v1-",
    }:
        raise VerificationError("local Git approval contract differs")
    freshness = require_object(gate["freshness"], "spec.local_gate_contract.freshness")
    require_exact_keys(
        freshness,
        {"maximum_generation_clock_skew_seconds", "maximum_ttl_seconds"},
        "spec.local_gate_contract.freshness",
    )
    if freshness != {
        "maximum_generation_clock_skew_seconds": 300,
        "maximum_ttl_seconds": 1_209_600,
    }:
        raise VerificationError("packet freshness contract differs")
    gate_environment = require_object(
        gate["required_environment"], "spec.local_gate_contract.required_environment"
    )
    require_exact_keys(
        gate_environment,
        {
            "CARGO_NET_OFFLINE",
            "CARGO_TARGET_DIR",
            "EMBEDDING_BACKEND",
            "LLM_BACKEND",
            "PYTHONDONTWRITEBYTECODE",
            "UV_CACHE_DIR",
            "UV_PROJECT_ENVIRONMENT",
        },
        "spec.local_gate_contract.required_environment",
    )
    if gate_environment != {
        "CARGO_NET_OFFLINE": "true",
        "CARGO_TARGET_DIR": "<OUTSIDE_REPO>/cargo-target",
        "EMBEDDING_BACKEND": "stub",
        "LLM_BACKEND": "stub",
        "PYTHONDONTWRITEBYTECODE": "1",
        "UV_CACHE_DIR": "<OUTSIDE_REPO>/uv-cache",
        "UV_PROJECT_ENVIRONMENT": "<OUTSIDE_REPO>/benchmark-venv",
    }:
        raise VerificationError("local gate environment differs")
    required_commands = require_string_list(
        gate["required_commands"], "spec.local_gate_contract.required_commands"
    )
    expected_commands = [
        "EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --offline --lib --all-features",
        "EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --locked --offline --all-features",
        "cargo clippy --locked --offline --all-targets --all-features -- -D warnings",
        "cargo fmt --all -- --check",
        "EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --locked --offline --lib --all-features --fail-under-lines 85",
        "cargo build --locked --offline --release --all-features --bins",
        "cd benchmarks && uv sync --locked --offline --extra dev",
        "cd benchmarks && uv run --offline --no-sync ruff check src tests",
        "cd benchmarks && uv run --offline --no-sync ruff format --check src tests",
        "cd benchmarks && uv run --offline --no-sync pytest -q",
        "python3 -B scripts/milestones/v53/test_v53_tools.py -v",
        "python3 -B scripts/milestones/v53/test_v53_authorize.py -v",
    ]
    if required_commands != expected_commands:
        raise VerificationError(
            "local gate commands differ from the frozen command set"
        )

    toolchain = require_object(spec["toolchain"], "spec.toolchain")
    require_exact_keys(
        toolchain,
        {
            "cargo",
            "cargo_llvm_cov",
            "clippy",
            "git",
            "pytest",
            "python",
            "ruff",
            "rustc",
            "rustfmt",
            "uv",
        },
        "spec.toolchain",
    )
    for name, item_value in toolchain.items():
        item = require_object(item_value, f"spec.toolchain.{name}")
        require_exact_keys(
            item,
            {"command", "expected", "required_lines", "source"},
            f"spec.toolchain.{name}",
        )
        require_string_list(item["command"], f"spec.toolchain.{name}.command")
        require_string(item["expected"], f"spec.toolchain.{name}.expected")
        require_string_list(
            item["required_lines"], f"spec.toolchain.{name}.required_lines"
        )
        require_string(item["source"], f"spec.toolchain.{name}.source")
    expected_rust_source = (
        "rust-toolchain.toml channel=1.95.0 plus exact release/commit assertion"
    )
    if (
        toolchain["cargo"]["source"] != expected_rust_source
        or toolchain["rustc"]["source"] != expected_rust_source
    ):
        raise VerificationError("Rust toolchain source is not release-pinned")

    bindings = require_string_list(
        spec["required_bindings"], "spec.required_bindings", sorted_unique=True
    )
    required_architecture = set(scope["architecture_owned"])
    approval_path = gate_approval["approval_path"]
    if not (required_architecture - {approval_path}).issubset(bindings):
        raise VerificationError("not every architecture-owned file is bound")
    if approval_path not in required_architecture or approval_path in bindings:
        raise VerificationError(
            "post-packet approval record must be architecture-owned and absent from packet bindings"
        )
    if any(path.startswith(".github/workflows/") for path in bindings):
        raise VerificationError("hosted workflow files must not be WP2 bindings")
    for required in (
        "Cargo.toml",
        "Cargo.lock",
        "benchmarks/pyproject.toml",
        "benchmarks/uv.lock",
    ):
        if required not in bindings:
            raise VerificationError(f"required toolchain binding is absent: {required}")

    lockfiles = require_object(spec["lockfiles"], "spec.lockfiles")
    require_exact_keys(
        lockfiles, {"Cargo.lock", "benchmarks/uv.lock"}, "spec.lockfiles"
    )
    for path, item_value in lockfiles.items():
        item = require_object(item_value, f"spec.lockfiles.{path}")
        require_exact_keys(item, {"bytes", "sha256"}, f"spec.lockfiles.{path}")
        if not isinstance(item["bytes"], int) or item["bytes"] <= 0:
            raise VerificationError(f"invalid lockfile byte count: {path}")
        _require_sha(item["sha256"], f"spec.lockfiles.{path}.sha256", HEX_SHA256)

    unsupported = require_string_list(spec["unsupported"], "spec.unsupported")
    if (
        "live producer" not in unsupported
        or "public reader or operation" not in unsupported
    ):
        raise VerificationError("unsupported boundaries are incomplete")
    return spec


def run_git(
    repo: Path,
    args: list[str],
    *,
    input_bytes: bytes | None = None,
    git_executable: Path | None = None,
) -> bytes:
    executable = (
        Path("/usr/bin/git") if git_executable is None else Path(git_executable)
    )
    if not executable.is_absolute():
        raise VerificationError("Git executable must be an absolute path")
    command = [
        os.fspath(executable),
        "--no-pager",
        "--no-replace-objects",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.preloadIndex=false",
        "-C",
        os.fspath(repo),
        *args,
    ]
    environment = {
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_NO_LAZY_FETCH": "1",
        "GIT_NO_REPLACE_OBJECTS": "1",
        "GIT_OPTIONAL_LOCKS": "0",
        "GIT_PAGER": "cat",
        "GIT_TERMINAL_PROMPT": "0",
        "LANG": "C",
        "LC_ALL": "C",
        "PATH": os.defpath,
    }
    try:
        result = subprocess.run(
            command,
            env=environment,
            input=input_bytes,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except OSError as error:
        raise VerificationError(f"cannot execute git: {error}") from error
    if result.returncode != 0:
        detail = result.stderr.decode("utf-8", errors="replace").strip().splitlines()
        suffix = detail[-1] if detail else "unknown git failure"
        raise VerificationError(f"git {' '.join(args[:2])} failed: {suffix}")
    return result.stdout


def resolve_commit(
    repo: Path, revision: str, *, git_executable: Path | None = None
) -> str:
    value = run_git(
        repo,
        ["rev-parse", "--verify", f"{revision}^{{commit}}"],
        git_executable=git_executable,
    )
    result = value.decode("ascii", errors="strict").strip()
    return _require_sha(result, "resolved commit", GIT_OBJECT_ID)


def git_status_clean(repo: Path, *, git_executable: Path | None = None) -> None:
    status = run_git(
        repo,
        ["status", "--porcelain=v1", "--untracked-files=all"],
        git_executable=git_executable,
    )
    if status:
        raise VerificationError("repository worktree is not clean")


def git_object(
    repo: Path,
    commit: str,
    path: str,
    *,
    git_executable: Path | None = None,
) -> tuple[str, str, bytes]:
    listing = run_git(
        repo,
        ["ls-tree", "-z", commit, "--", path],
        git_executable=git_executable,
    )
    records = [record for record in listing.split(b"\0") if record]
    if len(records) != 1:
        raise VerificationError(f"bound path is missing or ambiguous in Git: {path}")
    try:
        header, listed_path = records[0].split(b"\t", 1)
        mode_b, kind_b, object_b = header.split(b" ", 2)
        listed = listed_path.decode("utf-8", errors="strict")
        mode = mode_b.decode("ascii")
        kind = kind_b.decode("ascii")
        object_id = object_b.decode("ascii")
    except (ValueError, UnicodeDecodeError) as error:
        raise VerificationError(f"invalid ls-tree result for {path}") from error
    if listed != path or kind != "blob" or mode not in {"100644", "100755"}:
        raise VerificationError(f"bound path is not a regular tracked blob: {path}")
    _require_sha(object_id, f"Git object id for {path}", GIT_OBJECT_ID)
    data = run_git(
        repo,
        ["cat-file", "blob", object_id],
        git_executable=git_executable,
    )
    return mode, object_id, data


def verify_predecessor_history(
    repo: Path,
    implementation_base: str,
    predecessors: list[str],
    *,
    git_executable: Path | None = None,
) -> None:
    """Prove that the checkpoint base descends from every frozen R1/tooling commit."""

    for index, predecessor in enumerate(predecessors):
        resolved = resolve_commit(repo, predecessor, git_executable=git_executable)
        if resolved != predecessor:
            raise VerificationError(
                f"predecessor commit {index} does not resolve canonically"
            )
        try:
            output = run_git(
                repo,
                ["merge-base", "--is-ancestor", predecessor, implementation_base],
                git_executable=git_executable,
            )
        except VerificationError as error:
            raise VerificationError(
                f"implementation base does not descend from predecessor {predecessor}"
            ) from error
        if output:
            raise VerificationError("unexpected merge-base ancestry output")


def validate_bound_documents(spec: dict[str, Any], objects: dict[str, bytes]) -> None:
    adr_path = spec["accepted_adr"]["path"]
    contract_path = spec["contract"]["path"]
    for path in (adr_path, contract_path, "Cargo.lock"):
        if path not in objects:
            raise VerificationError(f"bound document is absent: {path}")

    adr = objects[adr_path].decode("utf-8", errors="strict")
    adr_contract = spec["accepted_adr"]
    if not adr.startswith(adr_contract["required_heading"] + "\n"):
        raise VerificationError(
            "Accepted ADR heading does not match the frozen heading"
        )
    for token in [adr_contract["required_status"], *adr_contract["required_phrases"]]:
        if token not in adr:
            raise VerificationError(f"Accepted ADR is missing required text: {token}")

    contract = objects[contract_path].decode("utf-8", errors="strict")
    for token in (
        spec["contract_version"],
        spec["contract"]["required_state"],
        "f60eec14da37b107a595f9f93e739a6c06bd6672",
        "Durable Store",
        "R2-R01",
        "CommitIndeterminate",
    ):
        if token not in contract:
            raise VerificationError(
                f"milestone contract is missing frozen text: {token}"
            )

    cargo_lock = objects["Cargo.lock"].decode("utf-8", errors="strict")
    if not re.search(r"(?m)^version\s*=\s*4\s*$", cargo_lock):
        raise VerificationError("tracked Cargo.lock is absent or not lock format v4")
    for path, identity in spec["lockfiles"].items():
        data = objects.get(path)
        if (
            data is None
            or len(data) != identity["bytes"]
            or sha256_bytes(data) != identity["sha256"]
        ):
            raise VerificationError(f"frozen lockfile identity differs: {path}")

    rust_toolchain = objects.get("rust-toolchain.toml")
    if rust_toolchain is None:
        raise VerificationError("rust-toolchain.toml is not bound")
    rust_toolchain_text = rust_toolchain.decode("utf-8", errors="strict")
    for token in (
        'channel = "1.95.0"',
        'profile = "minimal"',
        '"clippy"',
        '"rustfmt"',
        '"llvm-tools-preview"',
    ):
        if token not in rust_toolchain_text:
            raise VerificationError(
                f"rust-toolchain.toml is missing frozen token: {token}"
            )

    spec_source = objects.get(SPEC_PATH)
    if spec_source is None:
        raise VerificationError("WP2 spec Git object is not bound")
    if strict_json_loads(spec_source, "bound wp2_spec.json") != spec:
        raise VerificationError("embedded WP2 spec differs from its Git object")


def _tool_launcher(command: str, repo: Path | None) -> Path:
    if os.sep in command:
        path = Path(command)
        if not path.is_absolute():
            if repo is None:
                raise VerificationError(
                    f"relative tool launcher requires repo: {command}"
                )
            path = repo / path
        return path.absolute()
    located = shutil.which(command)
    if located is None:
        raise VerificationError(f"tool launcher is absent from PATH: {command}")
    return Path(located).absolute()


def _tool_file_digest(path: Path, location: str) -> str:
    try:
        realpath = path.resolve(strict=True)
        info = realpath.stat()
        data = realpath.read_bytes()
    except OSError as error:
        raise VerificationError(
            f"{location} launcher identity cannot be read: {error}"
        ) from error
    if not stat.S_ISREG(info.st_mode):
        raise VerificationError(
            f"{location} launcher does not resolve to a regular file"
        )
    return sha256_bytes(data)


def _uv_environment_root(repo: Path | None, environment: dict[str, str]) -> Path:
    configured = environment.get("UV_PROJECT_ENVIRONMENT")
    if configured:
        candidate = Path(configured)
        if not candidate.is_absolute():
            if repo is None:
                raise VerificationError(
                    "relative UV_PROJECT_ENVIRONMENT requires repository context"
                )
            candidate = repo / candidate
    elif repo is not None:
        candidate = repo / "benchmarks/.venv"
    else:
        raise VerificationError("UV_PROJECT_ENVIRONMENT is required")
    try:
        resolved = candidate.resolve(strict=True)
    except OSError as error:
        raise VerificationError(f"uv environment is unavailable: {error}") from error
    if not resolved.is_dir():
        raise VerificationError("uv environment is not a directory")
    return resolved


def _normalized_python_entrypoint(
    path: Path, environment_root: Path, location: str
) -> tuple[str, str, int, Path]:
    try:
        data = path.read_bytes()
    except OSError as error:
        raise VerificationError(f"{location} cannot be read: {error}") from error
    first_line, separator, body = data.partition(b"\n")
    if not separator or not first_line.startswith(b"#!"):
        raise VerificationError(f"{location} has no canonical Python shebang")
    try:
        interpreter_text = first_line[2:].decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise VerificationError(f"{location} shebang is not UTF-8") from error
    if not interpreter_text or any(
        character.isspace() for character in interpreter_text
    ):
        raise VerificationError(f"{location} shebang is not a single interpreter path")
    interpreter = Path(interpreter_text)
    expected_bin = environment_root / "bin"
    if (
        not interpreter.is_absolute()
        or interpreter.parent != expected_bin
        or not interpreter.name.startswith("python")
    ):
        raise VerificationError(f"{location} shebang escapes the uv environment")
    normalized = b"#!<UV_PYTHON>\n" + body
    return (
        sha256_bytes(normalized),
        _tool_file_digest(interpreter, f"{location} interpreter"),
        len(normalized),
        interpreter,
    )


def _python_distribution_digest(
    environment_root: Path,
    *,
    distribution: str,
    version: str,
    entrypoint: Path,
) -> str:
    records = sorted(
        (environment_root / "lib").glob(
            f"python*/site-packages/{distribution}-{version}.dist-info/RECORD"
        )
    )
    if len(records) != 1:
        raise VerificationError(
            f"expected exactly one installed {distribution} {version} RECORD"
        )
    record = records[0]
    site_packages = record.parent.parent
    entrypoint_sha, interpreter_sha, _, _ = _normalized_python_entrypoint(
        entrypoint, environment_root, f"resolved {distribution}"
    )
    manifest: list[dict[str, object]] = []
    try:
        rows = csv.reader(
            record.read_text(encoding="utf-8", errors="strict").splitlines()
        )
        for index, row in enumerate(rows, start=1):
            if len(row) != 3:
                raise VerificationError(
                    f"{distribution} RECORD row {index} is not three fields"
                )
            relative, encoded_digest, encoded_size = row
            if relative == f"{distribution}-{version}.dist-info/RECORD":
                if encoded_digest or encoded_size:
                    raise VerificationError(
                        f"{distribution} RECORD self-entry must be unhashed"
                    )
                continue
            if relative in {"../../../bin/pytest", "../../../bin/py.test"}:
                target = environment_root / "bin" / Path(relative).name
                target_sha, target_interpreter_sha, target_bytes, _ = (
                    _normalized_python_entrypoint(
                        target, environment_root, f"resolved {distribution} entrypoint"
                    )
                )
                if target_interpreter_sha != interpreter_sha:
                    raise VerificationError(
                        f"{distribution} entrypoints use different interpreters"
                    )
                manifest.append(
                    {
                        "bytes": target_bytes,
                        "path": f"bin/{target.name}",
                        "sha256": target_sha,
                    }
                )
                continue
            relative_path = Path(relative)
            if (
                relative_path.is_absolute()
                or ".." in relative_path.parts
                or "\\" in relative
            ):
                raise VerificationError(
                    f"{distribution} RECORD contains a non-portable path"
                )
            target = site_packages / relative_path
            try:
                info = target.lstat()
            except OSError as error:
                raise VerificationError(
                    f"{distribution} RECORD target cannot be read: {relative}: {error}"
                ) from error
            if not stat.S_ISREG(info.st_mode):
                raise VerificationError(
                    f"{distribution} RECORD target is not a regular file: {relative}"
                )
            try:
                data = target.read_bytes()
            except OSError as error:
                raise VerificationError(
                    f"{distribution} RECORD target cannot be read: {relative}: {error}"
                ) from error
            if not encoded_digest.startswith("sha256=") or not encoded_size.isdigit():
                raise VerificationError(
                    f"{distribution} RECORD target lacks sha256/size: {relative}"
                )
            try:
                digest_payload = encoded_digest.removeprefix("sha256=")
                padded_payload = digest_payload + "=" * (-len(digest_payload) % 4)
                declared = base64.b64decode(
                    padded_payload,
                    altchars=b"-_",
                    validate=True,
                )
            except (binascii.Error, ValueError, TypeError) as error:
                raise VerificationError(
                    f"{distribution} RECORD target has invalid sha256: {relative}"
                ) from error
            if (
                base64.urlsafe_b64encode(declared).rstrip(b"=").decode("ascii")
                != digest_payload
            ):
                raise VerificationError(
                    f"{distribution} RECORD target has noncanonical sha256: {relative}"
                )
            declared_hex = declared.hex()
            actual = sha256_bytes(data)
            if declared_hex != actual or int(encoded_size) != len(data):
                raise VerificationError(
                    f"{distribution} RECORD target identity differs: {relative}"
                )
            manifest.append({"bytes": len(data), "path": relative, "sha256": actual})
    except (OSError, UnicodeDecodeError, csv.Error) as error:
        raise VerificationError(
            f"cannot parse {distribution} RECORD: {error}"
        ) from error
    identity = {
        "distribution": distribution,
        "entrypoint_sha256": entrypoint_sha,
        "files": manifest,
        "interpreter_sha256": interpreter_sha,
        "schema": "plico.python-distribution-identity/v1",
        "version": version,
    }
    return sha256_bytes(canonical_json(identity))


def _observe_tool(
    name: str,
    entry: dict[str, Any],
    repo: Path | None,
    *,
    environment: dict[str, str],
) -> dict[str, Any]:
    """Observe a tool without serializing a host or checkout path."""

    launcher = _tool_launcher(entry["command"][0], repo)
    resolved_tool: dict[str, str] | None = None
    if name in {"pytest", "ruff"}:
        environment_root = _uv_environment_root(repo, environment)
        tool_name = entry["command"][-2]
        resolved_executable = environment_root / "bin" / tool_name
        if name == "pytest":
            version = entry["expected"].removeprefix("pytest ")
            resolved_sha = _python_distribution_digest(
                environment_root,
                distribution="pytest",
                version=version,
                entrypoint=resolved_executable,
            )
            _, _, _, interpreter = _normalized_python_entrypoint(
                resolved_executable, environment_root, "resolved pytest"
            )
            metadata_matches = sorted(
                (environment_root / "lib").glob(
                    f"python*/site-packages/pytest-{version}.dist-info/METADATA"
                )
            )
            if len(metadata_matches) != 1:
                raise VerificationError(
                    f"expected exactly one installed pytest {version} METADATA"
                )
            command = [
                os.fspath(interpreter),
                "-I",
                "-B",
                "-S",
                "-c",
                (
                    "from email.parser import BytesParser;import sys;"
                    "m=BytesParser().parse(open(sys.argv[1],'rb'),headersonly=True);"
                    "print(f\"{m['Name']} {m['Version']}\")"
                ),
                os.fspath(metadata_matches[0]),
            ]
        else:
            resolved_sha = _tool_file_digest(resolved_executable, "resolved ruff")
            command = [os.fspath(resolved_executable), "--version"]
        resolved_tool = {"name": tool_name, "sha256": resolved_sha}
    else:
        command = [os.fspath(launcher), *entry["command"][1:]]
    execution_environment = environment.copy()
    for variable in (
        "PYTHONBREAKPOINT",
        "PYTHONHOME",
        "PYTHONINSPECT",
        "PYTHONPATH",
        "PYTHONSTARTUP",
        "PYTHONUSERBASE",
        "PYTHONWARNINGS",
        "PYTEST_ADDOPTS",
        "PYTEST_PLUGINS",
        "VIRTUAL_ENV",
        "_OLD_VIRTUAL_PATH",
    ):
        execution_environment.pop(variable, None)
    execution_environment.update(
        {
            "PYTHONDONTWRITEBYTECODE": "1",
            "PYTHONNOUSERSITE": "1",
            "PYTHONSAFEPATH": "1",
        }
    )
    try:
        result = subprocess.run(
            command,
            cwd=repo,
            env=execution_environment,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise VerificationError(
            f"toolchain command failed for {name}: {error}"
        ) from error
    text = result.stdout.decode("utf-8", errors="replace").strip().splitlines()
    normalized_lines = [line.strip() for line in text]
    first_line = normalized_lines[0] if normalized_lines else ""
    if result.returncode != 0 or first_line != entry["expected"]:
        raise VerificationError(
            f"toolchain mismatch for {name}: expected {entry['expected']!r}, "
            f"got {first_line!r}"
        )
    for required_line in entry["required_lines"]:
        if required_line not in normalized_lines:
            raise VerificationError(
                f"toolchain mismatch for {name}: missing exact line {required_line!r}"
            )

    if name == "cargo":
        rustup = _tool_launcher("rustup", repo)
        try:
            resolved = subprocess.run(
                [os.fspath(rustup), "which", "cargo", "--toolchain", "1.95.0"],
                cwd=repo,
                env=execution_environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                check=False,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise VerificationError(f"resolved cargo lookup failed: {error}") from error
        resolved_path_text = resolved.stdout.decode("utf-8", errors="replace").strip()
        if resolved.returncode != 0 or not resolved_path_text:
            raise VerificationError("resolved cargo 1.95.0 lookup failed")
        resolved_tool = {
            "name": "cargo-1.95.0",
            "sha256": _tool_file_digest(Path(resolved_path_text), "resolved cargo"),
        }
    elif name == "cargo_llvm_cov":
        resolved_tool = {
            "name": "cargo-llvm-cov",
            "sha256": _tool_file_digest(
                _tool_launcher("cargo-llvm-cov", repo),
                "resolved cargo-llvm-cov",
            ),
        }
    return {
        "launcher_name": entry["command"][0],
        "launcher_sha256": _tool_file_digest(launcher, name),
        "resolved_tool": resolved_tool,
        "role": name,
        "version": first_line,
    }


def validate_toolchain(
    spec: dict[str, Any], repo: Path | None = None
) -> dict[str, Any]:
    observed: dict[str, Any] = {}
    with tempfile.TemporaryDirectory(prefix="plico-v53-tool-cache-") as cache:
        environment = os.environ.copy()
        environment.update(
            {
                "CARGO_NET_OFFLINE": "true",
                "PYTHONDONTWRITEBYTECODE": "1",
                "UV_CACHE_DIR": cache,
            }
        )
        for name, entry in spec["toolchain"].items():
            observed[name] = _observe_tool(
                name,
                entry,
                repo,
                environment=environment,
            )
    return observed


def _open_dir_no_symlinks(path: Path) -> int:
    raw = os.fspath(path)
    if not raw:
        raise VerificationError("empty artifact path")
    absolute = os.path.isabs(raw)
    parts = [part for part in Path(raw).parts if part not in {os.sep, "", "."}]
    if ".." in parts:
        raise VerificationError("artifact path may not contain '..'")
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC
    nofollow = getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(os.sep if absolute else ".", flags)
    try:
        for part in parts:
            next_fd = os.open(part, flags | nofollow, dir_fd=fd)
            os.close(fd)
            fd = next_fd
        return fd
    except OSError as error:
        os.close(fd)
        raise VerificationError(
            f"artifact directory is inaccessible or crosses a symlink: {error}"
        ) from error


def _read_packet_file(directory_fd: int, name: str) -> bytes:
    flags = os.O_RDONLY | os.O_CLOEXEC | getattr(os, "O_NOFOLLOW", 0)
    try:
        fd = os.open(name, flags, dir_fd=directory_fd)
    except OSError as error:
        raise VerificationError(f"cannot open packet file {name}: {error}") from error
    try:
        before = os.fstat(fd)
        if not stat.S_ISREG(before.st_mode):
            raise VerificationError(f"packet entry is not a regular file: {name}")
        if before.st_uid != os.geteuid() or stat.S_IMODE(before.st_mode) != 0o600:
            raise VerificationError(
                f"packet entry owner/mode is not current-euid/0600: {name}"
            )
        if before.st_nlink != 1:
            raise VerificationError(f"packet entry must not be hard-linked: {name}")
        if before.st_size > MAX_PACKET_FILE_BYTES:
            raise VerificationError(f"packet entry exceeds size limit: {name}")
        chunks: list[bytes] = []
        remaining = before.st_size
        while remaining:
            chunk = os.read(fd, min(remaining, 1024 * 1024))
            if not chunk:
                raise VerificationError(f"packet entry changed while reading: {name}")
            chunks.append(chunk)
            remaining -= len(chunk)
        if os.read(fd, 1):
            raise VerificationError(f"packet entry grew while reading: {name}")
        after = os.fstat(fd)
        if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
            after.st_dev,
            after.st_ino,
            after.st_size,
            after.st_mtime_ns,
        ):
            raise VerificationError(f"packet entry changed while reading: {name}")
        return b"".join(chunks)
    finally:
        os.close(fd)


def read_packet(artifact_dir: Path) -> dict[str, bytes]:
    directory_fd = _open_dir_no_symlinks(artifact_dir)
    try:
        info = os.fstat(directory_fd)
        if not stat.S_ISDIR(info.st_mode):
            raise VerificationError("artifact path is not a directory")
        if info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) != 0o700:
            raise VerificationError(
                "artifact directory owner/mode is not current-euid/0700"
            )
        names = sorted(os.listdir(directory_fd))
        if names != sorted(PACKET_FILES):
            raise VerificationError(
                f"packet inventory is not exact; expected={sorted(PACKET_FILES)}, actual={names}"
            )
        return {name: _read_packet_file(directory_fd, name) for name in PACKET_FILES}
    finally:
        os.close(directory_fd)


def _validate_binding(value: Any, index: int) -> dict[str, Any]:
    binding = require_object(value, f"handoff.bindings[{index}]")
    require_exact_keys(
        binding,
        {"bytes", "git_blob", "mode", "path", "sha256"},
        f"handoff.bindings[{index}]",
    )
    path = require_string(binding["path"], f"handoff.bindings[{index}].path")
    if path.startswith("/") or ".." in Path(path).parts or "\\" in path:
        raise VerificationError(f"binding path is not canonical repo-relative: {path}")
    if binding["mode"] not in {"100644", "100755"}:
        raise VerificationError(f"binding has forbidden Git mode: {path}")
    if (
        not isinstance(binding["bytes"], int)
        or not 0 <= binding["bytes"] <= MAX_SAFE_INTEGER
    ):
        raise VerificationError(f"binding has invalid byte count: {path}")
    _require_sha(binding["sha256"], f"binding sha256 for {path}", HEX_SHA256)
    _require_sha(binding["git_blob"], f"binding Git blob for {path}", GIT_OBJECT_ID)
    return binding


def _reject_nonportable_handoff_value(value: Any, location: str = "handoff") -> None:
    """Reject host/user/check-out paths from the serialized packet contract."""

    if isinstance(value, str):
        windows_home = re.search(r"(?i)(?:^|[\\/])users[\\/]", value)
        windows_absolute = re.match(r"(?i)^[a-z]:[\\/]", value)
        unc_absolute = value.startswith(("\\\\", "//"))
        home_relative = value == "~" or value.startswith(("~/", "~\\"))
        if (
            value.startswith("/")
            or "/home/" in value
            or "/Users/" in value
            or windows_home
            or windows_absolute
            or unc_absolute
            or home_relative
            or value.lower().startswith("file://")
        ):
            raise VerificationError(
                f"non-portable absolute or user-home path is forbidden at {location}"
            )
    elif isinstance(value, list):
        for index, item in enumerate(value):
            _reject_nonportable_handoff_value(item, f"{location}[{index}]")
    elif isinstance(value, dict):
        for key, item in value.items():
            _reject_nonportable_handoff_value(item, f"{location}.{key}")


def validate_handoff(value: Any, *, now: dt.datetime | None = None) -> dict[str, Any]:
    handoff = require_object(value, "handoff")
    _reject_nonportable_handoff_value(handoff)
    require_exact_keys(
        handoff,
        {
            "authorization",
            "bindings",
            "contract_version",
            "expires_at_utc",
            "generated_at_utc",
            "implementation_base_sha",
            "implementation_base_tree",
            "packet_id",
            "product_baseline_sha",
            "schema",
            "spec",
            "toolchain_observed",
        },
        "handoff",
    )
    if handoff["schema"] != HANDOFF_SCHEMA:
        raise VerificationError("unsupported handoff schema")
    packet_id = require_string(handoff["packet_id"], "handoff.packet_id", 64)
    if not PACKET_ID.fullmatch(packet_id):
        raise VerificationError("handoff packet id is not canonical")
    generated = parse_utc(handoff["generated_at_utc"], "handoff.generated_at_utc")
    expires = parse_utc(handoff["expires_at_utc"], "handoff.expires_at_utc")
    observed_now = now or dt.datetime.now(dt.timezone.utc)
    if observed_now.tzinfo is None:
        raise VerificationError("verification time must be timezone-aware")
    observed_now = observed_now.astimezone(dt.timezone.utc)
    if expires <= generated:
        raise VerificationError("handoff expiry is not after generation")
    _require_sha(
        handoff["product_baseline_sha"], "handoff.product_baseline_sha", GIT_OBJECT_ID
    )
    _require_sha(
        handoff["implementation_base_sha"],
        "handoff.implementation_base_sha",
        GIT_OBJECT_ID,
    )
    _require_sha(
        handoff["implementation_base_tree"],
        "handoff.implementation_base_tree",
        GIT_OBJECT_ID,
    )

    spec = validate_spec(handoff["spec"])
    if handoff["contract_version"] != spec["contract_version"]:
        raise VerificationError("handoff and spec contract versions differ")
    if handoff["product_baseline_sha"] != spec["product_baseline_sha"]:
        raise VerificationError("handoff and spec product baselines differ")
    freshness = spec["local_gate_contract"]["freshness"]
    if generated > observed_now + dt.timedelta(
        seconds=freshness["maximum_generation_clock_skew_seconds"]
    ):
        raise VerificationError("handoff generation time exceeds maximum clock skew")
    if (expires - generated).total_seconds() > freshness["maximum_ttl_seconds"]:
        raise VerificationError("handoff TTL exceeds the frozen maximum")
    if observed_now >= expires:
        raise VerificationError("handoff packet has expired")

    authorization = require_object(handoff["authorization"], "handoff.authorization")
    require_exact_keys(
        authorization,
        {"approval_path", "state"},
        "handoff.authorization",
    )
    if authorization != {
        "approval_path": spec["local_gate_contract"]["approval"]["approval_path"],
        "state": "unverified",
    }:
        raise VerificationError("packet authorization must remain unverified")

    if not isinstance(handoff["bindings"], list):
        raise VerificationError("handoff.bindings must be a list")
    bindings = [
        _validate_binding(item, index) for index, item in enumerate(handoff["bindings"])
    ]
    paths = [item["path"] for item in bindings]
    if paths != sorted(paths) or len(paths) != len(set(paths)):
        raise VerificationError("handoff bindings must be sorted and unique")
    if paths != spec["required_bindings"]:
        raise VerificationError(
            "handoff bindings do not exactly match spec.required_bindings"
        )

    observed = require_object(
        handoff["toolchain_observed"], "handoff.toolchain_observed"
    )
    require_exact_keys(observed, set(spec["toolchain"]), "handoff.toolchain_observed")
    for name, value in observed.items():
        identity = require_object(value, f"handoff.toolchain_observed.{name}")
        require_exact_keys(
            identity,
            {
                "launcher_name",
                "launcher_sha256",
                "resolved_tool",
                "role",
                "version",
            },
            f"handoff.toolchain_observed.{name}",
        )
        if identity["role"] != name:
            raise VerificationError(f"sealed tool role mismatch: {name}")
        if identity["launcher_name"] != spec["toolchain"][name]["command"][0]:
            raise VerificationError(f"sealed tool launcher name mismatch: {name}")
        if (
            require_string(
                identity["version"], f"handoff.toolchain_observed.{name}.version"
            )
            != spec["toolchain"][name]["expected"]
        ):
            raise VerificationError(f"sealed toolchain observation mismatch: {name}")
        _require_sha(
            identity["launcher_sha256"],
            f"handoff.toolchain_observed.{name}.launcher_sha256",
            HEX_SHA256,
        )
        resolved = identity["resolved_tool"]
        if name in {"cargo", "cargo_llvm_cov", "pytest", "ruff"}:
            resolved_object = require_object(
                resolved,
                f"handoff.toolchain_observed.{name}.resolved_tool",
            )
            require_exact_keys(
                resolved_object,
                {"name", "sha256"},
                f"handoff.toolchain_observed.{name}.resolved_tool",
            )
            expected_resolved_name = {
                "cargo": "cargo-1.95.0",
                "cargo_llvm_cov": "cargo-llvm-cov",
                "pytest": "pytest",
                "ruff": "ruff",
            }[name]
            if (
                require_string(resolved_object["name"], f"resolved {name} name")
                != expected_resolved_name
            ):
                raise VerificationError(f"resolved tool name mismatch: {name}")
            _require_sha(
                resolved_object["sha256"],
                f"resolved {name} sha256",
                HEX_SHA256,
            )
        elif resolved is not None:
            raise VerificationError(f"unexpected resolved tool identity for {name}")
    return handoff


def verify_handoff(
    artifact_dir: Path,
    *,
    repo: Path | None = None,
    require_head: bool = False,
    check_toolchain: bool = False,
    now: dt.datetime | None = None,
    git_executable: Path | None = None,
) -> dict[str, Any]:
    files = read_packet(artifact_dir)
    parsed: dict[str, Any] = {}
    for name, data in files.items():
        if name != "LOCK":
            value = strict_json_loads(data, name)
            if canonical_json(value) != data:
                raise VerificationError(f"packet JSON is not canonical: {name}")
            parsed[name] = value
    lock = strict_json_loads(files["LOCK"], "LOCK")
    if canonical_json(lock) != files["LOCK"]:
        raise VerificationError("packet JSON is not canonical: LOCK")
    lock = require_object(lock, "LOCK")
    require_exact_keys(lock, {"packet_id", "schema"}, "LOCK")
    if lock["schema"] != LOCK_SCHEMA:
        raise VerificationError("unsupported LOCK schema")

    handoff = validate_handoff(parsed["handoff.json"], now=now)
    if lock["packet_id"] != handoff["packet_id"]:
        raise VerificationError("LOCK packet id differs from handoff")

    sidecar = require_object(parsed["handoff.sha256.json"], "handoff.sha256.json")
    require_exact_keys(
        sidecar,
        {"algorithm", "artifact", "bytes", "schema", "sha256"},
        "handoff.sha256.json",
    )
    if (
        sidecar["schema"] != DIGEST_SCHEMA
        or sidecar["algorithm"] != "sha256"
        or sidecar["artifact"] != "handoff.json"
    ):
        raise VerificationError("invalid handoff digest sidecar identity")
    if sidecar["bytes"] != len(files["handoff.json"]):
        raise VerificationError("handoff digest sidecar byte count differs")
    if sidecar["sha256"] != sha256_bytes(files["handoff.json"]):
        raise VerificationError("handoff digest differs")

    committed = require_object(parsed["COMMITTED"], "COMMITTED")
    require_exact_keys(
        committed,
        {"handoff_sha256", "packet_id", "schema", "sidecar_sha256"},
        "COMMITTED",
    )
    if (
        committed["schema"] != COMMIT_SCHEMA
        or committed["packet_id"] != handoff["packet_id"]
    ):
        raise VerificationError("COMMITTED identity differs from handoff")
    if committed["handoff_sha256"] != sha256_bytes(files["handoff.json"]):
        raise VerificationError("COMMITTED handoff digest differs")
    if committed["sidecar_sha256"] != sha256_bytes(files["handoff.sha256.json"]):
        raise VerificationError("COMMITTED sidecar digest differs")

    if repo is not None:
        repo = Path(repo)
        base = resolve_commit(
            repo,
            handoff["implementation_base_sha"],
            git_executable=git_executable,
        )
        if base != handoff["implementation_base_sha"]:
            raise VerificationError("implementation-base object id is not canonical")
        verify_predecessor_history(
            repo,
            base,
            handoff["spec"]["predecessor_commits"],
            git_executable=git_executable,
        )
        tree = run_git(
            repo,
            ["rev-parse", f"{base}^{{tree}}"],
            git_executable=git_executable,
        )
        if tree.decode("ascii").strip() != handoff["implementation_base_tree"]:
            raise VerificationError("implementation-base tree digest differs")
        if require_head:
            if resolve_commit(repo, "HEAD", git_executable=git_executable) != base:
                raise VerificationError(
                    "repository HEAD differs from implementation base"
                )
            git_status_clean(repo, git_executable=git_executable)
        objects: dict[str, bytes] = {}
        for binding in handoff["bindings"]:
            mode, object_id, data = git_object(
                repo,
                base,
                binding["path"],
                git_executable=git_executable,
            )
            if mode != binding["mode"] or object_id != binding["git_blob"]:
                raise VerificationError(f"Git identity differs for {binding['path']}")
            if len(data) != binding["bytes"] or sha256_bytes(data) != binding["sha256"]:
                raise VerificationError(
                    f"Git object bytes differ for {binding['path']}"
                )
            objects[binding["path"]] = data
        validate_bound_documents(handoff["spec"], objects)
    if check_toolchain:
        observed = validate_toolchain(handoff["spec"], repo)
        if observed != handoff["toolchain_observed"]:
            raise VerificationError(
                "current toolchain output differs from sealed observations"
            )
    return handoff


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--repo", type=Path)
    parser.add_argument("--require-head", action="store_true")
    parser.add_argument("--check-toolchain", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(sys.argv[1:] if argv is None else argv)
    try:
        handoff = verify_handoff(
            args.artifact_dir,
            repo=args.repo,
            require_head=args.require_head,
            check_toolchain=args.check_toolchain,
        )
    except VerificationError as error:
        print(f"v53 WP2 verification failed: {error}", file=sys.stderr)
        return 1
    print(
        f"v53 WP2-R2 verified: packet={handoff['packet_id']} "
        f"implementation_base={handoff['implementation_base_sha']} "
        "integrity=verified authorization=unverified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
