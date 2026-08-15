"""Strict, content-free schemas for P3-A dogfood evidence."""

from __future__ import annotations

import hashlib
import json
import re
from datetime import datetime
from pathlib import PurePosixPath
from typing import Any, Literal
from uuid import UUID

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

from plico_benchmarks.core.client import PROTOCOL, PUBLIC_OPERATION_CATALOG

DOGFOOD_DIGEST_SCHEMA = "plico.p3a.dogfood-evidence-digest/v1"
MAX_SAFE_JSON_INTEGER = 9_007_199_254_740_991
MUTATING_OPERATIONS = (
    "object.put",
    "memory.create",
    "projection.rebuild",
    "memory.update",
    "memory.delete",
    "session.start",
    "session.end",
)

_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_SAFE_LABEL = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,255}$")
_TIMESTAMP = "%Y-%m-%dT%H:%M:%SZ"
_RESULT_ASSERTIONS = {
    "capabilities.describe": "catalog_exact_14",
    "runtime.readiness": "canonical_and_projection_ready",
    "object.put": "canonical_commit",
    "object.get": "target_content_verified",
    "object.search": "target_object_hit",
    "memory.create": "canonical_commit",
    "memory.get": "target_revision_verified",
    "memory.recall": "target_revision_hit_lexical",
    "projection.status": "ready_observed",
    "projection.rebuild": "durable_receipt_canonical_unchanged",
    "memory.update": "replacement_head_verified",
    "memory.delete": "tombstone_and_recall_absence_verified",
    "session.start": "session_started",
    "session.end": "watermark_monotonic",
}


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


def canonical_uuid(value: str) -> str:
    try:
        parsed = UUID(value)
    except (ValueError, AttributeError) as error:
        raise ValueError("value must be a canonical UUID") from error
    if parsed.int == 0 or str(parsed) != value:
        raise ValueError("value must be a canonical non-nil UUID")
    return value


def sha256(value: str) -> str:
    if not _SHA256.fullmatch(value):
        raise ValueError("value must be a lowercase SHA-256 digest")
    return value


def safe_label(value: str) -> str:
    if not _SAFE_LABEL.fullmatch(value) or "://" in value:
        raise ValueError("value must be a bounded non-secret identity label")
    return value


def provider_compatibility_id(value: dict[str, Any]) -> str:
    """Reproduce Rust's Ollama plus adaptive provider compatibility identity."""
    domain = b"plico.embedding.provider-compatibility.v1\0"
    base_evidence = {
        "schema": "plico.embedding.ollama-evidence/v1",
        "model_tag": value["exact_model_tag"],
        "model_digest": value["model_digest_before"],
        "server_version": value["server_version"],
        "api_contract": value["api_contract"],
        "raw_dimension": value["raw_dimension"],
    }
    base = hashlib.sha256(domain + _jcs(base_evidence)).hexdigest()
    adaptive_evidence = {
        "schema": "plico.embedding.adaptive-contract/v1",
        "inner_provider_compatibility_id": base,
        "prefix_contract_id": value["adaptive_prefix_contract_id"],
        "requested_target_dimension": value["requested_target_dimension"],
        "effective_dimension": value["effective_dimension"],
        "normalization": value["normalization"],
    }
    return hashlib.sha256(domain + _jcs(adaptive_evidence)).hexdigest()


def builder_spec_hash(value: dict[str, Any]) -> str:
    spec = {
        "schema": "plico.projection.builder-spec/v1",
        "projection_kind": "memory_embedding",
        "builder_id": "plico.memory-embedding",
        "builder_version": "p3a-controller-v1",
        "provider_family": "ollama",
        "provider_compatibility_id": value["provider_compatibility_id"],
        "model_id": value["exact_model_tag"],
        "raw_dimension": value["raw_dimension"],
        "dimension": value["effective_dimension"],
        "input_contract": "memory_text_utf8_v1",
        "operation_contract": "document_v1",
        "normalization": value["normalization"],
        "transform_contract_id": value["transform_contract_id"],
        "artifact_schema": "plico.projection.embedding-artifact/v1",
    }
    return hashlib.sha256(b"plico.projection.builder-spec.v1\0" + _jcs(spec)).hexdigest()


