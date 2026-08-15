"""Contracts for suites that contain only public-protocol measurements."""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from plico_benchmarks.suites.performance import PerformanceSuite


def test_performance_report_has_no_unmatched_competitor_data():
    suite = object.__new__(PerformanceSuite)
    suite.samples = 3
    suite._raw_results = []
    suite._performance_run_config = {
        "config_source": "test",
        "samples_override": 3,
    }

    report = suite.report({"overall": {}})

    assert "competitors" not in report.data


def test_removed_non_public_suites_are_not_registered():
    from plico_benchmarks.suites import SUITE_REGISTRY

    assert "kg-reasoning" not in SUITE_REGISTRY
    assert "scope-isolation" not in SUITE_REGISTRY
    assert "token-efficiency" not in SUITE_REGISTRY
    assert "memory-lifecycle" not in SUITE_REGISTRY
    assert "object-storage" not in SUITE_REGISTRY
    assert "session-lifecycle" not in SUITE_REGISTRY
