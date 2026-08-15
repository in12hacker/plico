"""Report generation for protocol-matched benchmark results."""

from __future__ import annotations

import json
import os
import re
import tempfile
import time
from pathlib import Path
from typing import Any

_RICH_TAG_RE = re.compile(r"\[(?:red|green|yellow|blue|bold|dim|/[^]]+)\]")


def _write_private_atomic(path: Path, content: str) -> None:
    """Atomically write a potentially sensitive artifact with owner-only mode."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    temporary = Path(temporary_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.chmod(0o600)
        os.replace(temporary, path)
        path.chmod(0o600)
    finally:
        temporary.unlink(missing_ok=True)


def _ragas_style_proxy(metrics: dict[str, Any]) -> dict[str, Any]:
    """Read scores that are explicitly labelled as the local judge proxy."""
    return metrics.get("ragas_style_proxy", {})


def _render_value(value: Any) -> str:
    if isinstance(value, float):
        return f"{value:.3f}"
    if isinstance(value, dict):
        parts = []
        for key, nested in value.items():
            rendered = f"{nested:.2f}" if isinstance(nested, float) else str(nested)
            parts.append(f"{key}={rendered}")
        return ", ".join(parts)
    return str(value)


def _render_ragas_style_proxy(lines: list[str], metrics: dict[str, Any], heading: str) -> None:
    proxy = _ragas_style_proxy(metrics)
    if not proxy:
        return
    lines.extend(
        [
            heading,
            "",
            "_Custom single-judge prompts; the official RAGAS package is not used. "
            "No unsourced reference threshold is applied._",
            "",
            "| Metric | Score |",
            "|--------|-------|",
        ]
    )
    for key, value in proxy.items():
        lines.append(f"| {key} | {value:.3f} |")
    lines.append("")


class Report:
    """Container for a single benchmark report."""

    def __init__(self, data: dict[str, Any]):
        self.data = data

    def to_json(self, indent: int = 2) -> str:
        return json.dumps(self.data, indent=indent, ensure_ascii=False)

    def commit_result(self, path: Path) -> None:
        """Commit a no-clobber result directory; ``path`` is never a file name."""
        from plico_benchmarks.core.result_artifact import commit_result_directory

        commit_result_directory(path, self.data)

    def to_markdown(self) -> str:
        return _render_markdown(self.data)

    def save_markdown(self, path: Path) -> None:
        _write_private_atomic(path, self.to_markdown())


def _render_markdown(data: dict[str, Any]) -> str:
    lines: list[str] = []
    meta = data.get("metadata", {})
    config = data.get("config", {})
    metrics = data.get("metrics", {})
    costs = data.get("costs", {})

    lines.extend(
        [
            f"# Benchmark Report: {meta.get('suite', 'unknown')}",
            "",
            f"> Version: {meta.get('version', 'unknown')}",
            f"> Timestamp: {meta.get('timestamp', '')}",
            "",
            "## Configuration",
            "",
        ]
    )
    for key, value in config.items():
        lines.append(f"- **{key}**: {value}")
    lines.extend(["", "## Overall Metrics", ""])

    overall = metrics.get("overall", {})
    if overall:
        lines.extend(["| Metric | Value |", "|--------|-------|"])
        for key, value in overall.items():
            lines.append(f"| {key} | {_render_value(value)} |")
    lines.append("")

    per_category = metrics.get("per_category", {})
    if per_category:
        lines.extend(
            [
                "## Per-Category Metrics",
                "",
                "| Category | Count | F1 | EM | LLM Score | Accuracy % |",
                "|----------|-------|----|----|----------|-----------|",
            ]
        )
        for category, values in per_category.items():
            f1 = f"{values['f1']:.3f}" if values.get("f1") is not None else "—"
            em = f"{values['em']:.3f}" if values.get("em") is not None else "—"
            llm_score = f"{values['llm_score']:.2f}" if values.get("llm_score") is not None else "—"
            accuracy = (
                f"{values['accuracy_pct']:.1f}" if values.get("accuracy_pct") is not None else "—"
            )
            lines.append(
                f"| {category} | {values.get('count', 0)} | {f1} | {em} | "
                f"{llm_score} | {accuracy} |"
            )
        lines.append("")

    _render_ragas_style_proxy(lines, metrics, "## RAGAS-style LLM-Judge Proxy Metrics")

    statistics = metrics.get("statistics", {})
    if statistics:
        lines.extend(
            [
                "## Statistics",
                "",
                f"- Mean: {statistics.get('mean', 0):.4f}",
                f"- Std: {statistics.get('std', 0):.4f}",
                f"- 95% CI: [{statistics.get('ci95_low', 0):.4f}, "
                f"{statistics.get('ci95_high', 0):.4f}]",
                "",
            ]
        )

    if costs:
        lines.extend(["## Cost Analysis", ""])
        for key, value in costs.items():
            lines.append(f"- **{key}**: {value}")
        lines.append("")

    lines.extend(
        [
            "---",
            f"_Generated by plico-benchmarks v{meta.get('plico_version', '0.1.0')}_",
            "",
        ]
    )
    return "\n".join(lines)


class MultiReporter:
    """Generate a combined report without unsupported cross-run inference."""

    def __init__(self, results: list[dict[str, Any]]):
        self.results = results

    def to_markdown(self) -> str:
        lines = [
            "# Plico Benchmark Report",
            "",
            f"> Generated: {time.strftime('%Y-%m-%d %H:%M:%S')}",
        ]
        lines.extend(
            ["", "## 1. Summary", "", self._summary_table(), "", "## 2. Suite Results", ""]
        )

        for result in self.results:
            suite = result.get("metadata", {}).get("suite", "unknown")
            lines.extend([f"### {suite}", ""])
            config = result.get("config", {})
            if config:
                lines.extend(
                    [
                        "#### Run Configuration",
                        "",
                        "| Field | Value |",
                        "|-------|-------|",
                    ]
                )
                for key, value in config.items():
                    rendered = (
                        json.dumps(value, ensure_ascii=False, sort_keys=True)
                        if isinstance(value, (dict, list))
                        else str(value)
                    )
                    lines.append(f"| {key} | {rendered} |")
                lines.append("")

            overall = result.get("metrics", {}).get("overall", {})
            if overall:
                lines.extend(["| Metric | Value |", "|--------|-------|"])
                for key, value in overall.items():
                    lines.append(f"| {key} | {_render_value(value)} |")
                lines.append("")

            _render_ragas_style_proxy(
                lines,
                result.get("metrics", {}),
                "#### RAGAS-style LLM-Judge Proxy Metrics",
            )

        return _RICH_TAG_RE.sub("", "\n".join(lines))

    def save(self, output_dir: Path, filename: str = "benchmark_report.md") -> Path:
        output_dir.mkdir(parents=True, exist_ok=True)
        path = output_dir / filename
        _write_private_atomic(path, self.to_markdown())
        return path

    def _summary_table(self) -> str:
        """Generate a one-row-per-suite summary without cross-protocol baselines."""
        lines = ["| Suite | Key Metric | Value |", "|-------|------------|-------|"]
        for result in self.results:
            suite = result.get("metadata", {}).get("suite", "unknown")
            overall = result.get("metrics", {}).get("overall", {})
            key_metric, key_value = self._pick_key_metric(suite, overall)
            rendered = f"{key_value:.3f}" if isinstance(key_value, float) else str(key_value)
            lines.append(f"| {suite} | {key_metric} | {rendered} |")
        return "\n".join(lines)

    def _pick_key_metric(self, suite: str, overall: dict[str, Any]) -> tuple[str, Any]:
        """Pick the most important metric per suite for the summary."""
        if suite == "conversational-qa":
            return "accuracy_pct", overall.get("accuracy_pct", overall.get("llm_score", 0))
        if suite == "retrieval":
            for dataset in ("beir_scifact", "mab_ar_answer_bearing_retrieval_proxy"):
                if dataset in overall:
                    return f"{dataset}.recall@5", overall[dataset].get("recall@5", 0)
            return "recall@5", 0
        if suite == "performance":
            for operation in ("object.search_warm_repeated", "object.put"):
                if operation in overall:
                    return f"{operation}.p50_ms", overall[operation].get("p50_ms", 0)
            return "p50_ms", 0
        return "value", 0