def _jcs(value: Any) -> bytes:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode()


class CanonicalWatermark(StrictModel):
    generation: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    revision_watermark: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    policy_watermark: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    relation_watermark: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)


class SeedEvidence(StrictModel):
    memory_id: str
    revision_id: str
    content_evidence_ref: str

    _memory_uuid = field_validator("memory_id")(canonical_uuid)
    _revision_uuid = field_validator("revision_id")(canonical_uuid)

    @field_validator("content_evidence_ref")
    @classmethod
    def evidence_ref(cls, value: str) -> str:
        return safe_label(value)


class MemoryGetEvidence(SeedEvidence):
    result: Literal["found"]


class MemoryRecallEvidence(StrictModel):
    strategy: Literal["lexical_overlap"]
    target_memory_id: str
    target_revision_id: str
    content_evidence_ref: str
    match_count: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    target_found: Literal[True]

    _memory_uuid = field_validator("target_memory_id")(canonical_uuid)
    _revision_uuid = field_validator("target_revision_id")(canonical_uuid)

    @field_validator("content_evidence_ref")
    @classmethod
    def evidence_ref(cls, value: str) -> str:
        return safe_label(value)


class StatusEvidence(StrictModel):
    kind: Literal["memory_embedding"]
    observation: Literal["observed"]
    state: Literal["ready"]
    event_watermark: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    reconciled_source: CanonicalWatermark
    memory_id: str
    revision_id: str
    content_evidence_ref: str
    builder_compatibility_id: str

    _memory_uuid = field_validator("memory_id")(canonical_uuid)
    _revision_uuid = field_validator("revision_id")(canonical_uuid)
    _builder_hash = field_validator("builder_compatibility_id")(sha256)

    @field_validator("content_evidence_ref")
    @classmethod
    def evidence_ref(cls, value: str) -> str:
        return safe_label(value)


class RebuildEvidence(StrictModel):
    kind: Literal["memory_embedding"]
    selected_count: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    manifest_generation: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    event_watermark: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    reconciled_source: CanonicalWatermark
    canonical_ledger_unchanged: Literal[True]
    memory_id: str
    revision_id: str
    builder_compatibility_id: str
    receipt_id: str

    _memory_uuid = field_validator("memory_id")(canonical_uuid)
    _revision_uuid = field_validator("revision_id")(canonical_uuid)
    _builder_hash = field_validator("builder_compatibility_id")(sha256)
    _receipt_uuid = field_validator("receipt_id")(canonical_uuid)


class OperationCapture(StrictModel):
    wire_operation: str
    request_id: str
    attempt_count: Literal[1]
    frame_count: Literal[1]
    typed_response_ok: Literal[True]
    result_assertion: str
    typed_result_evidence: (
        MemoryGetEvidence | MemoryRecallEvidence | StatusEvidence | RebuildEvidence | None
    ) = None

    _request_uuid = field_validator("request_id")(canonical_uuid)

    @model_validator(mode="after")
    def exact_operation_evidence(self) -> OperationCapture:
        expected = _RESULT_ASSERTIONS.get(self.wire_operation)
        if expected is None or self.result_assertion != expected:
            raise ValueError("operation result assertion does not match the public catalog")
        if self.wire_operation == "memory.get":
            if not isinstance(self.typed_result_evidence, MemoryGetEvidence):
                raise ValueError("memory.get requires its exact typed evidence")
        elif self.wire_operation == "memory.recall":
            if not isinstance(self.typed_result_evidence, MemoryRecallEvidence):
                raise ValueError("memory.recall requires its exact typed evidence")
        elif self.wire_operation == "projection.status":
            if not isinstance(self.typed_result_evidence, StatusEvidence):
                raise ValueError("projection.status requires its exact typed evidence")
        elif self.wire_operation == "projection.rebuild":
            if not isinstance(self.typed_result_evidence, RebuildEvidence):
                raise ValueError("projection.rebuild requires its exact typed evidence")
        elif self.typed_result_evidence is not None:
            raise ValueError("only memory retrieval and projection operations accept evidence")
        return self


