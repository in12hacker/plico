"""Regression tests for benchmark comparability and reproducibility metadata."""

import json
import sys
from pathlib import Path
from typing import Any

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.core.client import PlicoClient
from plico_benchmarks.core.harness import BaseSuite, _sanitize_url
from plico_benchmarks.core.integrity import build_run_manifest, validate_run_manifest
from plico_benchmarks.core.reporter import MultiReporter, Report
from plico_benchmarks.core.result_artifact import (
    RESULT_FILE,
    RUN_MANIFEST_FILE,
    commit_result_directory,
    verify_result_directory,
)
from plico_benchmarks.suites.performance import PerformanceSuite


class _NoNetworkSuite(BaseSuite):
    name = "metadata-test"

    def setup(self) -> None:
        pass

    def run(self) -> list[dict[str, Any]]:
        return [
            {"dataset": "alpha", "score": 1.0},
            {"dataset": "alpha", "score": 0.0},
            {"dataset": "beta", "score": 1.0},
        ]

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        return {"overall": {"count": len(raw)}}


@pytest.mark.parametrize(
    ("result_schema", "metadata_version"),
    [("plico.benchmark-result/v5", 5), ("plico.benchmark-result/v6", 6)],
)
def test_resigned_qa_result_cannot_bypass_verifier_by_changing_metadata_suite(
    tmp_path, result_schema, metadata_version
):
    run_id = "11111111-1111-4111-8111-111111111111"
    manifest = build_run_manifest(
        run_id=run_id,
        suite="conversational-qa",
        requested=0,
        actual=0,
        seed=42,
        input_artifacts=[],
        raw_results=[],
        source_watermark="unavailable_public_v2",
        external_evidence=[],
        run_class="research",
    )
    manifest["schemas"]["result"] = result_schema
    result = {
        "metadata": {
            "suite": "retrieval",
            "run_id": run_id,
            "result_schema_version": metadata_version,
        },
        "run_manifest": manifest,
    }

    output = tmp_path / result_schema.rsplit("/", 1)[-1]
    with pytest.raises(ValueError, match="metadata and manifest suites differ"):
        commit_result_directory(output, result)
    assert not output.exists()


@pytest.mark.parametrize("environment", [{}, {"PLICO_KG_AUTO_EXTRACT": "true"}])
def test_v6_qa_result_rejects_missing_or_true_signed_kg_environment(tmp_path, environment):
    run_id = "22222222-2222-4222-8222-222222222222"
    manifest = build_run_manifest(
        run_id=run_id,
        suite="conversational-qa",
        requested=0,
        actual=0,
        seed=42,
        input_artifacts=[],
        raw_results=[],
        source_watermark="unavailable_public_v2",
        external_evidence=[],
        run_class="research",
    )
    result = {
        "metadata": {
            "suite": "conversational-qa",
            "run_id": run_id,
            "result_schema_version": 6,
        },
        "config": {"environment": environment},
        "run_manifest": manifest,
    }

    with pytest.raises(ValueError, match="PLICO_KG_AUTO_EXTRACT=false"):
        commit_result_directory(tmp_path / f"kg-{len(environment)}", result)


