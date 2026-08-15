"""Fail-closed input validation for a five-run conversational-QA shadow."""

from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path
from typing import Any

from plico_benchmarks.core.client import PROTOCOL


@dataclass(frozen=True)
class QaShadowInput:
    result: dict[str, Any]
    role_configs: tuple[dict[str, Any], ...]


@dataclass(frozen=True)
class QaShadowCampaign:
    run_ids: tuple[str, ...]
    artifacts: dict[str, str]
    sample_ids: tuple[str, ...]
    sample_contract: dict[str, tuple[str, str, str]]
    rows_by_run: tuple[dict[str, dict[str, Any]], ...]
    role_configs: tuple[dict[str, Any], ...]
    llm_identity: dict[str, Any]
    embedding_runtime: dict[str, Any]
    implementation: dict[str, Any]
    suite_config: dict[str, Any]
    costs: tuple[str, ...]
    attempt_counts: tuple[int, ...]
    source_watermark_verified: bool


def load_qa_shadow_input(path: Path) -> QaShadowInput:
    """Deep-verify a committed QA result and load its adjacent durable journal."""
    from plico_benchmarks.core.llm_journal import read_attempt_journal
    from plico_benchmarks.core.result_artifact import verify_result_directory

    result = verify_result_directory(path)
    run_id = _run_id(result)
    snapshot = read_attempt_journal(path.parent / f"llm-journal-{run_id}", run_id)
    if not snapshot.run_complete:
        raise ValueError("QA shadow journal is not committed complete")
    return QaShadowInput(result=result, role_configs=snapshot.role_configs)


def validate_qa_shadow_inputs(inputs: list[QaShadowInput]) -> QaShadowCampaign:
    from plico_benchmarks.core.result_artifact import validate_qa_retrieval_runtime

    if len(inputs) != 5:
        raise ValueError("QA shadow comparison requires exactly five independent runs")
    ordered = sorted(inputs, key=lambda item: _run_id(item.result))
    run_ids = tuple(_run_id(item.result) for item in ordered)
    if len(set(run_ids)) != 5:
        raise ValueError("QA shadow run IDs must be non-empty and unique")

    common: dict[str, Any] = {}
    rows_by_run = []
    costs = []
    attempt_counts = []
    source_verified = True
    for item in ordered:
        result = item.result
        manifest = result.get("run_manifest")
        if not isinstance(manifest, dict):
            raise ValueError("QA shadow input is missing its run manifest")
        if (
            manifest.get("protocol") != PROTOCOL
            or manifest.get("suite") != "conversational-qa"
            or manifest.get("run_class") != "research"
        ):
            raise ValueError("QA shadow input protocol/suite/run class mismatch")
        _validate_sampling(manifest.get("sampling"))
        _same(common, "artifacts", _artifact_contract(manifest), "datasets/selection")

        accounting = result.get("metrics", {}).get("sample_accounting")
        ledger = result.get("metrics", {}).get("capability_ledger")
        selected = _selected_ids(accounting)
        _same(common, "sample_ids", tuple(selected), "ordered sample IDs")
        indexed, sample_contract = _validated_rows(ledger, selected)
        validate_qa_retrieval_runtime(
            result["metrics"],
            ledger,
            result_schema=manifest.get("schemas", {}).get("result", ""),
        )
        _same(common, "sample_contract", sample_contract, "sample classification")
        rows_by_run.append(indexed)

        role_configs = _normalized_role_configs(item.role_configs)
        _same(common, "role_configs", role_configs, "DeepSeek role configuration")
        evidence = result.get("metrics", {}).get("llm_evidence")
        if not isinstance(evidence, dict):
            raise ValueError("QA shadow input has no LLM evidence")
        identity = evidence.get("identity")
        _validate_deepseek_identity(identity)
        _same(common, "llm_identity", identity, "DeepSeek identity/fingerprint")
        attempt_count, cost = _journal_accounting(evidence)
        attempt_counts.append(attempt_count)
        costs.append(cost)

        runtime = result.get("metrics", {}).get("retrieval_runtime")
        if not isinstance(runtime, dict) or runtime.get("provider_identity_scope") not in {
            "projection_publishable_identity",
            "object_execution_only_unattested_provider",
        }:
            raise ValueError("QA shadow embedding provider identity scope is unavailable")
        _same(common, "embedding_runtime", runtime, "embedding provider/runtime")

        git_state = manifest.get("git_state")
        if not isinstance(git_state, dict) or git_state.get("dirty") is not False:
            raise ValueError("QA shadow requires one clean implementation revision")
        implementation = {
            key: git_state.get(key) for key in ("commit", "dirty", "worktree_digest_sha256")
        }
        _same(common, "implementation", implementation, "implementation revision")
        config = result.get("config")
        if not isinstance(config, dict):
            raise ValueError("QA shadow input suite configuration is missing")
        _same(
            common,
            "suite_config",
            {key: value for key, value in config.items() if key != "run_id"},
            "suite configuration",
        )
        source_verified &= manifest.get("pipeline", {}).get("source_watermark") not in {
            None,
            "unavailable_public_v2",
        }

    required = {"locomo", "longmemeval", "conversational_qa_sample_selection"}
    if set(common["artifacts"]) != required:
        raise ValueError("QA shadow input artifact roles are incomplete")
    return QaShadowCampaign(
        run_ids=run_ids,
        artifacts=common["artifacts"],
        sample_ids=common["sample_ids"],
        sample_contract=common["sample_contract"],
        rows_by_run=tuple(rows_by_run),
        role_configs=common["role_configs"],
        llm_identity=common["llm_identity"],
        embedding_runtime=common["embedding_runtime"],
        implementation=common["implementation"],
        suite_config=common["suite_config"],
        costs=tuple(costs),
        attempt_counts=tuple(attempt_counts),
        source_watermark_verified=source_verified,
    )


