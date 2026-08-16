"""Verified Experience Gain research-shadow contract."""

from __future__ import annotations

import copy
import hashlib
import json
import math
import sys
from dataclasses import replace
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.core.experience_gain import (
    ActionReceipt,
    ExperienceArm,
    ExperienceBudget,
    ExperimentIdentity,
    TaskReceipt,
    evaluate_verified_experience_gain,
    experience_budget_sha256,
)


def _budget(**overrides: object) -> ExperienceBudget:
    values: dict[str, object] = {
        "minimum_success_delta": 0.10,
        "maximum_input_tokens_per_task": 1_000.0,
        "maximum_candidate_token_ratio": 0.5,
        "maximum_p95_latency_ms": 200.0,
    }
    values.update(overrides)
    return ExperienceBudget(**values)  # type: ignore[arg-type]


def _identity(budget: ExperienceBudget, evidence_cids: tuple[str, ...]) -> ExperimentIdentity:
    return ExperimentIdentity(
        protocol_sha256="1" * 64,
        agent_sha256="2" * 64,
        model_revision_sha256="3" * 64,
        toolset_sha256="4" * 64,
        environment_sha256="5" * 64,
        input_evidence_sha256=hashlib.sha256(
            json.dumps(evidence_cids, separators=(",", ":")).encode()
        ).hexdigest(),
        judge_sha256="7" * 64,
        permission_policy_sha256="8" * 64,
        budget_sha256=experience_budget_sha256(budget),
    )


def _arm(
    run_id: str,
    *,
    budget: ExperienceBudget | None = None,
    **overrides: object,
) -> ExperienceArm:
    selected_budget = budget or _budget()
    input_evidence_cids = ("9" * 64,)
    identity = _identity(selected_budget, input_evidence_cids)
    task_ids = tuple(f"task-{index:03d}" for index in range(100))
    successes = tuple(index < (70 if run_id == "candidate" else 50) for index in range(100))
    task_action_ids = tuple(
        tuple(f"{run_id}-{task_id}-action-{action_index}" for action_index in range(2))
        for task_id in task_ids
    )
    task_receipts = tuple(
        TaskReceipt(
            task_id=task_id,
            run_id=run_id,
            action_ids=task_action_ids[index],
            success=successes[index],
            input_tokens=400 if run_id == "candidate" else 1_000,
            latency_ms=180.0,
        )
        for index, task_id in enumerate(task_ids)
    )
    receipts = tuple(
        receipt
        for task_index, task_id in enumerate(task_ids)
        for receipt in (
            ActionReceipt(
                action_id=action_id,
                run_id=run_id,
                task_id=task_id,
                evidence_cids=(("9" * 64,) if run_id == "candidate" else ()),
                permission_policy_sha256=identity.permission_policy_sha256,
                authorized=True,
                influenced_by_experience=run_id == "candidate",
            )
            for action_id in task_action_ids[task_index]
        )
    )
    values: dict[str, object] = {
        "run_id": run_id,
        "condition_sha256": ("a" if run_id == "candidate" else "b") * 64,
        "identity": identity,
        "ordered_task_ids": task_ids,
        "input_evidence_cids": input_evidence_cids,
        "task_receipts": task_receipts,
        "action_receipts": receipts,
    }
    values.update(overrides)
    return ExperienceArm(**values)  # type: ignore[arg-type]


def _large_token_arm(run_id: str, budget: ExperienceBudget) -> ExperienceArm:
    input_evidence_cids = ("9" * 64,)
    identity = _identity(budget, input_evidence_cids)
    task_ids = tuple(f"large-task-{index:05d}" for index in range(10_000))
    task_receipts = tuple(
        TaskReceipt(
            task_id=task_id,
            run_id=run_id,
            action_ids=(f"{run_id}-{task_id}-action",),
            success=index < (7_000 if run_id == "candidate" else 5_000),
            input_tokens=10**12,
            latency_ms=1.0,
        )
        for index, task_id in enumerate(task_ids)
    )
    return ExperienceArm(
        run_id=run_id,
        condition_sha256=("a" if run_id == "candidate" else "b") * 64,
        identity=identity,
        ordered_task_ids=task_ids,
        input_evidence_cids=input_evidence_cids,
        task_receipts=task_receipts,
        action_receipts=tuple(
            ActionReceipt(
                action_id=receipt.action_ids[0],
                run_id=run_id,
                task_id=receipt.task_id,
                evidence_cids=input_evidence_cids if run_id == "candidate" else (),
                permission_policy_sha256=identity.permission_policy_sha256,
                authorized=True,
                influenced_by_experience=run_id == "candidate",
            )
            for receipt in task_receipts
        ),
    )


