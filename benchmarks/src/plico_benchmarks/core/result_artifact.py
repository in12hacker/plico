"""No-clobber committed directories for one benchmark suite result."""

from __future__ import annotations

import math
from decimal import Decimal
from pathlib import Path
from typing import Any

from plico_benchmarks.core.dogfood_io import (
    canonical_json,
    commit_artifact_directory,
    sha256,
    strict_json_object,
    verify_artifact_directory,
)
from plico_benchmarks.core.integrity import validate_run_manifest
from plico_benchmarks.core.llm_evidence import (
    summarize_llm_costs,
    summarize_llm_identity,
)
from plico_benchmarks.core.llm_journal import read_attempt_journal
from plico_benchmarks.core.metrics import accuracy_pct, compute_statistics
from plico_benchmarks.core.qa_retrieval_policy import (
    QA_RETRIEVAL_POLICY_ROLE,
    qa_retrieval_policy_artifact,
    validate_exact_qa_execution,
    validate_qa_retrieval_policy,
)
from plico_benchmarks.core.retrieval_execution import (
    PROVIDER_IDENTITY_SCOPES,
    provider_identity_scope,
    validate_embedding_query,
    validate_retrieval_execution,
    verified_vector_execution,
)

RESULT_FILE = "result.json"
RUN_MANIFEST_FILE = "run_manifest.json"
RESULT_COMMIT_SCHEMA = "plico.benchmark.result-commit/v1"


def commit_result_directory(output: Path, result: dict[str, Any]) -> dict[str, Any]:
    """Commit one canonical result/manifest pair into a new owner-only directory."""
    manifest = result.get("run_manifest")
    if not isinstance(manifest, dict):
        raise ValueError("benchmark result has no run manifest")
    validate_run_manifest(manifest)
    if result.get("metadata", {}).get("run_id") != manifest.get("run_id"):
        raise ValueError("benchmark metadata and manifest run IDs differ")
    if result.get("metadata", {}).get("suite") != manifest.get("suite"):
        raise ValueError("benchmark metadata and manifest suites differ")
    result_payload = canonical_json(result)
    sidecar = {
        **manifest,
        "result_artifact": {
            "file_name": RESULT_FILE,
            "bytes": len(result_payload),
            "sha256": sha256(result_payload),
        },
    }
    commit_artifact_directory(
        output,
        artifact_name=RESULT_FILE,
        sidecar_name=RUN_MANIFEST_FILE,
        artifact=result_payload,
        sidecar=canonical_json(sidecar),
        commit_schema=RESULT_COMMIT_SCHEMA,
    )
    return verify_result_directory(output)


def verify_result_directory(output: Path) -> dict[str, Any]:
    """Deep-verify directory topology, canonical bytes, and embedded/detached manifest."""
    result_payload, sidecar_payload = verify_artifact_directory(
        output,
        artifact_name=RESULT_FILE,
        sidecar_name=RUN_MANIFEST_FILE,
        commit_schema=RESULT_COMMIT_SCHEMA,
    )
    result = strict_json_object(result_payload)
    if result_payload != canonical_json(result):
        raise ValueError("benchmark result is not canonical supported JSON")
    sidecar = strict_json_object(sidecar_payload)
    if sidecar_payload != canonical_json(sidecar):
        raise ValueError("benchmark run manifest is not canonical supported JSON")
    embedded = result.get("run_manifest")
    detached = {key: value for key, value in sidecar.items() if key != "result_artifact"}
    if not isinstance(embedded, dict) or embedded != detached:
        raise ValueError("benchmark embedded and detached run manifests differ")
    validate_run_manifest(embedded)
    if result.get("metadata", {}).get("suite") != embedded.get("suite"):
        raise ValueError("benchmark metadata and manifest suites differ")
    expected = {
        "file_name": RESULT_FILE,
        "bytes": len(result_payload),
        "sha256": sha256(result_payload),
    }
    if sidecar.get("result_artifact") != expected:
        raise ValueError("benchmark detached manifest does not bind result bytes")
    if result.get("metadata", {}).get("run_id") != embedded.get("run_id"):
        raise ValueError("benchmark result run identities differ")
    result_schema = embedded.get("schemas", {}).get("result")
    expected_version = {
        "plico.benchmark-result/v4": 4,
        "plico.benchmark-result/v5": 5,
        "plico.benchmark-result/v6": 6,
    }.get(result_schema)
    metadata_version = result.get("metadata", {}).get("result_schema_version")
    if (embedded.get("suite") == "conversational-qa" and metadata_version != expected_version) or (
        embedded.get("suite") != "conversational-qa"
        and metadata_version is not None
        and metadata_version != expected_version
    ):
        raise ValueError("benchmark result metadata and manifest schemas differ")
    _validate_suite_evidence(result, output)
    return result