def test_report_retains_reproducibility_configuration(monkeypatch):
    monkeypatch.setenv("PLICO_READER_MODEL", "deepseek-v4-flash")
    monkeypatch.setenv("OPENAI_API_KEY", "must-not-be-recorded")
    monkeypatch.setenv(
        "PLICO_READER_API_BASE",
        "https://reader:secret@example.test:8443/v1?token=must-not-be-recorded",
    )

    report = _NoNetworkSuite(samples=7, seed=123).execute(preprocess_timeout=9.5)

    assert report.data["metadata"]["result_schema_version"] == 4
    assert "raw_results" not in report.data
    config = report.data["config"]
    assert config["samples"] == 7
    assert config["samples_requested"] == 7
    assert config["samples_evaluated"] == 3
    assert config["seed"] == 123
    assert config["preprocess_timeout_seconds"] == 9.5
    assert config["client"] == {"transport": "tcp", "host": "127.0.0.1", "port": 7878}
    assert config["samples_evaluated_by_dataset"] == {"alpha": 2, "beta": 1}
    assert config["raw_results_included"] is False
    assert config["environment"]["PLICO_READER_MODEL"] == "deepseek-v4-flash"
    assert config["environment"]["PLICO_READER_API_BASE"] == "https://example.test:8443"
    assert "OPENAI_API_KEY" not in report.to_json()
    assert "reader:secret" not in report.to_json()
    assert "secret" not in report.to_json()
    assert "token" not in report.to_json()
    manifest = report.data["run_manifest"]
    assert manifest["schema_version"] == "plico.memory-eval-run/v1"
    assert manifest["sampling"] == {
        "requested": 7,
        "actual": 3,
        "scored": 3,
        "failed": 0,
        "excluded": 0,
        "seed": 123,
    }
    assert manifest["independent_runs_observed"] == 1
    assert manifest["comparative_inference"] == "not_available_single_run"
    assert manifest["schemas"]["canonical_ledger_root"] == "plico.memory.root/v1"
    assert manifest["schemas"]["projection_manifest_root"] == "plico.projection.manifest-root/v1"
    assert manifest["protocol"] == "plico.personal.v2"
    assert manifest["artifact_binding"]["status"] == "bound_when_result_is_saved"
    assert manifest["hardware"]["logical_cpus"]
    assert manifest["git_state"]["state"] == "available"
    assert len(manifest["git_state"]["worktree_digest_sha256"]) == 64
    validate_run_manifest(manifest)


def test_report_save_writes_private_machine_readable_run_manifest(tmp_path):
    report = _NoNetworkSuite(samples=3, seed=9).execute()
    result_directory = tmp_path / "result"

    report.commit_result(result_directory)

    result_path = result_directory / RESULT_FILE
    manifest_path = result_directory / RUN_MANIFEST_FILE
    sidecar = json.loads(manifest_path.read_text(encoding="utf-8"))
    assert {
        key: value for key, value in sidecar.items() if key != "result_artifact"
    } == report.data["run_manifest"]
    assert sidecar["result_artifact"]["file_name"] == RESULT_FILE
    assert sidecar["result_artifact"]["bytes"] == result_path.stat().st_size
    assert len(sidecar["result_artifact"]["sha256"]) == 64
    assert result_path.stat().st_mode & 0o777 == 0o600
    assert manifest_path.stat().st_mode & 0o777 == 0o600
    assert result_directory.stat().st_mode & 0o777 == 0o700
    assert verify_result_directory(result_directory)["run_manifest"] == report.data["run_manifest"]


def test_run_manifest_rejects_broken_count_conservation():
    manifest = _NoNetworkSuite(samples=3, seed=9).execute().data["run_manifest"]
    manifest["sampling"]["failed"] = 1

    try:
        validate_run_manifest(manifest)
    except ValueError as error:
        assert "actual must equal" in str(error)
    else:
        raise AssertionError("broken count conservation must fail closed")


def test_run_manifest_rejects_the_removed_v1_protocol():
    manifest = _NoNetworkSuite(samples=1, seed=9).execute().data["run_manifest"]
    manifest["protocol"] = "plico.personal.v1"

    try:
        validate_run_manifest(manifest)
    except ValueError as error:
        assert "protocol mismatch" in str(error)
    else:
        raise AssertionError("the removed v1 protocol must fail closed")


def test_run_class_is_typed_and_official_is_globally_unsupported(monkeypatch):
    monkeypatch.setenv("PLICO_BENCH_RUN_CLASS", "invented")
    try:
        _NoNetworkSuite(seed=9).execute()
    except ValueError as error:
        assert "unsupported benchmark run class" in str(error)
    else:
        raise AssertionError("an invented run class must fail before suite setup")

    monkeypatch.setenv("PLICO_BENCH_RUN_CLASS", "official")
    try:
        _NoNetworkSuite(seed=9).execute()
    except RuntimeError as error:
        assert "official conformance is unsupported" in str(error)
    else:
        raise AssertionError("official must remain unsupported until provenance is complete")