def _validate_sampling(value: Any) -> None:
    if not isinstance(value, dict) or value.get("failed") != 0 or value.get("excluded") != 0:
        raise ValueError("QA shadow inputs must have zero failed and excluded samples")
    scored = value.get("scored")
    if (
        value.get("actual") != scored
        or isinstance(scored, bool)
        or not isinstance(scored, int)
        or scored <= 0
    ):
        raise ValueError("QA shadow sampling accounting is incomplete")


def _selected_ids(accounting: Any) -> list[str]:
    if not isinstance(accounting, dict):
        raise ValueError("QA shadow input has no sample accounting")
    selected = accounting.get("selected_ids")
    if (
        not isinstance(selected, list)
        or not selected
        or accounting.get("scored_ids") != selected
        or accounting.get("failed_ids") != []
        or accounting.get("excluded_ids") != []
        or any(not isinstance(value, str) or not value for value in selected)
        or len(set(selected)) != len(selected)
    ):
        raise ValueError("QA shadow selected/scored sample identities are invalid")
    return selected


def _validated_rows(
    ledger: Any, selected: list[str]
) -> tuple[dict[str, dict[str, Any]], dict[str, tuple[str, str, str]]]:
    if not isinstance(ledger, list):
        raise ValueError("QA shadow input has no persistent sample ledger")
    indexed = {}
    contract = {}
    for row in ledger:
        if not isinstance(row, dict):
            raise ValueError("QA shadow sample ledger contains a non-object row")
        sample_id = row.get("sample_id")
        if not isinstance(sample_id, str) or not sample_id or sample_id in indexed:
            raise ValueError("QA shadow sample ledger contains a duplicate or invalid ID")
        if (
            row.get("status") != "ok"
            or row.get("embedding_query_state") != "succeeded"
            or row.get("embedding_query_degradation") is not None
            or row.get("retrieval_degraded") is not False
            or row.get("verified_vector_execution") is not True
        ):
            raise ValueError("degraded or unverified QA rows cannot enter shadow inference")
        dataset, stratum, answerability = (
            row.get("dataset"),
            row.get("stratum"),
            row.get("answerability"),
        )
        if (
            dataset not in {"locomo", "longmemeval"}
            or not isinstance(stratum, str)
            or answerability not in {"answerable", "adversarial_unanswerable"}
        ):
            raise ValueError("QA shadow sample classification is invalid")
        indexed[sample_id] = row
        contract[sample_id] = (dataset, stratum, answerability)
    if list(indexed) != selected:
        raise ValueError("QA shadow ledger order does not match selected sample order")
    return indexed, contract