def read_verified_result(output: Path) -> tuple[bytes, bytes, dict[str, Any]]:
    """Return committed bytes after the same deep verification used by report generation."""
    result = verify_result_directory(output)
    result_payload, sidecar_payload = verify_artifact_directory(
        output,
        artifact_name=RESULT_FILE,
        sidecar_name=RUN_MANIFEST_FILE,
        commit_schema=RESULT_COMMIT_SCHEMA,
    )
    return result_payload, sidecar_payload, result


def _validate_suite_evidence(result: dict[str, Any], output: Path) -> None:
    if result.get("metadata", {}).get("suite") != "conversational-qa":
        return
    if result.get("run_manifest", {}).get("schemas", {}).get("result") == (
        "plico.benchmark-result/v6"
    ) and result.get("config", {}).get("environment", {}).get("PLICO_KG_AUTO_EXTRACT") != ("false"):
        raise ValueError("QA v6 result does not bind PLICO_KG_AUTO_EXTRACT=false")
    metrics = result.get("metrics")
    if not isinstance(metrics, dict):
        raise ValueError("QA result has no typed metrics")
    evidence = metrics.get("llm_evidence")
    if not isinstance(evidence, dict) or set(evidence) != {
        "schema",
        "journal",
        "identity",
        "costs",
    }:
        raise ValueError("QA result has no exact LLM evidence envelope")
    if evidence["schema"] != "plico.benchmark.deepseek-attempt-ledger/v1":
        raise ValueError("QA LLM evidence schema is unsupported")
    journal = evidence["journal"]
    if not isinstance(journal, dict) or journal.get("status") != "verified_complete":
        raise ValueError("committed QA result requires a complete paid-attempt journal")
    run_id = result["run_manifest"]["run_id"]
    if journal.get("run_id") != run_id:
        raise ValueError("QA result and journal run identities differ")
    snapshot = read_attempt_journal(output.parent / f"llm-journal-{run_id}", run_id)
    expected_journal = {
        "status": "verified_complete",
        "run_id": snapshot.run_id,
        "inventory_sha256": snapshot.inventory_sha256,
        "attempt_count": snapshot.attempt_count,
        "finalized_attempt_count": snapshot.finalized_attempt_count,
        "incomplete_prepared_attempts": snapshot.incomplete_prepared_attempts,
        "incomplete_pending_files": snapshot.incomplete_pending_files,
        "total_usd_accounted": snapshot.total_usd_accounted,
    }
    if not snapshot.run_complete or journal != expected_journal:
        raise ValueError("QA journal summary does not match durable journal bytes")
    finalized_records = []
    for entry in snapshot.entries:
        if entry.phase != "finalized" or not isinstance(entry.finalized, dict):
            raise ValueError("QA committed journal contains an incomplete attempt")
        finalized_records.append(entry.finalized)
    identity = summarize_llm_identity(finalized_records)
    costs = summarize_llm_costs(finalized_records)
    if evidence["identity"] != identity or evidence["costs"] != costs:
        raise ValueError("QA identity or cost summary does not replay from the journal")
    if result.get("costs") != costs:
        raise ValueError("QA top-level costs differ from the durable journal")
    expected_pipeline = {
        "status": "verified_attempt_integrity_not_cross_run_comparability",
        "journal": journal,
        "identity": identity,
        "costs": costs,
    }
    if result["run_manifest"].get("pipeline", {}).get("llm_identity") != expected_pipeline:
        raise ValueError("QA manifest pipeline does not bind the durable LLM identity")
    external = result["run_manifest"].get("external_evidence", [])
    expected_external = {
        "role": "deepseek_paid_attempt_journal",
        "run_id": run_id,
        "inventory_sha256": snapshot.inventory_sha256,
        "attempt_count": snapshot.attempt_count,
        "finalized_attempt_count": snapshot.finalized_attempt_count,
        "total_usd_accounted": snapshot.total_usd_accounted,
    }
    if expected_external not in external:
        raise ValueError("QA manifest does not bind the external journal inventory")
    _validate_qa_sample_ledger(metrics, finalized_records, result["run_manifest"])