class DisconnectCapture(StrictModel):
    wire_operation: str
    request_id: str
    attempt_count: Literal[1]
    frame_count: Literal[1]
    response_observed: Literal[False]
    outcome: Literal["ambiguous_commit_no_retry"]

    _request_uuid = field_validator("request_id")(canonical_uuid)


class V1RejectCapture(StrictModel):
    protocol: Literal["plico.personal.v1"]
    wire_operation: Literal["memory.get"]
    request_id: str
    attempt_count: Literal[1]
    frame_count: Literal[1]
    response_category: Literal["unsupported_protocol"]
    predispatch_rejected: Literal[True]
    authentication_invoked: Literal[False]
    daemon_instance_id: str

    _request_uuid = field_validator("request_id")(canonical_uuid)
    _daemon_uuid = field_validator("daemon_instance_id")(canonical_uuid)


class V1ZeroStateEvidence(StrictModel):
    schema_id: Literal["plico.p3a.v1-zero-state-evidence/v1"] = Field(alias="schema")
    before_sha256: str
    after_sha256: str
    canonical_entry_count: int = Field(gt=0, le=4096)
    projection_entry_count: int = Field(gt=0, le=4096)
    unchanged: Literal[True]

    _before_hash = field_validator("before_sha256")(sha256)
    _after_hash = field_validator("after_sha256")(sha256)

    @model_validator(mode="after")
    def exact_match(self) -> V1ZeroStateEvidence:
        if self.before_sha256 != self.after_sha256:
            raise ValueError("v1 zero-state changed during the rejected request")
        return self


class UdsCapture(StrictModel):
    kind: Literal["uds"]
    file_type: Literal["socket"]
    mode: Literal["0600"]
    owner_matches_effective_user: Literal[True]


class OllamaCapture(StrictModel):
    family: Literal["ollama"]
    evidence_schema: Literal["plico.embedding.ollama-evidence/v1"]
    exact_model_tag: str
    exact_tag_match_count: Literal[1]
    model_digest_before: str
    model_digest_after: str
    server_version: str
    api_contract: Literal["ollama-api-embed-truncate-false/v1"]
    provider_compatibility_id: str
    raw_dimension: int = Field(gt=0, le=65_536)
    effective_dimension: int = Field(gt=0, le=65_536)
    requested_target_dimension: int | None = Field(default=None, gt=0, le=65_536)
    adaptive_prefix_contract_id: Literal[
        "provider-native-input-v1", "qwen3-web-search-query-document-native-v1"
    ]
    input_contract: Literal["memory_text_utf8_v1"]
    operation_contract: Literal["document_v1"]
    normalization: Literal["provider_native", "l2_after_matryoshka_truncation_v1"]
    transform_contract_id: Literal["provider-native-document-v1", "plico-matryoshka-truncate-l2-v1"]
    identity_verified_before_and_after: Literal[True]
    fallback_used: Literal[False]

    _before_hash = field_validator("model_digest_before")(sha256)
    _after_hash = field_validator("model_digest_after")(sha256)
    _compat_hash = field_validator("provider_compatibility_id")(sha256)

    @field_validator("exact_model_tag")
    @classmethod
    def exact_tag(cls, value: str) -> str:
        safe_label(value)
        if ":" not in value:
            raise ValueError("Ollama evidence requires the explicitly configured full model tag")
        return value

    @field_validator("server_version")
    @classmethod
    def version_label(cls, value: str) -> str:
        return safe_label(value)

    @model_validator(mode="after")
    def coherent_identity(self) -> OllamaCapture:
        if self.model_digest_before != self.model_digest_after:
            raise ValueError("Ollama model digest changed during the capture")
        native = self.normalization == "provider_native"
        if native and (
            self.raw_dimension != self.effective_dimension
            or self.transform_contract_id != "provider-native-document-v1"
        ):
            raise ValueError("provider-native identity has an invalid dimension contract")
        if not native and (
            self.raw_dimension <= self.effective_dimension
            or self.transform_contract_id != "plico-matryoshka-truncate-l2-v1"
        ):
            raise ValueError("Matryoshka identity has an invalid dimension contract")
        if self.requested_target_dimension not in {None, self.effective_dimension}:
            raise ValueError("requested target dimension differs from the effective dimension")
        if (
            provider_compatibility_id(self.model_dump(mode="json"))
            != self.provider_compatibility_id
        ):
            raise ValueError("Ollama compatibility identity differs from Rust's frozen domain")
        return self


