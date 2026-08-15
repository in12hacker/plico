"""Fail-closed regression contracts for benchmark security and integrity."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path
from typing import Any

import pytest
from typer.testing import CliRunner

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks import cli
from plico_benchmarks.core import cache
from plico_benchmarks.core.client import PlicoClient, PlicoProtocolError
from plico_benchmarks.core.harness import BaseSuite
from plico_benchmarks.core.integrity import build_run_manifest
from plico_benchmarks.core.judge import Judge
from plico_benchmarks.core.result_artifact import RESULT_FILE, commit_result_directory
from plico_benchmarks.suites.conversational_qa import ConversationalQASuite


class _RawContextSuite(BaseSuite):
    name = "raw-context-audit"

    def setup(self) -> None:
        pass

    def run(self) -> list[dict[str, Any]]:
        return [{"context": "personal-memory-canary", "score": 1.0}]

    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        return {"overall": {"count": len(raw)}}


def test_indexing_probe_write_failure_aborts_benchmark(monkeypatch):
    client = PlicoClient()
    monkeypatch.setattr(
        client,
        "object_put",
        lambda *args, **kwargs: (_ for _ in ()).throw(PlicoProtocolError("token required")),
    )
    monkeypatch.setattr("plico_benchmarks.core.client.time.sleep", lambda _seconds: None)

    with pytest.raises(RuntimeError, match="token required"):
        client.wait_for_object_indexing(timeout=0.01, poll_interval=0.0)


def test_missing_public_bearer_fails_before_network():
    client = PlicoClient(bearer_token=None)

    with pytest.raises(PlicoProtocolError, match="PLICO_BEARER_TOKEN is required"):
        client.runtime_readiness()


def test_default_report_artifact_excludes_personal_memory_context():
    report = _RawContextSuite(seed=7).execute()

    assert "personal-memory-canary" not in report.to_json()


def test_result_artifacts_are_owner_only(tmp_path):
    result_directory = tmp_path / "result"
    markdown_path = tmp_path / "result.md"
    report = _RawContextSuite(seed=7).execute()

    report.commit_result(result_directory)
    report.save_markdown(markdown_path)

    assert result_directory.stat().st_mode & 0o077 == 0
    assert all(path.stat().st_mode & 0o077 == 0 for path in result_directory.iterdir())
    assert markdown_path.stat().st_mode & 0o077 == 0


def test_malformed_cache_metadata_fails_closed(monkeypatch, tmp_path):
    monkeypatch.setattr(cache, "CACHE_ROOT", tmp_path)
    cache.cache_path("integrity-audit").write_text("payload", encoding="utf-8")
    cache.cache_meta_path("integrity-audit").write_text("not-json", encoding="utf-8")

    assert cache.is_cached("integrity-audit") is False


def test_cache_rejects_same_size_payload_tampering(monkeypatch, tmp_path):
    monkeypatch.setattr(cache, "CACHE_ROOT", tmp_path)
    path = cache.save_cache("tamper-audit", "original")
    path.write_text("tampered", encoding="utf-8")

    assert cache.is_cached("tamper-audit") is False

    with pytest.raises(FileNotFoundError, match="invalid cache"):
        cache.load_json_cache("tamper-audit")


def test_report_has_unique_run_identity():
    first = _RawContextSuite(seed=7).execute()
    second = _RawContextSuite(seed=7).execute()

    first_run = first.data["metadata"]["run_id"]
    second_run = second.data["metadata"]["run_id"]
    assert first_run
    assert second_run
    assert first_run != second_run


def test_client_supports_required_auth():
    requests: list[tuple[str, dict[str, Any]]] = []
    client = PlicoClient(bearer_token="bench-token")
    client.request = (  # type: ignore[method-assign]
        lambda operation, input_data: requests.append((operation, input_data)) or {"cid": "abc"}
    )

    client.object_put("payload", ["audit"])

    assert requests == [
        ("object.put", {"content": "payload", "encoding": "utf8", "tags": ["audit"]})
    ]


def test_legacy_agent_token_environment_is_not_an_auth_fallback(monkeypatch):
    monkeypatch.setenv("PLICO_AGENT_TOKEN", "legacy-token")
    monkeypatch.delenv("PLICO_BEARER_TOKEN", raising=False)
    client = PlicoClient()

    with pytest.raises(PlicoProtocolError, match="PLICO_BEARER_TOKEN is required"):
        client.memory_create("must not be sent with legacy auth")


def test_longmemeval_ingest_uses_per_sample_namespace():
    class RecordingClient:
        def __init__(self):
            self.tags: list[list[str]] = []

        def object_put(self, content, tags):
            self.tags.append(tags)
            return {"cid": str(len(self.tags))}

    client = RecordingClient()
    suite = ConversationalQASuite(client=client, seed=4)
    suite.longmemeval = [
        {"question_id": "sample-a", "haystack_sessions": [[{"role": "user", "content": "a"}]]},
        {"question_id": "sample-b", "haystack_sessions": [[{"role": "user", "content": "b"}]]},
    ]
    suite._longmemeval_sample = list(suite.longmemeval)

    suite._ingest_longmemeval()

    namespaces = [{tag for tag in tags if tag.startswith("question:")} for tags in client.tags]
    assert all(namespaces)
    assert namespaces[0].isdisjoint(namespaces[1])


def test_report_generation_aborts_on_malformed_run_artifact(tmp_path):
    valid = tmp_path / "valid"
    malformed = tmp_path / "malformed"
    output = tmp_path / "combined.md"
    run_id = "11111111-1111-4111-8111-111111111111"
    manifest = build_run_manifest(
        run_id=run_id,
        suite="performance",
        requested=1,
        actual=1,
        seed=7,
        input_artifacts=[],
        raw_results=[{"status": "measured", "count": 1}],
        source_watermark="unavailable_public_v2",
        external_evidence=[],
        run_class="research",
    )
    commit_result_directory(
        valid,
        {
            "metadata": {"suite": "performance", "run_id": run_id},
            "metrics": {"overall": {"count": 1}},
            "run_manifest": manifest,
        },
    )
    shutil.copytree(valid, malformed)
    (malformed / RESULT_FILE).write_text("not-json", encoding="utf-8")
    (malformed / RESULT_FILE).chmod(0o600)

    result = CliRunner().invoke(
        cli.app,
        [
            "report",
            "--result-dir",
            str(valid),
            "--result-dir",
            str(malformed),
            "--output",
            str(output),
        ],
    )

    assert result.exit_code != 0
    assert not output.exists()


def test_judge_failure_is_not_counted_as_a_valid_low_score():
    class FailingLlm:
        def chat(self, messages, **kwargs):
            raise ConnectionError("judge unavailable")

        def is_available(self):
            return False

    with pytest.raises(RuntimeError, match="scored judge evaluation failed"):
        Judge(llm=FailingLlm(), retries=1).evaluate_scored("q", "expected", "actual")


def test_malformed_judge_output_is_not_counted_as_score_one():
    class MalformedLlm:
        def chat(self, messages, **kwargs):
            return "unable to decide"

        def is_available(self):
            return True

    with pytest.raises(RuntimeError, match="returned no 1-5 score"):
        Judge(llm=MalformedLlm(), retries=1).evaluate_scored("q", "expected", "actual")


def test_run_all_command_is_removed_because_it_reused_one_dirty_vault():
    result = CliRunner().invoke(cli.app, ["run-all"])

    assert result.exit_code == 2
    assert "No such command 'run-all'" in result.output
