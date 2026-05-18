"""CLI entry point — typer-based command line interface."""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.table import Table

from plico_benchmarks.core.reporter import MultiReporter, Report

app = typer.Typer(
    name="plico-benchmarks",
    help="Plico AI-OS Kernel — Standardized Benchmark Framework",
    no_args_is_help=True,
)
console = Console()

BENCHMARKS_ROOT = Path(__file__).resolve().parent.parent.parent
RESULTS_DIR = BENCHMARKS_ROOT / "results"


def _list_suites() -> dict[str, str]:
    return {
        "conversational-qa": "LoCoMo + LongMemEval memory QA (incl. temporal, multi-hop)",
        "retrieval": "BEIR + MemoryAgentBench retrieval accuracy",
        "kg-reasoning": "KG multi-hop path finding + reasoning",
        "performance": "CAS, search, recall, KG latency micro-benchmarks",
        "memory-lifecycle": "Memory CRUD + layer migration + checkpoint/restore",
        "token-efficiency": "L0/L1/L2 context layering token savings vs competitors",
        "scope-isolation": "Private/Shared/Group scope enforcement (Axiom 4)",
        "session-lifecycle": "Session start/end, cross-session delta (Axiom 10)",
        "causal-reasoning": "Causal graph chain traversal + retrieval (Axiom 8)",
        "intent-routing": "Intent-aware retrieval routing (Axiom 2)",
        "proactive-optimization": "Context layering, prefetch, pattern detection (Axiom 7)",
    }


@app.command()
def list() -> None:
    """List available benchmark suites."""
    suites = _list_suites()
    table = Table(title="Benchmark Suites")
    table.add_column("Suite", style="cyan", no_wrap=True)
    table.add_column("Description")
    for name, desc in suites.items():
        table.add_row(name, desc)
    console.print(table)


@app.command()
def run(
    suite: str = typer.Argument(..., help="Suite name (use 'list' to see options)"),
    samples: Optional[int] = typer.Option(None, "--samples", "-n", help="Number of samples"),
    embedding: Optional[str] = typer.Option(None, "--embedding", "-e", help="Embedding model name"),
    host: str = typer.Option("127.0.0.1", "--host", "-h", help="plicod host"),
    port: int = typer.Option(7878, "--port", "-p", help="plicod port"),
    output: Optional[Path] = typer.Option(None, "--output", "-o", help="Output JSON path"),
    preprocess_timeout: float = typer.Option(300.0, "--preprocess-timeout", help="Seconds to wait for indexing after ingest"),
) -> None:
    """Run a single benchmark suite."""
    suites = _list_suites()
    if suite not in suites:
        console.print(f"[red]Unknown suite: {suite}[/red]")
        console.print(f"Available: {', '.join(suites.keys())}")
        raise typer.Exit(1)

    # Lazy import to avoid heavy deps at startup
    from plico_benchmarks.suites import SUITE_REGISTRY

    cls = SUITE_REGISTRY.get(suite)
    if cls is None:
        console.print(f"[red]Suite {suite} not yet implemented.[/red]")
        raise typer.Exit(1)

    instance = cls(host=host, port=port, samples=samples)
    console.print(f"[bold green]Running {suite}...[/bold green]")
    try:
        report = instance.execute(preprocess_timeout=preprocess_timeout)
    except Exception as e:
        console.print(f"[red]Benchmark failed: {e}[/red]")
        raise typer.Exit(1)

    version = os.environ.get("PLICO_BENCH_VERSION", "dev")
    out_path = output or RESULTS_DIR / f"{suite.replace('-', '_')}_{version}.json"
    report.save_json(out_path)
    console.print(f"[green]Results saved to {out_path}[/green]")
    console.print(report.to_markdown())


