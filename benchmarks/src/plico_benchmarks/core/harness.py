"""Benchmark harness base class — all suites extend this."""

from __future__ import annotations

from abc import ABC, abstractmethod
from typing import Any

from plico_benchmarks.core.client import PlicoClient
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
        samples: int | None = None,
    ):
        self.client = client or PlicoClient(host=host, port=port)
        self.samples = samples
        self._raw_results: list[dict[str, Any]] = []
        self._metrics: dict[str, Any] = {}

    @abstractmethod
    def setup(self) -> None:
        """Download/prepare data, warm up connections."""
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
        import time

        report_data = {
            "metadata": {
                "suite": self.name,
                "version": "v44",
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ"),
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
        self.client.wait_for_indexing(timeout=timeout)

    def execute(self, preprocess_timeout: float = 120.0) -> Report:
        """Orchestrate the full benchmark lifecycle."""
        self.setup()
        self._preprocess_timeout = preprocess_timeout
        self._raw_results = self.run()
        self._metrics = self.evaluate(self._raw_results)
        return self.report(self._metrics)

    def __enter__(self) -> BaseSuite:
        self.client.__enter__()
        return self

    def __exit__(self, *args: Any) -> None:
        self.client.__exit__(*args)
