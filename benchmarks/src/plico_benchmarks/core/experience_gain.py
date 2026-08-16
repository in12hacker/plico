"""Fail-closed shadow metric for experience improving future action.

This module is deliberately independent from the public Plico protocol. It is
an in-memory experimental comparison contract, not a committed artifact,
product capability, or release gate.
"""

from __future__ import annotations

import hashlib
import json
import math
import re
from dataclasses import dataclass
from typing import Any

_SHA256 = re.compile(r"[0-9a-f]{64}")
_MAX_SAFE_INTEGER = (1 << 53) - 1
_MAX_FINITE_METRIC = 1_000_000_000_000.0


@dataclass(frozen=True)
class ExperimentIdentity:
    """Common protocol identity that must be byte-equivalent across both arms."""

    protocol_sha256: str
    agent_sha256: str
    model_revision_sha256: str
    toolset_sha256: str
    environment_sha256: str
    input_evidence_sha256: str
    judge_sha256: str
    permission_policy_sha256: str
    budget_sha256: str


@dataclass(frozen=True)
class ActionReceipt:
    """Content-free proof that one action belongs to an arm, task, and policy."""

    action_id: str
    run_id: str
    task_id: str
    evidence_cids: tuple[str, ...]
    permission_policy_sha256: str
    authorized: bool
    influenced_by_experience: bool


@dataclass(frozen=True)
class TaskReceipt:
    """One terminal task result from which comparison aggregates are recomputed."""

    task_id: str
    run_id: str
    action_ids: tuple[str, ...]
    success: bool
    input_tokens: int
    latency_ms: float


@dataclass(frozen=True)
class ExperienceArm:
    """One independently executed arm over one exact ordered task set."""

    run_id: str
    condition_sha256: str
    identity: ExperimentIdentity
    ordered_task_ids: tuple[str, ...]
    input_evidence_cids: tuple[str, ...]
    task_receipts: tuple[TaskReceipt, ...]
    action_receipts: tuple[ActionReceipt, ...]


@dataclass(frozen=True)
class ExperienceBudget:
    """Pre-registered shadow thresholds; no implicit project-wide defaults."""

    minimum_success_delta: float
    maximum_input_tokens_per_task: float
    maximum_candidate_token_ratio: float
    maximum_p95_latency_ms: float


