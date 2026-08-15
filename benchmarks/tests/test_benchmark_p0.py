"""P0 regression tests for benchmark packaging, execution, and measurement."""

from __future__ import annotations

from pathlib import Path

import pytest

from plico_benchmarks.core import cache
from plico_benchmarks.core.client import PlicoClient
from plico_benchmarks.datasets.locomo import LoCoMoDataset
from plico_benchmarks.suites.performance import PerformanceSuite

BENCHMARK_ROOT = Path(__file__).resolve().parent.parent


def test_dataset_sources_are_not_shadowed_by_payload_ignore_rule():
    ignore_lines = (BENCHMARK_ROOT / ".gitignore").read_text(encoding="utf-8").splitlines()

    assert "/datasets/" in ignore_lines
    assert "datasets/" not in ignore_lines
    assert "/src/*.egg-info/" in ignore_lines


def test_local_dataset_cache_round_trip_does_not_need_download(monkeypatch, tmp_path):
    monkeypatch.setattr(cache, "CACHE_ROOT", tmp_path)
    dataset = LoCoMoDataset()
    expected = [{"conversation": {"session_1": []}, "qa": []}]

    dataset.save_to_cache(expected)

    assert dataset.load() == expected
    assert not hasattr(cache, "download")


def test_dataset_loader_does_not_fall_back_to_legacy_paths(monkeypatch, tmp_path):
    monkeypatch.setattr(cache, "CACHE_ROOT", tmp_path / "cache")

    with pytest.raises(FileNotFoundError, match="local cache"):
        LoCoMoDataset().load()


def test_dead_python_embedding_stack_is_removed():
    pyproject = (BENCHMARK_ROOT / "pyproject.toml").read_text(encoding="utf-8")
    lockfile = (BENCHMARK_ROOT / "uv.lock").read_text(encoding="utf-8")

    assert "sentence-transformers" not in pyproject
    assert 'name = "sentence-transformers"' not in lockfile
    assert not (BENCHMARK_ROOT / "src/plico_benchmarks/core/embedding.py").exists()
    assert not (BENCHMARK_ROOT / "scripts/embedding_server.py").exists()
    assert not (BENCHMARK_ROOT / "configs/embedding_models.yaml").exists()
    egg_info = BENCHMARK_ROOT / "src/plico_benchmarks.egg-info"
    assert not egg_info.exists() or not any(egg_info.iterdir())


def test_fresh_vault_runner_replaces_removed_run_all_command():
    script = (BENCHMARK_ROOT / "scripts/run_full_benchmark.sh").read_text(encoding="utf-8")

    assert "SUITES=(performance retrieval memory-recall-lexical conversational-qa)" in script
    assert "for ordinal in 1 2 3 4 5" in script
    assert "mktemp -d" in script
    assert "--samples" not in script
    assert "LLAMA_URL" not in script
    assert "gemma" not in script.casefold()


def test_performance_uses_versioned_yaml_config_and_rejects_uniform_override():
    defaults = PerformanceSuite(seed=42)._effective_performance_config()

    assert defaults["config_source"] == "configs/benchmark.yaml"
    assert defaults["object_put"] == 250
    assert defaults["memory_create"] == 100
    assert defaults["session_round_trips"] == 20
    assert defaults["search_warm_queries"] == [
        "machine learning",
        "neural network",
        "deep learning",
    ]
    assert defaults["samples_override"] is None
    assert defaults["warmup"] == {"enabled": True, "readiness_requests": 5}
    with pytest.raises(ValueError, match="does not accept a uniform --samples override"):
        PerformanceSuite(samples=7, seed=42)._effective_performance_config()
    assert defaults["projection_timeout_seconds"] == 120.0


def test_performance_latency_uses_real_request_distribution():
    result = PerformanceSuite(seed=42)._latency_result("object.put", [1.0, 1.0, 10.0, 20.0])

    assert result["p50_ms"] == pytest.approx(5.5)
    assert result["p95_ms"] == pytest.approx(18.5)
    assert result["p99_ms"] == pytest.approx(19.7)


def test_real_performance_setup_explicitly_bootstraps_owner_projection(monkeypatch):
    class FakeClient:
        def __init__(self):
            self.readiness_calls = 0
            self.rebuild_calls = 0

        def runtime_readiness(self):
            self.readiness_calls += 1
            state = "degraded" if self.readiness_calls <= 2 else "ready"
            worker = "unavailable" if self.readiness_calls <= 2 else "ready"
            return {"ready": True, "projection": {"control_plane": state, "worker": worker}}

        def projection_rebuild_all_eligible(self):
            self.rebuild_calls += 1
            return {
                "kind": "memory_embedding",
                "selected_count": 0,
                "manifest_generation": 1,
            }

    monkeypatch.setenv("PLICO_BENCH_REQUIRE_REAL_EMBEDDING", "1")
    client = FakeClient()
    suite = PerformanceSuite(client=client, seed=42)

    suite.setup()

    assert client.rebuild_calls == 1
    assert suite._projection_owner_setup == {
        "required": True,
        "owner_rebuild_performed": True,
        "selected_count": 0,
        "manifest_generation": 1,
    }


def test_client_does_not_retry_non_idempotent_writes(monkeypatch):
    client = PlicoClient(max_retries=3, bearer_token="bench-token")
    attempts = 0

    monkeypatch.setattr(client, "ensure_connected", lambda: None)

    def fail_send(_payload):
        nonlocal attempts
        attempts += 1
        raise ConnectionError("response lost")

    monkeypatch.setattr(client, "_send", fail_send)

    with pytest.raises(ConnectionError, match="response lost"):
        client.memory_create("payload", ["test"])

    assert attempts == 1


def test_client_rejects_zero_attempt_configuration():
    with pytest.raises(ValueError, match="at least 1"):
        PlicoClient(max_retries=0)


def test_client_memory_update_uses_typed_public_protocol(monkeypatch):
    requests = []
    client = PlicoClient(bearer_token="bench-token")
    monkeypatch.setattr(
        client,
        "request",
        lambda operation, input_data: requests.append((operation, input_data)) or {},
    )

    client.memory_update("11111111-1111-4111-8111-111111111111", "new content")

    assert requests == [
        (
            "memory.update",
            {
                "entry_id": "11111111-1111-4111-8111-111111111111",
                "content": "new content",
            },
        )
    ]


def test_performance_manifest_binds_the_v2_synthetic_workload():
    suite = PerformanceSuite(seed=17)
    suite._performance_run_config = suite._effective_performance_config()

    artifacts = suite.input_artifacts()

    assert len(artifacts) == 1
    assert artifacts[0]["role"] == "performance_synthetic_workload"
    assert artifacts[0]["bytes"] > 0
    assert len(artifacts[0]["sha256"]) == 64
