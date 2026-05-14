"""CLI entry point — typer-based command line interface."""

from __future__ import annotations

import json
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
        "conversational-qa": "LoCoMo + LongMemEval conversational memory QA",
        "retrieval": "BEIR + MemoryAgentBench AR + DMR retrieval accuracy",
        "kg-reasoning": "HotPotQA + KG multi-hop reasoning",
        "performance": "CAS, search, memory, KG micro-benchmarks",
        "temporal-reasoning": "Temporal reasoning evaluation (skeleton)",
        "memory-crud": "Memory CRUD correctness (skeleton)",
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

    out_path = output or RESULTS_DIR / f"{suite.replace('-', '_')}_v44.json"
    report.save_json(out_path)
    console.print(f"[green]Results saved to {out_path}[/green]")
    console.print(report.to_markdown())


@app.command()
def run_all(
    host: str = typer.Option("127.0.0.1", "--host", "-h"),
    port: int = typer.Option(7878, "--port", "-p"),
    output_dir: Path = typer.Option(RESULTS_DIR, "--output-dir", "-o"),
    preprocess_timeout: float = typer.Option(300.0, "--preprocess-timeout", help="Seconds to wait for indexing after ingest"),
) -> None:
    """Run all implemented benchmark suites."""
    from plico_benchmarks.suites import SUITE_REGISTRY

    results: list[dict] = []
    for name, cls in SUITE_REGISTRY.items():
        console.print(f"[bold blue]\n{'='*60}[/bold blue]")
        console.print(f"[bold blue]Running {name}...[/bold blue]")
        try:
            instance = cls(host=host, port=port)
            report = instance.execute(preprocess_timeout=preprocess_timeout)
            out_path = output_dir / f"{name.replace('-', '_')}_v44.json"
            report.save_json(out_path)
            results.append(report.data)
            console.print(f"[green]{name} completed.[/green]")
        except Exception as e:
            console.print(f"[red]{name} failed: {e}[/red]")

    # Combined report
    reporter = MultiReporter(results)
    md_path = output_dir / "benchmark_report_v44.md"
    reporter.save(output_dir, "benchmark_report_v44.md")
    console.print(f"[bold green]\nCombined report saved to {md_path}[/bold green]")


@app.command()
def report(
    input_dir: Path = typer.Option(RESULTS_DIR, "--input", "-i"),
    output: Path = typer.Option(Path("docs/benchmark_report_v44.md"), "--output", "-o"),
) -> None:
    """Generate Markdown report from existing JSON results."""
    results: list[dict] = []
    for path in sorted(input_dir.glob("*_v44.json")):
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
            results.append(data)
            console.print(f"[dim]Loaded {path.name}[/dim]")
        except Exception as e:
            console.print(f"[yellow]Skipped {path.name}: {e}[/yellow]")

    if not results:
        console.print("[red]No result files found.[/red]")
        raise typer.Exit(1)

    reporter = MultiReporter(results)
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