def test_release_evidence_class_is_reserved_for_release_suite(monkeypatch):
    monkeypatch.setenv("PLICO_BENCH_RUN_CLASS", "release_evidence")

    try:
        _NoNetworkSuite(seed=9).execute()
    except RuntimeError as error:
        assert "not valid for suite" in str(error)
    else:
        raise AssertionError("ordinary suites must not self-declare release evidence")


def test_run_manifest_rejects_rebound_suite_run_class():
    manifest = _NoNetworkSuite(samples=1, seed=9).execute().data["run_manifest"]
    manifest["suite"] = "v1b-release"

    try:
        validate_run_manifest(manifest)
    except ValueError as error:
        assert "suite/run class combination" in str(error)
    else:
        raise AssertionError("suite/run class rebound must fail closed")


def test_real_embedding_run_rejects_stub_backend(monkeypatch):
    monkeypatch.setenv("PLICO_BENCH_REQUIRE_REAL_EMBEDDING", "1")
    monkeypatch.setenv("EMBEDDING_BACKEND", "stub")

    try:
        _NoNetworkSuite(samples=3, seed=9).execute()
    except RuntimeError as error:
        assert "non-stub" in str(error)
    else:
        raise AssertionError("stub embedding must not enter a real-embedding run")


def test_raw_results_require_explicit_opt_in(monkeypatch):
    monkeypatch.setenv("PLICO_BENCH_INCLUDE_RAW_RESULTS", "true")

    report = _NoNetworkSuite(seed=123).execute()

    assert report.data["config"]["raw_results_included"] is True
    assert report.data["raw_results"] == [
        {"dataset": "alpha", "score": 1.0},
        {"dataset": "alpha", "score": 0.0},
        {"dataset": "beta", "score": 1.0},
    ]


def test_url_sanitizer_removes_all_non_origin_components():
    assert (
        _sanitize_url("http://user:pass@[2001:db8::1]:8080/v1?q=token#fragment")
        == "http://[2001:db8::1]:8080"
    )
    assert _sanitize_url("not-a-url?token=secret") == "[redacted-invalid-url]"


def test_scifact_recall_is_not_subtracted_from_mteb_average():
    result = {
        "metadata": {"suite": "retrieval"},
        "metrics": {
            "overall": {"beir_scifact": {"count": 30, "recall@5": 0.687, "recall@10": 0.727}}
        },
    }

    summary = MultiReporter([result])._summary_table()

    assert "beir_scifact.recall@5" in summary
    assert "0.687" in summary
    assert "72.31" not in summary
    assert "NV-Embed" not in summary
    assert "Comparable Baseline" not in summary


def test_ragas_style_scores_are_labeled_as_non_official_proxy():
    report = Report(
        {
            "metadata": {"suite": "qa"},
            "metrics": {"ragas_style_proxy": {"faithfulness": 0.8}},
        }
    )

    markdown = report.to_markdown()

    assert "RAGAS-style LLM-Judge Proxy Metrics" in markdown
    assert "official RAGAS package is not used" in markdown
    assert "Production Target" not in markdown


def test_combined_proxy_report_does_not_compute_reference_gap():
    result = {
        "metadata": {"suite": "conversational-qa"},
        "config": {"seed": 42, "samples_evaluated": 20},
        "metrics": {
            "overall": {"accuracy_pct": 50.0},
            "ragas_style_proxy": {"faithfulness": 0.8},
        },
    }

    markdown = MultiReporter([result]).to_markdown()

    assert "| faithfulness | 0.800 |" in markdown
    assert "Internal Reference" not in markdown
    assert "0.85 (-0.05)" not in markdown
    assert "#### Run Configuration" in markdown
    assert "| seed | 42 |" in markdown
    assert "| samples_evaluated | 20 |" in markdown