def experience_budget_sha256(value: ExperienceBudget) -> str:
    """Return the canonical digest that both arms must freeze before execution."""

    _validate_budget(value)
    payload = _budget_payload(value)
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def evaluate_verified_experience_gain(
    *, candidate: ExperienceArm, control: ExperienceArm, budget: ExperienceBudget
) -> dict[str, Any]:
    """Evaluate a paired shadow comparison without granting release eligibility."""

    budget_sha256 = experience_budget_sha256(budget)
    _validate_arm(candidate, "candidate", budget_sha256)
    _validate_arm(control, "control", budget_sha256)
    if candidate.run_id == control.run_id:
        raise ValueError("candidate and control require distinct run IDs")
    if candidate.condition_sha256 == control.condition_sha256:
        raise ValueError("candidate and control require distinct condition identities")
    if candidate.identity != control.identity:
        raise ValueError("candidate and control experiment identities differ")
    if candidate.ordered_task_ids != control.ordered_task_ids:
        raise ValueError("candidate and control ordered task sets differ")
    if candidate.input_evidence_cids != control.input_evidence_cids:
        raise ValueError("candidate and control evidence inventories differ")

    evaluated_tasks = len(candidate.ordered_task_ids)
    candidate_successful_tasks = sum(receipt.success for receipt in candidate.task_receipts)
    control_successful_tasks = sum(receipt.success for receipt in control.task_receipts)
    candidate_success = candidate_successful_tasks / evaluated_tasks
    control_success = control_successful_tasks / evaluated_tasks
    success_delta = candidate_success - control_success

    influenced_receipts = tuple(
        receipt for receipt in candidate.action_receipts if receipt.influenced_by_experience
    )
    if not influenced_receipts:
        raise ValueError("candidate has no experience-influenced actions")
    traceable_actions = sum(bool(receipt.evidence_cids) for receipt in influenced_receipts)
    if traceable_actions != len(influenced_receipts):
        raise ValueError("candidate has an untraceable experience-influenced action")
    traceable_ratio = traceable_actions / len(influenced_receipts)

    candidate_input_tokens = sum(receipt.input_tokens for receipt in candidate.task_receipts)
    control_input_tokens = sum(receipt.input_tokens for receipt in control.task_receipts)
    if candidate_input_tokens > _MAX_SAFE_INTEGER or control_input_tokens > _MAX_SAFE_INTEGER:
        raise ValueError("total input token accounting exceeds the portable integer range")
    candidate_max_input_tokens = max(receipt.input_tokens for receipt in candidate.task_receipts)
    control_max_input_tokens = max(receipt.input_tokens for receipt in control.task_receipts)
    candidate_tokens_per_task = candidate_input_tokens / evaluated_tasks
    control_tokens_per_task = control_input_tokens / evaluated_tasks
    token_ratio = candidate_tokens_per_task / control_tokens_per_task
    candidate_p95_latency_ms = _nearest_rank_p95(candidate.task_receipts)
    control_p95_latency_ms = _nearest_rank_p95(control.task_receipts)
    permission_compliance = 1.0
    veg = success_delta * traceable_ratio * permission_compliance

    checks = {
        "success_delta": success_delta >= budget.minimum_success_delta,
        "candidate_input_tokens": (
            candidate_max_input_tokens <= budget.maximum_input_tokens_per_task
        ),
        "control_input_tokens": (control_max_input_tokens <= budget.maximum_input_tokens_per_task),
        "token_ratio": token_ratio <= budget.maximum_candidate_token_ratio,
        "candidate_p95_latency": (candidate_p95_latency_ms <= budget.maximum_p95_latency_ms),
        "control_p95_latency": (control_p95_latency_ms <= budget.maximum_p95_latency_ms),
        "traceable_actions": True,
        "permission_compliance": True,
    }
    budget_compliant = all(
        checks[name]
        for name in (
            "candidate_input_tokens",
            "control_input_tokens",
            "token_ratio",
            "candidate_p95_latency",
            "control_p95_latency",
        )
    )
    status = (
        "shadow_ineligible_budget"
        if not budget_compliant
        else (
            "shadow_effect_threshold_met"
            if checks["success_delta"]
            else "shadow_effect_threshold_not_met"
        )
    )
    return {
        "schema": "plico.benchmark.verified-experience-gain-shadow/v1",
        "status": status,
        "gate_eligible": False,
        "comparative_inference": "research_shadow_only",
        "experiment_identity": _identity_payload(candidate.identity),
        "ordered_task_set_sha256": _ordered_task_set_sha256(candidate.ordered_task_ids),
        "evaluated_tasks": evaluated_tasks,
        "candidate_run_id": candidate.run_id,
        "control_run_id": control.run_id,
        "candidate_condition_sha256": candidate.condition_sha256,
        "control_condition_sha256": control.condition_sha256,
        "candidate_task_success_rate": round(candidate_success, 8),
        "control_task_success_rate": round(control_success, 8),
        "candidate_successful_tasks": candidate_successful_tasks,
        "control_successful_tasks": control_successful_tasks,
        "task_success_delta": round(success_delta, 8),
        "candidate_action_receipts": len(candidate.action_receipts),
        "control_action_receipts": len(control.action_receipts),
        "candidate_experience_influenced_actions": len(influenced_receipts),
        "candidate_traceable_experience_actions": traceable_actions,
        "traceable_action_ratio": round(traceable_ratio, 8),
        "permission_compliance_rate": permission_compliance,
        "raw_signed_experience_gain": round(veg, 8),
        "verified_experience_gain": round(veg, 8) if budget_compliant else None,
        "candidate_tokens_per_task": round(candidate_tokens_per_task, 8),
        "control_tokens_per_task": round(control_tokens_per_task, 8),
        "candidate_max_input_tokens_per_task": candidate_max_input_tokens,
        "control_max_input_tokens_per_task": control_max_input_tokens,
        "candidate_total_input_tokens": candidate_input_tokens,
        "control_total_input_tokens": control_input_tokens,
        "candidate_token_ratio": round(token_ratio, 8),
        "candidate_p95_latency_ms": round(candidate_p95_latency_ms, 8),
        "control_p95_latency_ms": round(control_p95_latency_ms, 8),
        "budget": {**_budget_payload(budget), "sha256": budget_sha256},
        "checks": checks,
    }