def _validate_qa_sample_ledger(
    metrics: dict[str, Any], attempts: list[dict[str, Any]], manifest: dict[str, Any]
) -> None:
    ledger = metrics.get("capability_ledger")
    accounting = metrics.get("sample_accounting")
    if not isinstance(ledger, list) or not isinstance(accounting, dict):
        raise ValueError("QA sample evidence ledger is missing")
    selected = accounting.get("selected_ids")
    scored = accounting.get("scored_ids")
    failed = accounting.get("failed_ids")
    excluded = accounting.get("excluded_ids")
    ledger_ids = [item.get("sample_id") for item in ledger if isinstance(item, dict)]
    if (
        not isinstance(selected, list)
        or scored != selected
        or failed != []
        or excluded != []
        or ledger_ids != scored
        or len(set(scored)) != len(scored)
        or manifest["sampling"]["scored"] != len(scored)
    ):
        raise ValueError("QA sample accounting is not an exact partition")
    attempt_by_sequence = {attempt.get("attempt_sequence"): attempt for attempt in attempts}
    if len(attempt_by_sequence) != len(attempts) or set(attempt_by_sequence) != set(
        range(1, len(attempts) + 1)
    ):
        raise ValueError("QA durable attempt sequences are not exact and contiguous")
    observed_sequences = []
    observed_request_ids = set()
    for item in ledger:
        answerability = item.get("answerability")
        if answerability not in {"answerable", "adversarial_unanswerable"}:
            raise ValueError("QA answerability evidence is invalid")
        is_unanswerable = answerability == "adversarial_unanswerable"
        overlap = item.get("token_overlap")
        if not isinstance(overlap, dict):
            raise ValueError("QA token overlap evidence is missing")
        predicted = overlap.get("predicted_token_count")
        expected = overlap.get("expected_token_count")
        common = overlap.get("common_token_count")
        if any(
            isinstance(value, bool) or not isinstance(value, int) or value < 0
            for value in (predicted, expected, common)
        ) or common > min(predicted, expected):
            raise ValueError("QA token overlap evidence is invalid")
        if is_unanswerable:
            if (
                expected != 0
                or item.get("f1") is not None
                or item.get("bleu1") is not None
                or item.get("llm_score") is not None
                or not isinstance(item.get("abstention_correct"), bool)
            ):
                raise ValueError("QA adversarial abstention evidence is invalid")
        else:
            if item.get("abstention_correct") is not None:
                raise ValueError("QA answerable sample cannot claim abstention accuracy")
            if not math.isclose(item.get("f1"), _f1_from_counts(predicted, expected, common)):
                raise ValueError("QA F1 does not recompute from retained evidence")
            if not math.isclose(item.get("bleu1"), _bleu1_from_counts(predicted, expected, common)):
                raise ValueError("QA BLEU-1 does not recompute from retained evidence")
        recall_counts = item.get("evidence_recall_counts")
        if not isinstance(recall_counts, dict):
            raise ValueError("QA evidence recall counts are missing")
        expected_evidence = recall_counts.get("expected_count")
        retrieved_evidence = recall_counts.get("retrieved_expected_count")
        if (
            any(
                isinstance(value, bool) or not isinstance(value, int) or value < 0
                for value in (expected_evidence, retrieved_evidence)
            )
            or retrieved_evidence > expected_evidence
        ):
            raise ValueError("QA evidence recall counts are invalid")
        recomputed_recall = (
            None if expected_evidence == 0 else retrieved_evidence / expected_evidence
        )
        if item.get("evidence_recall@10") != recomputed_recall:
            raise ValueError("QA evidence recall does not recompute")
        score = item.get("llm_score")
        if not is_unanswerable and (
            isinstance(score, bool) or not isinstance(score, int) or score not in range(1, 6)
        ):
            raise ValueError("QA judge score is invalid")
        embedding_state, embedding_degradation = validate_embedding_query(
            {
                "state": item.get("embedding_query_state"),
                **(
                    {"degradation": item.get("embedding_query_degradation")}
                    if item.get("embedding_query_degradation") is not None
                    else {}
                ),
            }
        )
        retrieval_execution = validate_retrieval_execution(item.get("retrieval_execution"))
        expected_vector_verified = verified_vector_execution(embedding_state, retrieval_execution)
        expected_degraded = embedding_degradation is not None or any(
            execution["degradation"] is not None for execution in retrieval_execution
        )
        if (
            item.get("verified_vector_execution") is not expected_vector_verified
            or item.get("retrieval_degraded") is not expected_degraded
        ):
            raise ValueError("QA retrieval execution summary does not recompute")
        requests = item.get("llm_request_evidence")
        if not isinstance(requests, list) or not requests:
            raise ValueError("QA sample has no paid request references")
        for request in requests:
            if request.get("evidence_status") != "verified":
                raise ValueError("QA committed sample contains unverified LLM evidence")
            request_ids = request.get("request_ids")
            summaries = request.get("attempts")
            sequences = request.get("attempt_sequences")
            if (
                not isinstance(request_ids, list)
                or not request_ids
                or len(set(request_ids)) != len(request_ids)
                or not isinstance(summaries, list)
                or not summaries
                or sequences != [summary.get("attempt_sequence") for summary in summaries]
            ):
                raise ValueError("QA request attempt summary is invalid")
            if observed_request_ids.intersection(request_ids):
                raise ValueError("QA request identity is rebound across samples")
            boundary = request.get("boundary")
            if boundary not in {"reader", "scored_judge", "ragas_style_proxy"} or (
                is_unanswerable and boundary != "reader"
            ):
                raise ValueError("QA request boundary disagrees with answerability")
            expected_role = "reader" if boundary == "reader" else "judge"
            observed_request_ids.update(request_ids)
            request_usd = Decimal(0)
            terminal_by_request: dict[str, dict[str, Any]] = {}
            for summary in summaries:
                sequence = summary.get("attempt_sequence")
                durable = attempt_by_sequence.get(sequence)
                expected_summary = (
                    {
                        "attempt_sequence": durable.get("attempt_sequence"),
                        "role": durable.get("role"),
                        "role_request_id": durable.get("role_request_id"),
                        "status": durable.get("status"),
                        "usd_accounted": durable.get("usd_accounted"),
                    }
                    if durable is not None
                    else None
                )
                if (
                    summary != expected_summary
                    or durable.get("sample_id") != item.get("sample_id")
                    or durable.get("role_request_id") not in request_ids
                    or durable.get("role") != expected_role
                ):
                    raise ValueError("QA sample request does not match durable attempt evidence")
                request_usd += Decimal(durable["usd_accounted"])
                terminal_by_request[durable["role_request_id"]] = durable
                observed_sequences.append(sequence)
            if set(terminal_by_request) != set(request_ids) or any(
                terminal.get("status") != "ok" for terminal in terminal_by_request.values()
            ):
                raise ValueError("QA request does not have an exact successful terminal attempt")
            if Decimal(request.get("usd_accounted")) != request_usd:
                raise ValueError("QA request cost does not match durable attempts")
        boundaries = {request.get("boundary") for request in requests}
        expected_boundaries = {"reader"} if is_unanswerable else {"reader", "scored_judge"}
        if not expected_boundaries.issubset(boundaries):
            raise ValueError("QA sample is missing its required request boundary")
    if sorted(observed_sequences) != list(range(1, len(attempts) + 1)):
        raise ValueError("QA request references do not cover the journal exactly once")
    validate_qa_retrieval_runtime(
        metrics,
        ledger,
        result_schema=manifest.get("schemas", {}).get("result", ""),
    )
    if manifest.get("schemas", {}).get("result") == "plico.benchmark-result/v6":
        policy = metrics["retrieval_runtime"]["retrieval_policy"]
        expected_policy_artifact = qa_retrieval_policy_artifact(policy)
        matching = [
            artifact
            for artifact in manifest.get("artifacts", [])
            if isinstance(artifact, dict) and artifact.get("role") == QA_RETRIEVAL_POLICY_ROLE
        ]
        if matching != [expected_policy_artifact]:
            raise ValueError("QA manifest does not bind the frozen retrieval policy artifact")
    _validate_qa_aggregates(metrics, ledger, manifest)