class FileCapture(StrictModel):
    path: str
    bytes: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    sha256: str

    _hash = field_validator("sha256")(sha256)

    @field_validator("path")
    @classmethod
    def relative_path(cls, value: str) -> str:
        path = PurePosixPath(value)
        if (
            not value
            or value.startswith(("/", "~"))
            or "\\" in value
            or path.is_absolute()
            or ".." in path.parts
            or str(path) != value
        ):
            raise ValueError("source manifest paths must be normalized relative POSIX paths")
        forbidden_parts = {".git", ".venv", "__pycache__", ".pytest_cache"}
        if (
            forbidden_parts & set(path.parts)
            or any(part == ".env" or part.startswith(".env.") for part in path.parts)
            or path.suffix in {".pyc", ".pyo"}
        ):
            raise ValueError("source manifest must exclude secrets, VCS, environments, and caches")
        return value


class BinaryCapture(StrictModel):
    role: Literal["plicod_binary"]
    file_name: Literal["plicod"]
    bytes: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    sha256: str
    trust: Literal["sealed_owner_only_executable_0700"]

    _hash = field_validator("sha256")(sha256)


class RestartState(StrictModel):
    daemon_instance_id: str
    projection_observation: Literal["observed"]
    projection_state: Literal["ready"]
    canonical_store_ready: Literal[True]
    canonical_memory_persistence_ready: Literal[True]
    projection_control_plane_ready: Literal[True]
    projection_worker_ready: Literal[True]
    revision_id: str
    builder_compatibility_id: str
    event_watermark: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    reconciled_source: CanonicalWatermark
    rebuild_receipt_id: str

    _daemon_uuid = field_validator("daemon_instance_id")(canonical_uuid)
    _revision_uuid = field_validator("revision_id")(canonical_uuid)
    _builder_hash = field_validator("builder_compatibility_id")(sha256)
    _receipt_uuid = field_validator("rebuild_receipt_id")(canonical_uuid)


class PostRestartVerification(StrictModel):
    memory_get: MemoryGetEvidence
    memory_recall: MemoryRecallEvidence
    projection_status: StatusEvidence

    @model_validator(mode="after")
    def same_target(self) -> PostRestartVerification:
        if (
            self.memory_get.memory_id != self.memory_recall.target_memory_id
            or self.memory_get.revision_id != self.memory_recall.target_revision_id
            or self.memory_get.content_evidence_ref != self.memory_recall.content_evidence_ref
            or self.projection_status.memory_id != self.memory_get.memory_id
            or self.projection_status.revision_id != self.memory_get.revision_id
            or self.projection_status.content_evidence_ref != self.memory_get.content_evidence_ref
        ):
            raise ValueError("post-restart responses do not bind one seeded memory revision")
        return self


class RestartCapture(StrictModel):
    status: Literal["passed"]
    shutdown_flush_observed: Literal[True]
    new_daemon_process_started: Literal[True]
    pre_restart: RestartState
    post_restart: RestartState

    @model_validator(mode="after")
    def distinct_daemon_instances(self) -> RestartCapture:
        if self.pre_restart.daemon_instance_id == self.post_restart.daemon_instance_id:
            raise ValueError("restart evidence must bind two distinct daemon instances")
        before = self.pre_restart.model_dump(exclude={"daemon_instance_id"})
        after = self.post_restart.model_dump(exclude={"daemon_instance_id"})
        if before != after:
            raise ValueError("restart must preserve the exact revision, builder, and watermarks")
        return self