def _validate_arm(value: ExperienceArm, label: str, budget_sha256: str) -> None:
    if not isinstance(value, ExperienceArm):
        raise ValueError(f"{label} arm has an invalid type")
    if not _is_canonical_id(value.run_id):
        raise ValueError(f"{label} run ID is empty or non-canonical")
    _require_sha256(value.condition_sha256, f"{label} condition")
    _validate_identity(value.identity, label, budget_sha256)
    if not isinstance(value.ordered_task_ids, tuple) or not value.ordered_task_ids:
        raise ValueError(f"{label} ordered task set is empty or mutable")
    if any(not _is_canonical_id(task_id) for task_id in value.ordered_task_ids):
        raise ValueError(f"{label} task ID is empty or non-canonical")
    if len(set(value.ordered_task_ids)) != len(value.ordered_task_ids):
        raise ValueError(f"{label} ordered task set contains duplicates")
    if not isinstance(value.input_evidence_cids, tuple) or not value.input_evidence_cids:
        raise ValueError(f"{label} input evidence inventory is empty or mutable")
    for evidence_cid in value.input_evidence_cids:
        _require_sha256(evidence_cid, f"{label} input evidence CID")
    if len(set(value.input_evidence_cids)) != len(value.input_evidence_cids):
        raise ValueError(f"{label} input evidence inventory contains duplicates")
    if value.identity.input_evidence_sha256 != _ordered_evidence_sha256(value.input_evidence_cids):
        raise ValueError(f"{label} input evidence digest does not match its inventory")
    _validate_task_receipts(value, label)
    if not isinstance(value.action_receipts, tuple) or not value.action_receipts:
        raise ValueError(f"{label} action receipts are empty or mutable")
    _validate_receipts(value, label)
    _validate_action_inventory(value, label)


def _validate_identity(value: ExperimentIdentity, label: str, budget_sha256: str) -> None:
    if not isinstance(value, ExperimentIdentity):
        raise ValueError(f"{label} experiment identity has an invalid type")
    for field, digest in _identity_payload(value).items():
        _require_sha256(digest, f"{label} experiment {field}")
    if value.budget_sha256 != budget_sha256:
        raise ValueError(f"{label} did not freeze the supplied budget")


def _validate_receipts(value: ExperienceArm, label: str) -> None:
    task_ids = set(value.ordered_task_ids)
    input_evidence_cids = set(value.input_evidence_cids)
    observed_tasks: set[str] = set()
    action_ids: set[str] = set()
    for receipt in value.action_receipts:
        if not isinstance(receipt, ActionReceipt):
            raise ValueError(f"{label} action receipt has an invalid type")
        if not _is_canonical_id(receipt.action_id) or receipt.action_id in action_ids:
            raise ValueError(f"{label} action receipt ID is invalid or duplicated")
        action_ids.add(receipt.action_id)
        if receipt.run_id != value.run_id:
            raise ValueError(f"{label} action receipt is rebound to another run")
        if not _is_canonical_id(receipt.task_id) or receipt.task_id not in task_ids:
            raise ValueError(f"{label} action receipt is rebound to another task")
        observed_tasks.add(receipt.task_id)
        if not isinstance(receipt.evidence_cids, tuple):
            raise ValueError(f"{label} receipt evidence list is mutable")
        for evidence_cid in receipt.evidence_cids:
            _require_sha256(evidence_cid, f"{label} receipt evidence CID")
            if evidence_cid not in input_evidence_cids:
                raise ValueError(f"{label} receipt references foreign evidence")
        if receipt.permission_policy_sha256 != value.identity.permission_policy_sha256:
            raise ValueError(f"{label} action receipt uses another permission policy")
        if not isinstance(receipt.authorized, bool) or not isinstance(
            receipt.influenced_by_experience, bool
        ):
            raise ValueError(f"{label} action receipt flags are not boolean")
        if not receipt.authorized:
            raise ValueError(f"{label} contains an unauthorized action")
        if receipt.influenced_by_experience and not receipt.evidence_cids:
            raise ValueError(f"{label} contains an untraceable experience-influenced action")
    if observed_tasks != task_ids:
        raise ValueError(f"{label} action receipts do not cover the task set")


def _validate_task_receipts(value: ExperienceArm, label: str) -> None:
    if not isinstance(value.task_receipts, tuple):
        raise ValueError(f"{label} task receipts are mutable")
    if any(not isinstance(receipt, TaskReceipt) for receipt in value.task_receipts):
        raise ValueError(f"{label} task receipt has an invalid type")
    if tuple(receipt.task_id for receipt in value.task_receipts) != value.ordered_task_ids:
        raise ValueError(f"{label} task receipts do not match the ordered task set")
    action_ids: set[str] = set()
    for receipt in value.task_receipts:
        if receipt.run_id != value.run_id:
            raise ValueError(f"{label} task receipt is rebound to another run")
        if not isinstance(receipt.action_ids, tuple) or not receipt.action_ids:
            raise ValueError(f"{label} task action inventory is empty or mutable")
        for action_id in receipt.action_ids:
            if not _is_canonical_id(action_id) or action_id in action_ids:
                raise ValueError(f"{label} task action inventory is invalid or duplicated")
            action_ids.add(action_id)
        if not isinstance(receipt.success, bool):
            raise ValueError(f"{label} task receipt success is not boolean")
        if (
            isinstance(receipt.input_tokens, bool)
            or not isinstance(receipt.input_tokens, int)
            or not 0 < receipt.input_tokens <= _MAX_SAFE_INTEGER
        ):
            raise ValueError(f"{label} task receipt token accounting is invalid")
        if not _is_finite_metric(receipt.latency_ms):
            raise ValueError(f"{label} task receipt latency is invalid")