def validate_qa_retrieval_runtime(
    metrics: dict[str, Any],
    ledger: list[dict[str, Any]],
    *,
    result_schema: str = "plico.benchmark-result/v5",
) -> None:
    runtime = metrics.get("retrieval_runtime")
    common_fields = {
        "requirement",
        "configured_embedding_backend",
        "active_embedding_provider",
        "embedding_provider_state",
        "provider_identity_scope",
        "ingest_watermark",
    }
    if not isinstance(runtime, dict):
        raise ValueError("QA retrieval runtime evidence is missing")
    if result_schema == "plico.benchmark-result/v4":
        if set(runtime) != common_fields:
            raise ValueError("QA retrieval runtime evidence is missing")
        watermark_fields = {"accepted", "completed", "in_flight"}
        ingest_outcomes = None
    elif result_schema in {"plico.benchmark-result/v5", "plico.benchmark-result/v6"}:
        v6_fields = {"retrieval_policy"} if result_schema == "plico.benchmark-result/v6" else set()
        if set(runtime) != common_fields | {"ingest_outcomes", "cognitive_pipeline"} | v6_fields:
            raise ValueError("QA retrieval runtime evidence is missing")
        watermark_fields = {"accepted", "accepted_delta", "completed", "in_flight"}
        ingest_outcomes = runtime["ingest_outcomes"]
    else:
        raise ValueError("QA result schema is unsupported")
    ingest_watermark = runtime["ingest_watermark"]
    if (
        not isinstance(ingest_watermark, dict)
        or set(ingest_watermark) != watermark_fields
        or any(
            isinstance(ingest_watermark[field], bool)
            or not isinstance(ingest_watermark[field], int)
            or ingest_watermark[field] < 0
            for field in ingest_watermark
        )
        or ingest_watermark["completed"] < ingest_watermark["accepted"]
        or (
            result_schema in {"plico.benchmark-result/v5", "plico.benchmark-result/v6"}
            and ingest_watermark["accepted_delta"] > ingest_watermark["accepted"]
        )
        or ingest_watermark["in_flight"] != 0
    ):
        raise ValueError("QA ingest watermark evidence is incomplete")
    if ingest_outcomes is not None:
        pipeline = runtime["cognitive_pipeline"]
        if (
            not isinstance(pipeline, dict)
            or set(pipeline) != {"max_in_flight", "queue_capacity"}
            or any(
                isinstance(pipeline[field], bool) or not isinstance(pipeline[field], int)
                for field in pipeline
            )
            or not 1 <= pipeline["max_in_flight"] <= 64
            or not pipeline["max_in_flight"] <= pipeline["queue_capacity"] <= 65_536
        ):
            raise ValueError("QA cognitive pipeline runtime configuration is invalid")
        outcome_fields = {
            "submitted",
            "unique_cids",
            "duplicate_cids",
            "queued_accepted_attempts",
            "inline_document_attempts",
            "document_vector_succeeded_attempts",
            "document_lexical_degraded_attempts",
            "task_failed_attempts",
            "other_succeeded_attempts",
        }
        if (
            not isinstance(ingest_outcomes, dict)
            or set(ingest_outcomes) != outcome_fields
            or any(
                isinstance(ingest_outcomes[field], bool)
                or not isinstance(ingest_outcomes[field], int)
                or ingest_outcomes[field] < 0
                for field in outcome_fields
            )
            or ingest_outcomes["submitted"] == 0
            or ingest_outcomes["unique_cids"] == 0
            or ingest_outcomes["submitted"]
            != ingest_outcomes["unique_cids"] + ingest_outcomes["duplicate_cids"]
            or ingest_outcomes["queued_accepted_attempts"] != ingest_watermark["accepted_delta"]
            or ingest_outcomes["submitted"]
            != ingest_outcomes["queued_accepted_attempts"]
            + ingest_outcomes["inline_document_attempts"]
            or ingest_outcomes["submitted"]
            != ingest_outcomes["document_vector_succeeded_attempts"]
            + ingest_outcomes["document_lexical_degraded_attempts"]
            + ingest_outcomes["task_failed_attempts"]
            or ingest_outcomes["other_succeeded_attempts"] != 0
        ):
            raise ValueError("QA ingest outcome evidence does not conserve the submitted cohort")
    if result_schema == "plico.benchmark-result/v6":
        validate_qa_retrieval_policy(runtime["retrieval_policy"])
        for item in ledger:
            execution = validate_retrieval_execution(item.get("retrieval_execution"))
            validate_exact_qa_execution(execution)
    requirement = runtime["requirement"]
    if requirement not in {"typed_execution_observed", "real_non_stub_vector_per_query"}:
        raise ValueError("QA retrieval runtime requirement is unsupported")
    for key in (
        "configured_embedding_backend",
        "active_embedding_provider",
        "embedding_provider_state",
        "provider_identity_scope",
    ):
        if not isinstance(runtime[key], str) or not runtime[key]:
            raise ValueError("QA retrieval runtime identity is invalid")
    expected_scope = provider_identity_scope(
        runtime["configured_embedding_backend"],
        runtime["active_embedding_provider"],
        runtime["embedding_provider_state"],
    )
    if (
        runtime["provider_identity_scope"] not in PROVIDER_IDENTITY_SCOPES
        or runtime["provider_identity_scope"] != expected_scope
    ):
        raise ValueError("QA retrieval provider identity scope is inconsistent")
    if requirement == "real_non_stub_vector_per_query":
        if (
            (
                ingest_outcomes is not None
                and (
                    runtime.get("cognitive_pipeline")
                    not in (
                        {"max_in_flight": 3, "queue_capacity": 8192},
                        {"max_in_flight": 4, "queue_capacity": 8192},
                    )
                    or ingest_outcomes["inline_document_attempts"] != 0
                    or ingest_outcomes["submitted"]
                    > runtime["cognitive_pipeline"]["queue_capacity"]
                )
            )
            or runtime["configured_embedding_backend"].lower()
            in {"stub", "none", "disabled", "unknown"}
            or expected_scope == "unavailable"
            or (
                ingest_outcomes is not None
                and (
                    ingest_outcomes["document_lexical_degraded_attempts"] != 0
                    or ingest_outcomes["task_failed_attempts"] != 0
                )
            )
            or any(
                item.get("embedding_query_state") != "succeeded"
                or item.get("verified_vector_execution") is not True
                or item.get("retrieval_degraded") is not False
                for item in ledger
            )
        ):
            raise ValueError("QA real-vector execution evidence is incomplete")