def test_verified_experience_gain_is_deterministic_shadow_only() -> None:
    budget = _budget()
    first = evaluate_verified_experience_gain(
        candidate=_arm("candidate", budget=budget),
        control=_arm("control", budget=budget),
        budget=budget,
    )
    second = evaluate_verified_experience_gain(
        candidate=_arm("candidate", budget=budget),
        control=_arm("control", budget=budget),
        budget=budget,
    )

    assert first == second
    assert first["task_success_delta"] == pytest.approx(0.2)
    assert first["verified_experience_gain"] == pytest.approx(0.2)
    assert first["candidate_token_ratio"] == pytest.approx(0.4)
    assert first["candidate_action_receipts"] == 200
    assert first["candidate_experience_influenced_actions"] == 200
    assert first["status"] == "shadow_effect_threshold_met"
    assert first["gate_eligible"] is False
    assert all(first["checks"].values())


@pytest.mark.parametrize(
    ("candidate_overrides", "failed_check"),
    [
        (
            {
                "task_receipts": tuple(
                    replace(receipt, success=index < 55)
                    for index, receipt in enumerate(_arm("candidate").task_receipts)
                )
            },
            "success_delta",
        ),
        (
            {
                "task_receipts": tuple(
                    replace(receipt, input_tokens=600)
                    for receipt in _arm("candidate").task_receipts
                )
            },
            "token_ratio",
        ),
        (
            {
                "task_receipts": tuple(
                    replace(receipt, latency_ms=201.0)
                    for receipt in _arm("candidate").task_receipts
                )
            },
            "candidate_p95_latency",
        ),
    ],
)
def test_shadow_reports_hard_thresholds_without_claiming_gate(
    candidate_overrides: dict[str, object], failed_check: str
) -> None:
    result = evaluate_verified_experience_gain(
        candidate=_arm("candidate", **candidate_overrides),
        control=_arm("control"),
        budget=_budget(),
    )

    expected_status = (
        "shadow_effect_threshold_not_met"
        if failed_check == "success_delta"
        else "shadow_ineligible_budget"
    )
    assert result["status"] == expected_status
    assert result["checks"][failed_check] is False
    if expected_status == "shadow_ineligible_budget":
        assert result["verified_experience_gain"] is None
    assert result["gate_eligible"] is False


def test_untraceable_influenced_action_is_structurally_invalid() -> None:
    candidate = _arm("candidate")
    receipts = list(candidate.action_receipts)
    receipts[0] = replace(receipts[0], evidence_cids=())
    with pytest.raises(ValueError, match="untraceable"):
        evaluate_verified_experience_gain(
            candidate=replace(candidate, action_receipts=tuple(receipts)),
            control=_arm("control"),
            budget=_budget(),
        )


def test_control_untraceable_influenced_action_is_structurally_invalid() -> None:
    control = _arm("control")
    receipts = list(control.action_receipts)
    receipts[0] = replace(receipts[0], influenced_by_experience=True)

    with pytest.raises(ValueError, match="untraceable"):
        evaluate_verified_experience_gain(
            candidate=_arm("candidate"),
            control=replace(control, action_receipts=tuple(receipts)),
            budget=_budget(),
        )