def _artifact_contract(manifest: dict[str, Any]) -> dict[str, str]:
    artifacts = manifest.get("artifacts")
    if not isinstance(artifacts, list):
        raise ValueError("QA shadow input has no artifact manifest")
    contract = {}
    for artifact in artifacts:
        if not isinstance(artifact, dict):
            raise ValueError("QA shadow artifact manifest contains a non-object")
        role = artifact.get("role", artifact.get("logical_name"))
        digest = artifact.get("sha256")
        if (
            not isinstance(role, str)
            or not role
            or role in contract
            or not isinstance(digest, str)
            or len(digest) != 64
        ):
            raise ValueError("QA shadow artifact roles/digests are invalid or duplicated")
        contract[role] = digest
    return contract


def _normalized_role_configs(configs: tuple[dict[str, Any], ...]) -> tuple[dict[str, Any], ...]:
    if not isinstance(configs, tuple):
        raise ValueError("QA shadow durable role configuration is missing")
    normalized, roles = [], set()
    for config in configs:
        if not isinstance(config, dict):
            raise ValueError("QA shadow durable role configuration is invalid")
        role = config.get("role")
        if role not in {"reader", "judge"} or role in roles or config.get("provider") != "deepseek":
            raise ValueError("QA shadow requires exact DeepSeek reader and judge roles")
        roles.add(role)
        normalized.append({key: value for key, value in config.items() if key != "run_id"})
    if roles != {"reader", "judge"}:
        raise ValueError("QA shadow requires exact DeepSeek reader and judge roles")
    return tuple(sorted(normalized, key=lambda config: str(config["role"])))


def _validate_deepseek_identity(identity: Any) -> None:
    if not isinstance(identity, dict) or identity.get("status") != (
        "verified_attempt_integrity_not_cross_run_comparability"
    ):
        raise ValueError("QA shadow DeepSeek identity is not attempt-verified")
    roles = identity.get("roles")
    if not isinstance(roles, dict) or set(roles) != {"reader", "judge"}:
        raise ValueError("QA shadow DeepSeek role identities are incomplete")
    fingerprints = set()
    for role in roles.values():
        if (
            not isinstance(role, dict)
            or role.get("provider") != "deepseek"
            or role.get("model_revision_attestation") != "unattested_alias"
            or role.get("identity_class")
            != "unattested_alias_requires_same_fingerprint_and_five_runs"
        ):
            raise ValueError("QA shadow requires the frozen DeepSeek alias identity contract")
        fingerprint = role.get("system_fingerprint")
        if not isinstance(fingerprint, str) or not fingerprint:
            raise ValueError("QA shadow DeepSeek identity has no system fingerprint")
        fingerprints.add(fingerprint)
    if len(fingerprints) != 1:
        raise ValueError("QA shadow DeepSeek reader/judge fingerprints differ")


def _journal_accounting(evidence: dict[str, Any]) -> tuple[int, str]:
    journal = evidence.get("journal")
    if not isinstance(journal, dict) or journal.get("status") != "verified_complete":
        raise ValueError("QA shadow input journal is not complete")
    count = journal.get("attempt_count")
    if (
        not isinstance(count, int)
        or journal.get("finalized_attempt_count") != count
        or journal.get("incomplete_pending_files") != 0
        or journal.get("incomplete_prepared_attempts") != 0
    ):
        raise ValueError("QA shadow input journal has incomplete attempts")
    cost = str(evidence.get("costs", {}).get("total_usd"))
    try:
        amount = Decimal(cost)
        if not amount.is_finite() or amount < 0:
            raise ValueError
    except Exception as error:
        raise ValueError("QA shadow input cost evidence is invalid") from error
    return count, cost


def _same(common: dict[str, Any], key: str, value: Any, label: str) -> None:
    if key not in common:
        common[key] = value
    elif common[key] != value:
        raise ValueError(f"QA shadow {label} changed across runs")


def _run_id(result: dict[str, Any]) -> str:
    value = result.get("run_manifest", {}).get("run_id")
    if not isinstance(value, str) or not value:
        raise ValueError("QA shadow run IDs must be non-empty and unique")
    return value