def _f1_from_counts(predicted: int, expected: int, common: int) -> float:
    if predicted == 0 and expected == 0:
        return 1.0
    if predicted == 0 or expected == 0 or common == 0:
        return 0.0
    precision = common / predicted
    recall = common / expected
    return 2 * precision * recall / (precision + recall)


def _bleu1_from_counts(predicted: int, expected: int, common: int) -> float:
    if predicted == 0 or expected == 0:
        return 0.0
    precision = common / predicted
    brevity_penalty = math.exp(1 - expected / predicted) if predicted < expected else 1.0
    return precision * brevity_penalty


def _validate_qa_aggregates(
    metrics: dict[str, Any], ledger: list[dict[str, Any]], manifest: dict[str, Any]
) -> None:
    grouped: dict[str, list[dict[str, Any]]] = {}
    for item in ledger:
        grouped.setdefault(str(item.get("stratum")), []).append(item)

    def aggregate(items: list[dict[str, Any]]) -> dict[str, Any]:
        answerable = [item for item in items if item["answerability"] == "answerable"]
        adversarial = [
            item for item in items if item["answerability"] == "adversarial_unanswerable"
        ]
        recalls = [
            item["evidence_recall@10"] for item in items if item["evidence_recall@10"] is not None
        ]
        scores = [item["llm_score"] for item in answerable]
        abstentions = [bool(item["abstention_correct"]) for item in adversarial]
        return {
            "count": len(items),
            "answerable_count": len(answerable),
            "adversarial_unanswerable_count": len(adversarial),
            "f1": (sum(item["f1"] for item in answerable) / len(answerable) if answerable else 0.0),
            "bleu1": (
                sum(item["bleu1"] for item in answerable) / len(answerable) if answerable else 0.0
            ),
            "llm_score": sum(scores) / len(scores) if scores else 0.0,
            "accuracy_pct": accuracy_pct(scores),
            "adversarial_abstention_accuracy_pct": (
                round(sum(abstentions) / len(abstentions) * 100, 1) if abstentions else None
            ),
            "evidence_recall@10": sum(recalls) / len(recalls) if recalls else None,
        }

    expected_categories = {category: aggregate(items) for category, items in grouped.items()}
    if metrics.get("per_category") != expected_categories:
        raise ValueError("QA per-category metrics do not recompute from the ledger")
    expected_overall = aggregate(ledger)
    expected_overall["f1_statistics"] = compute_statistics(
        [item["f1"] for item in ledger if item["f1"] is not None],
        seed=manifest["sampling"]["seed"],
    )
    if metrics.get("overall") != expected_overall:
        raise ValueError("QA overall metrics do not recompute from the ledger")