def _validate_action_inventory(value: ExperienceArm, label: str) -> None:
    expected_actions = tuple(
        (task_receipt.task_id, action_id)
        for task_receipt in value.task_receipts
        for action_id in task_receipt.action_ids
    )
    observed_actions = tuple(
        (receipt.task_id, receipt.action_id) for receipt in value.action_receipts
    )
    if observed_actions != expected_actions:
        raise ValueError(f"{label} action receipts do not match the task action inventory")


def _validate_budget(value: ExperienceBudget) -> None:
    if not isinstance(value, ExperienceBudget):
        raise ValueError("experience budget has an invalid type")
    fields = (
        value.minimum_success_delta,
        value.maximum_input_tokens_per_task,
        value.maximum_candidate_token_ratio,
        value.maximum_p95_latency_ms,
    )
    if any(isinstance(item, bool) or not isinstance(item, (int, float)) for item in fields):
        raise ValueError("experience budget contains a non-number")
    if any(abs(item) > _MAX_FINITE_METRIC for item in fields):
        raise ValueError("experience budget exceeds the portable metric range")
    if any(not math.isfinite(float(item)) for item in fields):
        raise ValueError("experience budget contains a non-finite number")
    if not 0 <= value.minimum_success_delta <= 1:
        raise ValueError("minimum success delta must be in [0, 1]")
    if value.maximum_input_tokens_per_task <= 0:
        raise ValueError("maximum input tokens per task must be positive")
    if value.maximum_candidate_token_ratio <= 0:
        raise ValueError("maximum candidate token ratio must be positive")
    if value.maximum_p95_latency_ms < 0:
        raise ValueError("maximum p95 latency must be non-negative")


def _identity_payload(value: ExperimentIdentity) -> dict[str, str]:
    return {
        "agent_sha256": value.agent_sha256,
        "budget_sha256": value.budget_sha256,
        "environment_sha256": value.environment_sha256,
        "input_evidence_sha256": value.input_evidence_sha256,
        "judge_sha256": value.judge_sha256,
        "model_revision_sha256": value.model_revision_sha256,
        "permission_policy_sha256": value.permission_policy_sha256,
        "protocol_sha256": value.protocol_sha256,
        "toolset_sha256": value.toolset_sha256,
    }


def _budget_payload(value: ExperienceBudget) -> dict[str, str | float]:
    return {
        "maximum_candidate_token_ratio": _normalized_float(value.maximum_candidate_token_ratio),
        "maximum_input_tokens_per_task": _normalized_float(value.maximum_input_tokens_per_task),
        "maximum_p95_latency_ms": _normalized_float(value.maximum_p95_latency_ms),
        "minimum_success_delta": _normalized_float(value.minimum_success_delta),
        "schema": "plico.benchmark.verified-experience-budget/v1",
    }


def _ordered_task_set_sha256(task_ids: tuple[str, ...]) -> str:
    return hashlib.sha256(
        json.dumps(task_ids, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def _ordered_evidence_sha256(evidence_cids: tuple[str, ...]) -> str:
    return hashlib.sha256(
        json.dumps(evidence_cids, separators=(",", ":")).encode("utf-8")
    ).hexdigest()


def _nearest_rank_p95(receipts: tuple[TaskReceipt, ...]) -> float:
    ordered = sorted(float(receipt.latency_ms) for receipt in receipts)
    return ordered[math.ceil(0.95 * len(ordered)) - 1]


def _is_finite_metric(value: object) -> bool:
    return (
        not isinstance(value, bool)
        and isinstance(value, (int, float))
        and 0 <= value <= _MAX_FINITE_METRIC
        and math.isfinite(float(value))
    )


def _normalized_float(value: int | float) -> float:
    normalized = float(value)
    return 0.0 if normalized == 0 else normalized


def _is_canonical_id(value: object) -> bool:
    if not isinstance(value, str) or not value or value.strip() != value or not value.isprintable():
        return False
    try:
        value.encode("utf-8")
    except UnicodeEncodeError:
        return False
    return True


def _require_sha256(value: object, label: str) -> None:
    if not isinstance(value, str) or _SHA256.fullmatch(value) is None:
        raise ValueError(f"{label} digest is not canonical SHA-256")