class CanonicalFingerprintCapture(StrictModel):
    schema_id: Literal["plico.canonical-tree-fingerprint/v1"] = Field(alias="schema")
    before_projection_rebuild_sha256: str
    after_projection_rebuild_sha256: str
    before_restart_sha256: str
    after_restart_sha256: str
    unchanged_across_projection_rebuild: Literal[True]
    unchanged_across_restart: Literal[True]

    _before = field_validator("before_projection_rebuild_sha256")(sha256)
    _after = field_validator("after_projection_rebuild_sha256")(sha256)
    _restart_before = field_validator("before_restart_sha256")(sha256)
    _restart = field_validator("after_restart_sha256")(sha256)

    @model_validator(mode="after")
    def exact_fingerprint(self) -> CanonicalFingerprintCapture:
        if (
            len(
                {
                    self.before_projection_rebuild_sha256,
                    self.after_projection_rebuild_sha256,
                    self.before_restart_sha256,
                    self.after_restart_sha256,
                }
            )
            != 1
        ):
            raise ValueError("canonical fingerprint changed across dogfood checkpoints")
        return self


class TraceCapture(StrictModel):
    schema_id: Literal["plico.p3a.dogfood-trace/v1"] = Field(alias="schema")
    bytes: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    records: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    public_request_count: Literal[14]
    disconnect_request_count: Literal[7]
    auxiliary_request_count: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    sha256: str
    privacy_canary_scan_passed: Literal[True]

    _hash = field_validator("sha256")(sha256)


class RealLlmReaderCapture(StrictModel):
    status: Literal["passed"]
    workflow_run_id: str
    backend: Literal["openai-compatible"]
    model: str
    fallback_used: Literal[False]
    seeded_object_present_in_evidence_ids: Literal[True]
    seeded_memory_present_in_evidence_ids: Literal[True]
    reported_citations_subset_of_evidence_ids: Literal[True]
    workflow_performed_no_object_or_memory_writeback: Literal[True]
    model_answer_body_recorded: Literal[False]

    _run_uuid = field_validator("workflow_run_id")(canonical_uuid)

    @field_validator("model")
    @classmethod
    def real_model(cls, value: str) -> str:
        safe_label(value)
        if any(marker in value.lower() for marker in ("stub", "mock", "fallback")):
            raise ValueError("reader evidence must name a real model")
        return value


class RealLlmReaderEvidence(RealLlmReaderCapture):
    trace_sha256: str
    trace_bytes: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    trace_records: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    trace_request_count: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    privacy_canary_scan_passed: Literal[True]

    _trace_hash = field_validator("trace_sha256")(sha256)