def test_search_benchmark_separates_warm_and_cold_workloads(monkeypatch):
    class FakeClient:
        host = "127.0.0.1"
        port = 7878

        def __init__(self):
            self.queries: list[str] = []
            self.cold_targets: dict[str, str] = {}

        def wait_for_object_indexing(self, timeout):
            pass

        def object_search(self, query, limit, require_tags):
            self.queries.append(query)
            cid = self.cold_targets.get(query, "warm-cid")
            return {
                "hits": [{"cid": cid}],
                "embedding_query": {"state": "succeeded"},
                "retrieval": [{"path": "vector", "candidates": 1, "accepted": 1}],
            }

    clock = iter(i / 1000 for i in range(1000))
    monkeypatch.setenv("EMBEDDING_BACKEND", "openai-compatible")
    monkeypatch.setattr(
        "plico_benchmarks.suites.performance.time.perf_counter", lambda: next(clock)
    )
    client = FakeClient()
    suite = PerformanceSuite(client=client, seed=17)
    suite._performance_run_config = {"object_put": 12}
    objects = [(f"cid-{index}", f"unique-query-17-{index}") for index in range(12)]
    # One miss remains a retrieval-quality diagnostic; it must not erase the
    # independently valid latency/vector-execution measurement.
    client.cold_targets = {token: cid for cid, token in objects[1:]}

    results = suite._bench_object_search(
        objects,
        12,
        ["machine learning", "neural network", "deep learning"],
    )

    assert [result["operation"] for result in results] == [
        "object.search_warm_repeated",
        "object.search_query_cold_unique",
    ]
    assert results[0]["count"] == 6
    assert results[0]["query_cardinality"] == 3
    assert results[1]["count"] == 6
    assert results[1]["query_cardinality"] == 6
    assert all("qps" not in result for result in results)
    assert all("serial_service_rate" in result for result in results)
    assert all(result["rate_unit"] == "requests/s" for result in results)
    assert results[1]["includes_query_embedding_stage"] is True
    assert results[1]["query_embedding_backend"] == "openai-compatible"
    assert results[1]["retrieval_claim"] == "verified_vector_execution_latency"
    assert results[1]["verified_vector_query_count"] == 6
    assert results[1]["degraded_query_count"] == 0
    assert results[1]["expected_target_query_count"] == 6
    assert results[1]["expected_target_hit_count"] == 5
    assert results[1]["expected_target_hit_rate"] == 0.833333
    assert len(results[1]["query_execution_ledger"]) == 6
    assert "includes_remote_query_embedding" not in results[1]
    cold_queries = client.queries[-6:]
    assert len(cold_queries) == len(set(cold_queries))
    assert all("unique-query-17" in query for query in cold_queries)


def test_client_working_memory_uses_typed_create_operation(monkeypatch):
    client = PlicoClient(bearer_token="bench-token")
    requests = []
    monkeypatch.setattr(
        client,
        "request",
        lambda operation, input_data: requests.append((operation, input_data)) or {},
    )

    client.memory_create("remember me", tags=["test"])

    assert requests == [("memory.create", {"content": "remember me", "tags": ["test"]})]


def test_memory_recall_latency_preserves_target_miss_as_quality_diagnostic(monkeypatch):
    class FakeClient:
        def __init__(self):
            self.calls = 0

        def memory_recall(self, query, limit):
            self.calls += 1
            assert query == "unique-token"
            assert limit == 10
            if self.calls == 1:
                return {"hits": [{"entry": {"entry_id": "revision-1"}}]}
            return {"hits": []}

    clock = iter([0.0, 0.001, 0.002, 0.005])
    monkeypatch.setattr(
        "plico_benchmarks.suites.performance.time.perf_counter", lambda: next(clock)
    )
    suite = PerformanceSuite(client=FakeClient(), seed=17)

    result = suite._bench_memory_recall([("revision-1", "unique-token")], 2)

    assert result["count"] == 2
    assert result["expected_target_query_count"] == 2
    assert result["expected_target_hit_count"] == 1
    assert result["expected_target_hit_rate"] == 0.5
    assert result["quality_boundary"] == "latency_workload_with_target_hit_diagnostic"