@app.command()
def run_all(
    host: str = typer.Option("127.0.0.1", "--host", "-h"),
    port: int = typer.Option(7878, "--port", "-p"),
    output_dir: Path = typer.Option(RESULTS_DIR, "--output-dir", "-o"),
    preprocess_timeout: float = typer.Option(300.0, "--preprocess-timeout", help="Seconds to wait for indexing after ingest"),
    compare_version: Optional[str] = typer.Option(None, "--compare", help="Version to compare against (loads from output_dir)"),
) -> None:
    """Run all implemented benchmark suites."""
    from plico_benchmarks.suites import SUITE_REGISTRY

    # Load previous results for comparison if specified
    prev_results: list[dict] = []
    if compare_version:
        for path in sorted(output_dir.glob(f"*_{compare_version}.json")):
            try:
                prev_results.append(json.loads(path.read_text(encoding="utf-8")))
                console.print(f"[dim]Loaded baseline: {path.name}[/dim]")
            except Exception:
                pass

    results: list[dict] = []
    for name, cls in SUITE_REGISTRY.items():
        console.print(f"[bold blue]\n{'='*60}[/bold blue]")
        console.print(f"[bold blue]Running {name}...[/bold blue]")
        try:
            instance = cls(host=host, port=port)
            report = instance.execute(preprocess_timeout=preprocess_timeout)
            version = os.environ.get("PLICO_BENCH_VERSION", "dev")
            out_path = output_dir / f"{name.replace('-', '_')}_{version}.json"
            report.save_json(out_path)
            results.append(report.data)
            console.print(f"[green]{name} completed.[/green]")
        except Exception as e:
            console.print(f"[red]{name} failed: {e}[/red]")

    # Combined report with optional comparison
    reporter = MultiReporter(results, prev_results=prev_results)
    version = os.environ.get("PLICO_BENCH_VERSION", "dev")
    report_name = f"benchmark_report_{version}.md"
    md_path = output_dir / report_name
    reporter.save(output_dir, report_name)
    console.print(f"[bold green]\nCombined report saved to {md_path}[/bold green]")


@app.command()
def report(
    input_dir: Path = typer.Option(RESULTS_DIR, "--input", "-i"),
    output: Path = typer.Option(None, "--output", "-o"),
    compare_version: Optional[str] = typer.Option(None, "--compare", help="Version to compare against"),
) -> None:
    """Generate Markdown report from existing JSON results."""
    if output is None:
        version = os.environ.get("PLICO_BENCH_VERSION", "dev")
        output = Path(f"docs/benchmark_report_{version}.md")
    results: list[dict] = []
    version = os.environ.get("PLICO_BENCH_VERSION", "dev")
    for path in sorted(input_dir.glob(f"*_{version}.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            results.append(data)
            console.print(f"[dim]Loaded {path.name}[/dim]")
        except Exception as e:
            console.print(f"[yellow]Skipped {path.name}: {e}[/yellow]")

    if not results:
        console.print("[red]No result files found.[/red]")
        raise typer.Exit(1)

    # Load comparison baseline
    prev_results: list[dict] = []
    if compare_version:
        for path in sorted(input_dir.glob(f"*_{compare_version}.json")):
            try:
                prev_results.append(json.loads(path.read_text(encoding="utf-8")))
                console.print(f"[dim]Loaded baseline: {path.name}[/dim]")
            except Exception:
                pass

    reporter = MultiReporter(results, prev_results=prev_results)
    reporter.save(output.parent, output.name)
    console.print(f"[green]Report saved to {output}[/green]")


@app.command()
def compare(
    baseline: Path = typer.Argument(..., help="Baseline result JSON"),
    current: Path = typer.Argument(..., help="Current result JSON"),
) -> None:
    """Compare two benchmark runs."""
    try:
        base_data = json.loads(baseline.read_text(encoding="utf-8"))
        curr_data = json.loads(current.read_text(encoding="utf-8"))
    except Exception as e:
        console.print(f"[red]Failed to load JSON: {e}[/red]")
        raise typer.Exit(1)

    table = Table(title=f"Comparison: {baseline.name} vs {current.name}")
    table.add_column("Metric")
    table.add_column("Baseline")
    table.add_column("Current")
    table.add_column("Δ")

    base_overall = base_data.get("metrics", {}).get("overall", {})
    curr_overall = curr_data.get("metrics", {}).get("overall", {})

    for key in set(base_overall) | set(curr_overall):
        b = base_overall.get(key, 0)
        c = curr_overall.get(key, 0)
        delta = c - b if isinstance(b, (int, float)) and isinstance(c, (int, float)) else "—"
        color = "green" if isinstance(delta, (int, float)) and delta > 0 else "red" if isinstance(delta, (int, float)) and delta < 0 else "white"
        table.add_row(
            key,
            f"{b:.3f}" if isinstance(b, float) else str(b),
            f"{c:.3f}" if isinstance(c, float) else str(c),
            f"{delta:+.3f}" if isinstance(delta, float) else str(delta),
            style=color,
        )

    console.print(table)


if __name__ == "__main__":
    app()