class DogfoodCapture(StrictModel):
    schema_id: Literal["plico.p3a.dogfood-capture/v1"] = Field(alias="schema")
    protocol: Literal["plico.personal.v2"]
    bundle_run_id: str
    captured_at_utc: str
    transport: UdsCapture
    embedding_provider: OllamaCapture
    seed_evidence: SeedEvidence
    request_ledger: list[OperationCapture]
    disconnect_cases: list[DisconnectCapture]
    v1_reject: V1RejectCapture
    restart_replay: RestartCapture
    real_llm_reader: RealLlmReaderCapture

    _bundle_uuid = field_validator("bundle_run_id")(canonical_uuid)

    @field_validator("captured_at_utc")
    @classmethod
    def timestamp(cls, value: str) -> str:
        try:
            parsed = datetime.strptime(value, _TIMESTAMP)
        except ValueError as error:
            raise ValueError("captured_at_utc must be UTC seconds with a Z suffix") from error
        if parsed.strftime(_TIMESTAMP) != value:
            raise ValueError("captured_at_utc is not canonical")
        return value

    @model_validator(mode="after")
    def exact_ledgers(self) -> DogfoodCapture:
        operations = tuple(item.wire_operation for item in self.request_ledger)
        if operations != PUBLIC_OPERATION_CATALOG:
            raise ValueError("request ledger must contain exact ordered personal.v2 operations")
        request_ids = [item.request_id for item in self.request_ledger]
        if len(request_ids) != len(set(request_ids)):
            raise ValueError("request ledger request IDs must be unique")
        disconnect = tuple(item.wire_operation for item in self.disconnect_cases)
        if disconnect != MUTATING_OPERATIONS:
            raise ValueError("disconnect ledger must contain exact ordered mutating operations")
        disconnect_ids = [item.request_id for item in self.disconnect_cases]
        if len(disconnect_ids) != len(set(disconnect_ids)) or set(disconnect_ids) & set(
            request_ids
        ):
            raise ValueError("all evidence request IDs must be unique")
        if self.v1_reject.request_id in set((*request_ids, *disconnect_ids)):
            raise ValueError("v1 rejection request ID must be unique")
        if self.v1_reject.daemon_instance_id != self.restart_replay.pre_restart.daemon_instance_id:
            raise ValueError("v1 rejection must bind the pre-restart daemon instance")
        status = self.request_ledger[8].typed_result_evidence
        rebuild = self.request_ledger[9].typed_result_evidence
        memory_get = self.request_ledger[6].typed_result_evidence
        memory_recall = self.request_ledger[7].typed_result_evidence
        if (
            not isinstance(memory_get, MemoryGetEvidence)
            or not isinstance(memory_recall, MemoryRecallEvidence)
            or not isinstance(status, StatusEvidence)
            or not isinstance(rebuild, RebuildEvidence)
            or memory_get.memory_id != self.seed_evidence.memory_id
            or memory_get.revision_id != self.seed_evidence.revision_id
            or memory_get.content_evidence_ref != self.seed_evidence.content_evidence_ref
            or memory_recall.target_memory_id != self.seed_evidence.memory_id
            or memory_recall.target_revision_id != self.seed_evidence.revision_id
            or memory_recall.content_evidence_ref != self.seed_evidence.content_evidence_ref
            or status.memory_id != self.seed_evidence.memory_id
            or status.revision_id != self.seed_evidence.revision_id
            or status.content_evidence_ref != self.seed_evidence.content_evidence_ref
            or rebuild.memory_id != self.seed_evidence.memory_id
            or rebuild.reconciled_source != status.reconciled_source
            or rebuild.event_watermark <= status.event_watermark
            or rebuild.revision_id != status.revision_id
            or rebuild.builder_compatibility_id != status.builder_compatibility_id
        ):
            raise ValueError("projection status and rebuild watermarks are inconsistent")
        restart = self.restart_replay.pre_restart
        expected_builder = builder_spec_hash(self.embedding_provider.model_dump(mode="json"))
        if (
            restart.revision_id != rebuild.revision_id
            or restart.builder_compatibility_id != rebuild.builder_compatibility_id
            or restart.event_watermark != rebuild.event_watermark
            or restart.reconciled_source != rebuild.reconciled_source
            or restart.rebuild_receipt_id != rebuild.receipt_id
            or status.builder_compatibility_id != expected_builder
        ):
            raise ValueError("restart does not bind the durable projection rebuild receipt")
        return self


class SourceManifest(StrictModel):
    schema_id: Literal["plico.source-file-manifest/v1"] = Field(alias="schema")
    file_count: int = Field(gt=0)
    total_bytes: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    aggregate_sha256: str
    row_format: Literal["canonical_jcs_file_capture_jsonl_v1"]
    inventory_rule: Literal["fixed_execution_source_selection_v1"]
    trust: Literal["live_same_euid_non_world_writable"]
    files: list[FileCapture]

    _aggregate_hash = field_validator("aggregate_sha256")(sha256)

    @model_validator(mode="after")
    def exact_aggregate(self) -> SourceManifest:
        if self.file_count != len(self.files) or self.total_bytes != sum(
            item.bytes for item in self.files
        ):
            raise ValueError("source manifest count or byte total differs")
        paths = [item.path for item in self.files]
        if paths != sorted(paths) or len(paths) != len(set(paths)):
            raise ValueError("source manifest rows must be sorted and unique")
        rows = b"".join(_jcs(item.model_dump(mode="json")) + b"\n" for item in self.files)
        if hashlib.sha256(rows).hexdigest() != self.aggregate_sha256:
            raise ValueError("source manifest aggregate differs from its rows")
        return self


class BuildEvidence(StrictModel):
    plicod_binary: BinaryCapture
    plico_source_manifest: SourceManifest
    plico_agents_source_manifest: SourceManifest
    plico_agents_uv_lock: FileCapture
    ollama_probe: FileCapture


