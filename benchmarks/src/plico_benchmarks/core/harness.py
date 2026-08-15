"""Benchmark harness base class — all suites extend this."""

from __future__ import annotations

import os
import uuid
from abc import ABC, abstractmethod
from datetime import datetime, timezone
from typing import Any
from urllib.parse import urlsplit

from plico_benchmarks.core.client import PlicoClient
from plico_benchmarks.core.integrity import (
    build_run_manifest,
    resolve_run_class,
    validate_real_embedding_requirement,
)
from plico_benchmarks.core.reporter import Report


class BaseSuite(ABC):
    """Abstract base class for all benchmark suites."""

    name: str = "base"
    description: str = ""

    def __init__(
        self,
        client: PlicoClient | None = None,
        host: str = "127.0.0.1",
        port: int = 7878,
        uds_path: str | None = None,
        samples: int | None = None,
        seed: int | None = None,
    ):
        self.client = client or PlicoClient(host=host, port=port, uds_path=uds_path)
        self.samples = samples
        self.seed = seed if seed is not None else int(os.environ.get("PLICO_SEED", "42"))
        configured_run_id = os.environ.get("PLICO_BENCH_RUN_ID")
        if configured_run_id is None:
            self.run_id = str(uuid.uuid4())
        else:
            try:
                canonical_run_id = str(uuid.UUID(configured_run_id))
            except (ValueError, AttributeError) as error:
                raise ValueError("PLICO_BENCH_RUN_ID must be a canonical UUID") from error
            if configured_run_id != canonical_run_id:
                raise ValueError("PLICO_BENCH_RUN_ID must use canonical hyphenated form")
            self.run_id = canonical_run_id
        journal_run_id = os.environ.get("PLICO_LLM_RUN_ID")
        if journal_run_id is not None and journal_run_id != self.run_id:
            raise ValueError("benchmark and LLM journal run IDs must match exactly")
        self._raw_results: list[dict[str, Any]] = []
        self._metrics: dict[str, Any] = {}

    @abstractmethod
    def setup(self) -> None:
        """Load/prepare local data and warm up connections."""
        ...

    @abstractmethod
    def run(self) -> list[dict[str, Any]]:
        """Execute the benchmark and return raw per-sample results."""
        ...

    @abstractmethod
    def evaluate(self, raw: list[dict[str, Any]]) -> dict[str, Any]:
        """Compute aggregated metrics from raw results."""
        ...

    def report(self, metrics: dict[str, Any]) -> Report:
        """Build a standardized report dict."""
        import os

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": os.environ.get("PLICO_BENCH_VERSION", "dev"),
                "timestamp": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                "plico_version": "0.1.0",
            },
            "config": {
                "samples": self.samples,
            },
            "metrics": metrics,
            "raw_results": self._raw_results,
        }
        return Report(report_data)

    def wait_for_indexing(self, timeout: float = 300.0) -> None:
        """Convenience wrapper — wait until recent writes are searchable."""
        self.client.wait_for_object_indexing(timeout=timeout)

    def execute(self, preprocess_timeout: float = 120.0) -> Report:
        """Orchestrate the full benchmark lifecycle."""
        self._run_class = resolve_run_class(self.name)
        self.setup()
        self._preprocess_timeout = preprocess_timeout
        self._raw_results = self.run()
        validate_real_embedding_requirement(self._raw_results)
        self._metrics = self.evaluate(self._raw_results)
        report = self.report(self._metrics)
        self._add_reproducibility_config(report, preprocess_timeout)
        return report

    def evaluated_sample_count(self) -> int:
        """Return the number of evaluated samples represented by this run."""
        return len(self._raw_results)

    def evaluated_samples_by_operation(self) -> dict[str, int]:
        """Return per-operation sample counts when a suite is operation based."""
        return {}

    def _add_reproducibility_config(self, report: Report, preprocess_timeout: float) -> None:
        """Attach the non-secret inputs needed to interpret or repeat a run.

        Suites historically supplied their own ``report()`` implementations,
        which meant that sample limits and seeds were frequently omitted.  Do
        this once after every suite report is built so JSON artifacts have a
        consistent, auditable configuration record.
        """
        config = report.data.setdefault("config", {})
        config.setdefault("samples", self.samples)
        config["samples_requested"] = self.samples
        config["samples_evaluated"] = self.evaluated_sample_count()
        config["run_id"] = self.run_id
        config["seed"] = self.seed
        config["preprocess_timeout_seconds"] = preprocess_timeout
        run_class = getattr(self, "_run_class", None)
        if run_class is None:
            run_class = resolve_run_class(self.name)
        config["run_class"] = run_class
        config["client"] = {
            "transport": getattr(self.client, "transport", "tcp"),
            "host": getattr(self.client, "host", None)
            if getattr(self.client, "transport", "tcp") == "tcp"
            else None,
            "port": getattr(self.client, "port", None)
            if getattr(self.client, "transport", "tcp") == "tcp"
            else None,
        }

        dataset_counts: dict[str, int] = {}
        for item in self._raw_results:
            dataset = item.get("dataset")
            if dataset:
                dataset_counts[str(dataset)] = dataset_counts.get(str(dataset), 0) + 1
        config["samples_evaluated_by_dataset"] = dict(sorted(dataset_counts.items()))
        operation_counts = self.evaluated_samples_by_operation()
        if operation_counts:
            config["samples_evaluated_by_operation"] = dict(sorted(operation_counts.items()))

        # Explicit allowlist: retain model/runtime inputs without leaking API
        # keys or arbitrary process environment variables into result files.
        environment_keys = (
            "PLICO_READER_PROVIDER",
            "PLICO_READER_API_BASE",
            "PLICO_READER_MODEL",
            "PLICO_READER_THINKING",
            "PLICO_READER_REASONING_EFFORT",
            "PLICO_READER_TEMPERATURE",
            "PLICO_READER_TOP_P",
            "PLICO_READER_MAX_ATTEMPTS",
            "PLICO_JUDGE_PROVIDER",
            "PLICO_JUDGE_API_BASE",
            "PLICO_JUDGE_MODEL",
            "PLICO_JUDGE_THINKING",
            "PLICO_JUDGE_REASONING_EFFORT",
            "PLICO_JUDGE_TEMPERATURE",
            "PLICO_JUDGE_TOP_P",
            "PLICO_JUDGE_MAX_ATTEMPTS",
            "PLICO_RERANKER_MODEL",
            "PLICO_RERANKER_API_BASE",
            "PLICO_KG_AUTO_EXTRACT",
        )
        url_keys = {
            "PLICO_READER_API_BASE",
            "PLICO_JUDGE_API_BASE",
            "PLICO_RERANKER_API_BASE",
        }
        config["environment"] = {
            key: _sanitize_url(os.environ[key]) if key in url_keys else os.environ[key]
            for key in environment_keys
            if key in os.environ
        }
        include_raw_results = os.environ.get("PLICO_BENCH_INCLUDE_RAW_RESULTS", "").lower() in {
            "1",
            "true",
            "yes",
        }
        config["raw_results_included"] = include_raw_results
        if include_raw_results:
            report.data["raw_results"] = self._raw_results
        else:
            report.data.pop("raw_results", None)
        metadata = report.data.setdefault("metadata", {})
        metadata["result_schema_version"] = 4
        metadata["run_id"] = self.run_id
        report.data["run_manifest"] = build_run_manifest(
            run_id=self.run_id,
            suite=self.name,
            requested=self.samples,
            actual=self.evaluated_sample_count(),
            seed=self.seed,
            input_artifacts=self.input_artifacts(),
            raw_results=self._raw_results,
            source_watermark=self.source_watermark(),
            external_evidence=self.external_evidence(),
            run_class=run_class,
            llm_evidence=report.data.get("metrics", {}).get("llm_evidence"),
        )

    def input_artifacts(self) -> list[dict[str, Any]]:
        """Return verified input artifacts consumed by this suite."""
        return []

    def source_watermark(self) -> dict[str, Any] | str:
        """Return the source state that bounded this run's observations."""
        return "unavailable_public_v2"

    def external_evidence(self) -> list[dict[str, Any]]:
        """Return separately-scoped evidence linked to, not scored in, this run."""
        return []

    def __enter__(self) -> BaseSuite:
        self.client.__enter__()
        return self

    def __exit__(self, *args: Any) -> None:
        self.client.__exit__(*args)


def _sanitize_url(value: str) -> str:
    """Retain only a URL's scheme and host/port for benchmark artifacts."""
    try:
        parsed = urlsplit(value)
        hostname = parsed.hostname
        if not parsed.scheme or not hostname:
            return "[redacted-invalid-url]"
        host = f"[{hostname}]" if ":" in hostname else hostname
        port = parsed.port
        authority = f"{host}:{port}" if port is not None else host
        return f"{parsed.scheme}://{authority}"
    except ValueError:
        return "[redacted-invalid-url]"