def test_working_memory_ack_and_projection_observability(monkeypatch):
    class FakeMemoryClient:
        host = "127.0.0.1"
        port = 7878

        def memory_create(self, content, tags):
            entry_id = content.rsplit("-", 1)[-1]
            return {"entry": {"entry_id": entry_id}}

        def projection_status(self, entry_id):
            return {
                "kind": "memory_embedding",
                "status": {
                    "observation": "observed",
                    "revision_id": entry_id,
                    "state": {"state": "ready"},
                },
            }

    clock = iter(i / 1000 for i in range(1000))
    monkeypatch.setattr(
        "plico_benchmarks.suites.performance.time.perf_counter", lambda: next(clock)
    )
    suite = PerformanceSuite(client=FakeMemoryClient(), seed=9)

    single, entry_ids = suite._bench_memory_create(4)
    projection = suite._bench_memory_projection_lag(entry_ids, timeout=1.0, poll_interval=0.001)

    assert single["count"] == 4
    assert single["acknowledgement_boundary"] == "canonical_working_memory_persisted"
    assert "qps" not in single
    assert projection["status"] == "measured"
    assert projection["ready"] == 4
    assert projection["failed"] == 0
    assert projection["timeout"] == 0
    assert projection["count"] == 4


def test_performance_sample_count_sums_concrete_operation_counts():
    suite = PerformanceSuite(seed=42)
    suite._raw_results = [
        {"operation": "memory.create_ack", "count": 4, "status": "measured"},
        {"operation": "object.search_warm_repeated", "count": 6, "status": "measured"},
        {
            "operation": "search",
            "count": 12,
            "status": "measured",
            "is_aggregate": True,
        },
        {
            "operation": "projection.memory_embedding_catch_up",
            "count": 0,
            "status": "measured",
        },
    ]
    report = Report({"metadata": {}, "config": {}})

    suite._add_reproducibility_config(report, preprocess_timeout=10.0)

    assert report.data["config"]["samples_evaluated"] == 10
    assert report.data["config"]["samples_evaluated_by_operation"] == {
        "memory.create_ack": 4,
        "projection.memory_embedding_catch_up": 0,
        "object.search_warm_repeated": 6,
    }


def test_projection_lag_reports_queued_ready_failed_and_timeout(monkeypatch):
    class ProjectionClient:
        host = "127.0.0.1"
        port = 7878

        def __init__(self):
            self.calls: dict[str, int] = {}

        def projection_status(self, entry_id):
            self.calls[entry_id] = self.calls.get(entry_id, 0) + 1
            if entry_id == "ready-after-queue":
                state = "queued" if self.calls[entry_id] == 1 else "ready"
            elif entry_id == "failed":
                state = "failed"
            else:
                return {
                    "kind": "memory_embedding",
                    "status": {
                        "observation": "unreconciled",
                        "revision_id": entry_id,
                    },
                }
            return {
                "kind": "memory_embedding",
                "status": {
                    "observation": "observed",
                    "revision_id": entry_id,
                    "state": {"state": state},
                },
            }

    ticks = iter(index / 10 for index in range(100))
    monkeypatch.setattr(
        "plico_benchmarks.suites.performance.time.perf_counter", lambda: next(ticks)
    )
    monkeypatch.setattr("plico_benchmarks.suites.performance.time.sleep", lambda _: None)
    suite = PerformanceSuite(client=ProjectionClient(), seed=3)

    result = suite._bench_memory_projection_lag(
        [
            ("ready-after-queue", "token-ready"),
            ("failed", "token-failed"),
            ("timeout", "token-timeout"),
        ],
        timeout=0.7,
        poll_interval=0.01,
    )

    assert result["queued"] > 0
    assert result["unreconciled"] > 0
    assert result["ready"] == 1
    assert result["failed"] == 1
    assert result["timeout"] == 1
    assert result["status"] == "partial"
    assert result["failure_count"] == 2
    assert result["count"] >= 4
    assert result["phase_elapsed_ms"] > 0

    suite._raw_results = [result]
    report = Report({"metadata": {}, "config": {}})
    suite._add_reproducibility_config(report, preprocess_timeout=1.0)
    manifest = report.data["run_manifest"]
    assert manifest["sampling"]["actual"] == 3
    assert manifest["sampling"]["failed"] == 2
    assert manifest["sampling"]["scored"] == 1
    assert manifest["failure_ledger"][0]["count"] == 2