@pytest.mark.parametrize(
    ("field", "digest"),
    [
        ("protocol_sha256", "c" * 64),
        ("agent_sha256", "c" * 64),
        ("model_revision_sha256", "c" * 64),
        ("toolset_sha256", "c" * 64),
        ("environment_sha256", "c" * 64),
        ("input_evidence_sha256", "c" * 64),
        ("judge_sha256", "c" * 64),
        ("permission_policy_sha256", "c" * 64),
    ],
)
def test_shadow_rejects_cross_protocol_identity(field: str, digest: str) -> None:
    candidate = _arm("candidate")
    candidate = replace(candidate, identity=replace(candidate.identity, **{field: digest}))

    with pytest.raises(ValueError):
        evaluate_verified_experience_gain(
            candidate=candidate, control=_arm("control"), budget=_budget()
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "same_run",
        "same_condition",
        "task_rebound",
        "duplicate_task",
        "task_receipt_count",
        "non_boolean_result",
        "invalid_condition_digest",
        "zero_tokens",
        "nan_latency",
        "huge_tokens",
        "huge_latency",
        "invalid_utf8_task",
        "budget_rebound",
    ],
)
def test_shadow_rejects_unpaired_or_invalid_arm(mutation: str) -> None:
    candidate = _arm("candidate")
    control = _arm("control")
    if mutation == "same_run":
        candidate = replace(candidate, run_id=control.run_id)
    elif mutation == "same_condition":
        candidate = replace(candidate, condition_sha256=control.condition_sha256)
    elif mutation == "task_rebound":
        candidate = replace(
            candidate,
            ordered_task_ids=tuple(reversed(candidate.ordered_task_ids)),
        )
    elif mutation == "duplicate_task":
        candidate = replace(
            candidate,
            ordered_task_ids=(candidate.ordered_task_ids[0],) + candidate.ordered_task_ids[:-1],
        )
    elif mutation == "task_receipt_count":
        candidate = replace(candidate, task_receipts=candidate.task_receipts[:-1])
    elif mutation == "non_boolean_result":
        receipts = list(candidate.task_receipts)
        receipts[0] = replace(receipts[0], success=1)  # type: ignore[arg-type]
        candidate = replace(candidate, task_receipts=tuple(receipts))
    elif mutation == "invalid_condition_digest":
        candidate = replace(candidate, condition_sha256="A" * 64)
    elif mutation == "zero_tokens":
        receipts = list(control.task_receipts)
        receipts[0] = replace(receipts[0], input_tokens=0)
        control = replace(control, task_receipts=tuple(receipts))
    elif mutation == "nan_latency":
        receipts = list(candidate.task_receipts)
        receipts[0] = replace(receipts[0], latency_ms=math.nan)
        candidate = replace(candidate, task_receipts=tuple(receipts))
    elif mutation == "huge_tokens":
        receipts = list(candidate.task_receipts)
        receipts[0] = replace(receipts[0], input_tokens=10**400)
        candidate = replace(candidate, task_receipts=tuple(receipts))
    elif mutation == "huge_latency":
        receipts = list(candidate.task_receipts)
        receipts[0] = replace(receipts[0], latency_ms=10**400)
        candidate = replace(candidate, task_receipts=tuple(receipts))
    elif mutation == "invalid_utf8_task":
        task_ids = ("\ud800",) + candidate.ordered_task_ids[1:]
        candidate = replace(candidate, ordered_task_ids=task_ids)
    else:
        candidate = replace(
            candidate,
            identity=replace(candidate.identity, budget_sha256="c" * 64),
        )

    with pytest.raises(ValueError):
        evaluate_verified_experience_gain(
            candidate=candidate,
            control=control,
            budget=_budget(),
        )


@pytest.mark.parametrize(
    "mutation",
    [
        "duplicate_action",
        "missing_action",
        "reordered_action",
        "cross_task_swap",
        "wrong_run",
        "wrong_task",
        "wrong_policy",
        "bad_evidence",
        "foreign_evidence",
        "mutable_evidence",
        "unauthorized",
        "non_boolean_flag",
        "missing_task",
        "no_candidate_influence",
    ],
)
def test_shadow_rejects_invalid_or_rebound_action_receipts(mutation: str) -> None:
    candidate = _arm("candidate")
    receipts = list(candidate.action_receipts)
    if mutation == "duplicate_action":
        receipts[1] = replace(receipts[1], action_id=receipts[0].action_id)
    elif mutation == "missing_action":
        receipts = receipts[:-1]
    elif mutation == "reordered_action":
        receipts[0], receipts[1] = receipts[1], receipts[0]
    elif mutation == "cross_task_swap":
        first_task_id = receipts[0].task_id
        second_task_id = receipts[2].task_id
        receipts[0] = replace(receipts[0], task_id=second_task_id)
        receipts[2] = replace(receipts[2], task_id=first_task_id)
    elif mutation == "wrong_run":
        receipts[0] = replace(receipts[0], run_id="control")
    elif mutation == "wrong_task":
        receipts[0] = replace(receipts[0], task_id="another-task")
    elif mutation == "wrong_policy":
        receipts[0] = replace(receipts[0], permission_policy_sha256="c" * 64)
    elif mutation == "bad_evidence":
        receipts[0] = replace(receipts[0], evidence_cids=("A" * 64,))
    elif mutation == "foreign_evidence":
        receipts[0] = replace(receipts[0], evidence_cids=("d" * 64,))
    elif mutation == "mutable_evidence":
        receipts[0] = replace(receipts[0], evidence_cids=["9" * 64])  # type: ignore[arg-type]
    elif mutation == "unauthorized":
        receipts[0] = replace(receipts[0], authorized=False)
    elif mutation == "non_boolean_flag":
        receipts[0] = replace(receipts[0], influenced_by_experience=1)  # type: ignore[arg-type]
    elif mutation == "missing_task":
        receipts = [receipt for receipt in receipts if receipt.task_id != "task-099"]
    else:
        receipts = [replace(receipt, influenced_by_experience=False) for receipt in receipts]

    with pytest.raises(ValueError):
        evaluate_verified_experience_gain(
            candidate=replace(candidate, action_receipts=tuple(receipts)),
            control=_arm("control"),
            budget=_budget(),
        )