class CanonicalCatalogEvidence(StrictModel):
    status: Literal["passed"]
    catalog_exact_order_and_unique: Literal[True]
    catalog_operation_count: Literal[14]
    observed_wire_operation_set_exact: Literal[True]
    request_count: Literal[14]
    object_target_hit_verified: Literal[True]
    memory_target_hit_verified: Literal[True]
    delete_target_absence_verified: Literal[True]
    session_watermark_monotonic: Literal[True]
    projection_status_observed: Literal[True]
    projection_rebuild_durable_receipt_verified: Literal[True]
    request_ledger: list[OperationCapture]

    @model_validator(mode="after")
    def exact_catalog(self) -> CanonicalCatalogEvidence:
        if tuple(item.wire_operation for item in self.request_ledger) != PUBLIC_OPERATION_CATALOG:
            raise ValueError("evidence does not contain the exact ordered public catalog")
        if len({item.request_id for item in self.request_ledger}) != 14:
            raise ValueError("public request IDs must be unique")
        return self


class DisconnectEvidence(StrictModel):
    status: Literal["passed"]
    cases: list[DisconnectCapture]

    @model_validator(mode="after")
    def exact_mutations(self) -> DisconnectEvidence:
        if tuple(item.wire_operation for item in self.cases) != MUTATING_OPERATIONS:
            raise ValueError("evidence does not contain the exact ordered write fault ledger")
        if len({item.request_id for item in self.cases}) != len(MUTATING_OPERATIONS):
            raise ValueError("write fault request IDs must be unique")
        return self


class PrivacyEvidence(StrictModel):
    body_recorded: Literal[False]
    query_recorded: Literal[False]
    tag_recorded: Literal[False]
    cid_recorded: Literal[False]
    bearer_or_token_recorded: Literal[False]
    provider_body_recorded: Literal[False]
    provider_secret_recorded: Literal[False]
    provider_endpoint_recorded: Literal[False]
    full_vault_path_recorded: Literal[False]
    canonical_root_or_content_hash_recorded: Literal[False]


class CaptureBinding(StrictModel):
    schema_id: Literal["plico.p3a.dogfood-capture-binding/v1"] = Field(alias="schema")
    canonical_capture_bytes: int = Field(gt=0, le=MAX_SAFE_JSON_INTEGER)
    capture_sha256: str

    _hash = field_validator("capture_sha256")(sha256)


class AuxiliaryRequestEvidence(StrictModel):
    request_count: int = Field(ge=0, le=MAX_SAFE_JSON_INTEGER)
    request_ids: list[str]

    @model_validator(mode="after")
    def exact_ids(self) -> AuxiliaryRequestEvidence:
        for request_id in self.request_ids:
            canonical_uuid(request_id)
        if self.request_count != len(self.request_ids) or len(self.request_ids) != len(
            set(self.request_ids)
        ):
            raise ValueError("auxiliary request ledger count or uniqueness differs")
        return self