@pytest.mark.parametrize(
    "budget",
    [
        _budget(minimum_success_delta=math.nan),
        _budget(minimum_success_delta=-0.1),
        _budget(maximum_input_tokens_per_task=0),
        _budget(maximum_candidate_token_ratio=0),
        _budget(maximum_p95_latency_ms=-1),
        _budget(maximum_p95_latency_ms=10**400),
    ],
)
def test_shadow_rejects_invalid_preregistered_budget(
    budget: ExperienceBudget,
) -> None:
    with pytest.raises(ValueError):
        experience_budget_sha256(budget)


def test_permission_violation_is_always_invalid() -> None:
    candidate = _arm("candidate")
    receipts = list(candidate.action_receipts)
    receipts[0] = replace(receipts[0], authorized=False)

    with pytest.raises(ValueError, match="unauthorized"):
        evaluate_verified_experience_gain(
            candidate=replace(candidate, action_receipts=tuple(receipts)),
            control=_arm("control"),
            budget=_budget(),
        )


@pytest.mark.parametrize(
    ("arm_name", "field", "failed_check"),
    [
        ("candidate", "tokens", "candidate_input_tokens"),
        ("control", "tokens", "control_input_tokens"),
        ("candidate", "latency", "candidate_p95_latency"),
        ("control", "latency", "control_p95_latency"),
    ],
)
def test_both_arms_must_fit_the_absolute_budget(
    arm_name: str, field: str, failed_check: str
) -> None:
    candidate = _arm("candidate")
    control = _arm("control")
    arm = candidate if arm_name == "candidate" else control
    receipts = list(arm.task_receipts)
    if field == "tokens":
        receipts[0] = replace(receipts[0], input_tokens=10_000)
    else:
        receipts = [replace(receipt, latency_ms=201.0) for receipt in receipts]
    if arm_name == "candidate":
        candidate = replace(candidate, task_receipts=tuple(receipts))
    else:
        control = replace(control, task_receipts=tuple(receipts))

    result = evaluate_verified_experience_gain(
        candidate=candidate, control=control, budget=_budget()
    )
    assert result["checks"][failed_check] is False
    assert result["status"] == "shadow_ineligible_budget"
    assert result["verified_experience_gain"] is None
    assert result["gate_eligible"] is False


def test_semantically_equal_signed_zero_budgets_have_one_digest() -> None:
    assert experience_budget_sha256(_budget(minimum_success_delta=0.0)) == (
        experience_budget_sha256(_budget(minimum_success_delta=-0.0))
    )


def test_total_tokens_must_fit_the_portable_integer_range() -> None:
    budget = _budget(
        maximum_input_tokens_per_task=10**12,
        maximum_candidate_token_ratio=1.0,
        maximum_p95_latency_ms=1.0,
    )

    with pytest.raises(ValueError, match="portable integer"):
        evaluate_verified_experience_gain(
            candidate=_large_token_arm("candidate", budget),
            control=_large_token_arm("control", budget),
            budget=budget,
        )


def test_inputs_are_immutable_values() -> None:
    candidate = _arm("candidate")
    before = copy.deepcopy(candidate)
    evaluate_verified_experience_gain(
        candidate=candidate, control=_arm("control"), budget=_budget()
    )
    assert candidate == before