class DogfoodEvidence(StrictModel):
    schema_id: Literal["plico.p3a.dogfood-evidence/v1"] = Field(alias="schema")
    protocol: Literal["plico.personal.v2"]
    bundle_run_id: str
    captured_at_utc: str
    scope: Literal["single local personal vault; no tenant or organization semantics"]
    attestation_boundary: Literal[
        "local_artifact_integrity_and_drift_detection_not_external_cryptographic_attestation"
    ]
    capture_binding: CaptureBinding
    transport: UdsCapture
    embedding_provider: OllamaCapture
    seed_evidence: SeedEvidence
    build: BuildEvidence
    canonical_catalog: CanonicalCatalogEvidence
    restart_replay: RestartCapture
    write_disconnect_no_retry: DisconnectEvidence
    v1_reject: V1RejectCapture
    v1_zero_state: V1ZeroStateEvidence
    canonical_fingerprint: CanonicalFingerprintCapture
    daemon_trace: TraceCapture
    auxiliary_requests: AuxiliaryRequestEvidence
    post_restart_verification: PostRestartVerification
    privacy: PrivacyEvidence
    real_llm_reader: RealLlmReaderEvidence

    _bundle_uuid = field_validator("bundle_run_id")(canonical_uuid)

    @field_validator("captured_at_utc")
    @classmethod
    def timestamp(cls, value: str) -> str:
        try:
            parsed = datetime.strptime(value, _TIMESTAMP)
        except ValueError as error:
            raise ValueError("captured_at_utc must be UTC seconds with a Z suffix") from error
        if parsed.strftime(_TIMESTAMP) != value:
            raise ValueError("captured_at_utc is not canonical")
        return value

    @model_validator(mode="after")
    def cross_bind_evidence(self) -> DogfoodEvidence:
        public_ids = {item.request_id for item in self.canonical_catalog.request_ledger}
        fault_ids = {item.request_id for item in self.write_disconnect_no_retry.cases}
        if public_ids & fault_ids:
            raise ValueError("public and fault request IDs must be disjoint")
        status = self.canonical_catalog.request_ledger[8].typed_result_evidence
        rebuild = self.canonical_catalog.request_ledger[9].typed_result_evidence
        expected_builder = builder_spec_hash(self.embedding_provider.model_dump(mode="json"))
        if (
            not isinstance(status, StatusEvidence)
            or not isinstance(rebuild, RebuildEvidence)
            or status.builder_compatibility_id != expected_builder
            or rebuild.builder_compatibility_id != expected_builder
            or self.restart_replay.pre_restart.builder_compatibility_id != expected_builder
            or self.restart_replay.post_restart.builder_compatibility_id != expected_builder
            or self.post_restart_verification.memory_get.memory_id != self.seed_evidence.memory_id
            or self.post_restart_verification.memory_get.revision_id
            != self.seed_evidence.revision_id
            or self.post_restart_verification.memory_get.content_evidence_ref
            != self.seed_evidence.content_evidence_ref
            or self.post_restart_verification.projection_status.builder_compatibility_id
            != expected_builder
            or self.post_restart_verification.projection_status.event_watermark
            != self.restart_replay.post_restart.event_watermark
            or self.post_restart_verification.projection_status.reconciled_source
            != self.restart_replay.post_restart.reconciled_source
        ):
            raise ValueError("projection evidence does not bind the verified builder spec")
        auxiliary_count = self.auxiliary_requests.request_count
        if (
            self.daemon_trace.auxiliary_request_count != auxiliary_count
            or self.daemon_trace.records != 46 + 2 * auxiliary_count
            or self.real_llm_reader.trace_records
            != 4 + 2 * self.real_llm_reader.trace_request_count
        ):
            raise ValueError("trace metadata does not conserve its typed request ledger")
        return self


def validate_no_sensitive_values(value: Any, key: str = "") -> None:
    """Reject common secret/body/path keys and unapproved full hashes."""
    if key.endswith("_recorded") and value is False:
        return
    forbidden_key_parts = (
        "bearer",
        "token",
        "secret",
        "password",
        "api_key",
        "authorization",
        "endpoint",
        "socket_path",
        "vault_path",
        "content_hash",
        "root_hash",
        "artifact_hash",
        "cid",
    )
    if any(part in key.lower() for part in forbidden_key_parts):
        raise ValueError("dogfood evidence contains a forbidden sensitive field")
    if isinstance(value, dict):
        for child_key, child in value.items():
            validate_no_sensitive_values(child, child_key)
        return
    if isinstance(value, list):
        for child in value:
            validate_no_sensitive_values(child, key)
        return
    if not isinstance(value, str):
        return
    if value.startswith(("/", "~/", "file://")) or re.match(r"^[A-Za-z]:[\\/]", value):
        raise ValueError("dogfood evidence contains a full host path")
    if "://" in value:
        raise ValueError("dogfood evidence contains a provider or service URL")
    approved_hash_keys = {
        "sha256",
        "aggregate_sha256",
        "capture_sha256",
        "model_digest_before",
        "model_digest_after",
        "provider_compatibility_id",
        "builder_compatibility_id",
        "before_projection_rebuild_sha256",
        "after_projection_rebuild_sha256",
        "before_restart_sha256",
        "after_restart_sha256",
        "trace_sha256",
        "before_sha256",
        "after_sha256",
    }
    if _SHA256.fullmatch(value) and key not in approved_hash_keys:
        raise ValueError("dogfood evidence contains an unapproved full hash")


def assert_protocol_constant() -> None:
    if PROTOCOL != "plico.personal.v2" or len(PUBLIC_OPERATION_CATALOG) != 14:
        raise RuntimeError("dogfood evidence producer was built against the wrong protocol")
