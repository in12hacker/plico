"""CLI entry point — typer-based command line interface."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Optional

import typer
from rich.console import Console
from rich.table import Table

from plico_benchmarks.core.reporter import MultiReporter

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
        "memory-recall-lexical": "Canonical Working Memory lexical recall quality",
        "performance": "Typed object, memory, session, and readiness E2E latency",
        "v1b-release": "V1-B canonical ledger, restart, policy, migration release evidence",
    }


@app.command("list")
def list_suites() -> None:
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
    host: str = typer.Option("127.0.0.1", "--host", "-h", help="plicod host"),
    port: int = typer.Option(7878, "--port", "-p", help="plicod port"),
    uds: Optional[Path] = typer.Option(None, "--uds", help="plicod Unix socket"),
    output: Optional[Path] = typer.Option(None, "--output", "-o", help="New result directory"),
    preprocess_timeout: float = typer.Option(
        300.0, "--preprocess-timeout", help="Seconds to wait for indexing after ingest"
    ),
    seed: int = typer.Option(42, "--seed", help="Deterministic sampling seed", envvar="PLICO_SEED"),
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
        console.print(f"[red]Suite {suite} is not registered.[/red]")
        raise typer.Exit(1)

    instance = cls(
        host=host,
        port=port,
        uds_path=str(uds) if uds is not None else None,
        samples=samples,
        seed=seed,
    )
    console.print(f"[bold green]Running {suite}...[/bold green]")
    try:
        report = instance.execute(preprocess_timeout=preprocess_timeout)
    except Exception as e:
        console.print(f"[red]Benchmark failed: {e}[/red]")
        raise typer.Exit(1)

    out_path = output or RESULTS_DIR / report.data["run_manifest"]["run_id"]
    report.commit_result(out_path)
    console.print(f"[green]Results saved to {out_path}[/green]")
    console.print(report.to_markdown())


@app.command()
def report(
    result_directories: list[Path] = typer.Option(
        ..., "--result-dir", help="Explicit committed suite result directory"
    ),
    output: Path = typer.Option(None, "--output", "-o"),
) -> None:
    """Generate Markdown report from existing JSON results."""
    if output is None:
        version = os.environ.get("PLICO_BENCH_VERSION", "dev")
        output = Path(f"docs/benchmark_report_{version}.md")
    from plico_benchmarks.core.result_artifact import verify_result_directory

    results: list[dict] = []
    seen_runs = set()
    for path in result_directories:
        try:
            data = verify_result_directory(path)
            run_id = data["run_manifest"]["run_id"]
            if run_id in seen_runs:
                raise ValueError("report input repeats one run identity")
            seen_runs.add(run_id)
            results.append(data)
            console.print(f"[dim]Loaded {path.name}[/dim]")
        except Exception as e:
            console.print(f"[red]Invalid result artifact {path.name}: {e}[/red]")
            raise typer.Exit(1) from e

    reporter = MultiReporter(results)
    reporter.save(output.parent, output.name)
    console.print(f"[green]Report saved to {output}[/green]")


@app.command("compare-shadow")
def compare_shadow(
    results: list[Path] = typer.Option(..., "--result", help="Exactly five explicit result files"),
    candidate: str = typer.Option(..., "--candidate"),
    reference: str = typer.Option(..., "--reference"),
    metric: str = typer.Option("recall@10", "--metric"),
    output: Path = typer.Option(..., "--output"),
    seed: int = typer.Option(42, "--seed"),
) -> None:
    """Build an exploratory five-run paired retrieval comparison."""
    from plico_benchmarks.core.comparison import (
        commit_shadow_directory,
        compare_retrieval_shadow,
        load_result,
    )

    try:
        comparison = compare_retrieval_shadow(
            [load_result(path) for path in results],
            candidate=candidate,
            reference=reference,
            metric=metric,
            seed=seed,
        )
        commit_shadow_directory(output, comparison)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]Shadow comparison rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print(f"[green]Shadow comparison saved to {output}[/green]")


@app.command("bundle-v1b")
def bundle_v1b(
    benchmark_result: Path = typer.Option(..., "--benchmark-result"),
    dogfood_bundle: Path = typer.Option(..., "--dogfood-bundle"),
    output_directory: Path = typer.Option(..., "--output-dir"),
) -> None:
    """Bind a completed V1-B benchmark and dogfood artifact without merging scores."""
    from plico_benchmarks.core.release_bundle import build_v1b_release_bundle

    try:
        bundle = build_v1b_release_bundle(
            benchmark_result=benchmark_result,
            dogfood_bundle=dogfood_bundle,
            output=output_directory,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]V1-B bundle rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print(f"[green]V1-B release bundle {bundle['bundle_run_id']} committed[/green]")


@app.command("dogfood-evidence")
def dogfood_evidence(
    capture: Path = typer.Option(..., "--capture"),
    plicod_binary: Path = typer.Option(..., "--plicod-binary"),
    uds_socket: Path = typer.Option(..., "--uds-socket"),
    plico_root: Path = typer.Option(..., "--plico-root"),
    plico_agents_root: Path = typer.Option(..., "--plico-agents-root"),
    uv_lock: Path = typer.Option(..., "--uv-lock"),
    daemon_trace: Path = typer.Option(..., "--daemon-trace"),
    reader_trace: Path = typer.Option(..., "--reader-trace"),
    canonical_before_rebuild: Path = typer.Option(..., "--canonical-before-rebuild"),
    canonical_after_rebuild: Path = typer.Option(..., "--canonical-after-rebuild"),
    canonical_before_restart: Path = typer.Option(..., "--canonical-before-restart"),
    canonical_after_restart: Path = typer.Option(..., "--canonical-after-restart"),
    canary: Path = typer.Option(..., "--canary"),
    ollama_probe: Path = typer.Option(..., "--ollama-probe"),
    canonical_vault: Path = typer.Option(..., "--canonical-vault"),
    v1_zero_before: Path = typer.Option(..., "--v1-zero-before"),
    v1_zero_after: Path = typer.Option(..., "--v1-zero-after"),
    output_directory: Path = typer.Option(..., "--output-dir"),
) -> None:
    """Build deterministic P3-A evidence from typed capture and concrete artifacts."""
    from plico_benchmarks.core.dogfood_artifacts import ArtifactInputs
    from plico_benchmarks.core.dogfood_evidence import generate_dogfood_evidence

    try:
        evidence = generate_dogfood_evidence(
            capture_path=capture,
            inputs=ArtifactInputs(
                plicod_binary=plicod_binary,
                uds_socket=uds_socket,
                plico_root=plico_root,
                plico_agents_root=plico_agents_root,
                uv_lock=uv_lock,
                daemon_trace=daemon_trace,
                reader_trace=reader_trace,
                canonical_before_rebuild=canonical_before_rebuild,
                canonical_after_rebuild=canonical_after_rebuild,
                canonical_before_restart=canonical_before_restart,
                canonical_after_restart=canonical_after_restart,
                canary=canary,
                ollama_probe=ollama_probe,
                canonical_vault=canonical_vault,
                v1_zero_before=v1_zero_before,
                v1_zero_after=v1_zero_after,
            ),
            output_directory=output_directory,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]P3-A dogfood capture rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print(f"[green]P3-A dogfood evidence {evidence.bundle_run_id} committed[/green]")


@app.command("verify-dogfood-evidence")
def verify_dogfood_evidence_command(
    artifact_directory: Path = typer.Option(..., "--artifact-dir"),
) -> None:
    """Deep-verify one P3-A evidence artifact and its detached digest."""
    from plico_benchmarks.core.dogfood_evidence import verify_dogfood_evidence

    try:
        evidence = verify_dogfood_evidence(artifact_directory)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]P3-A dogfood evidence rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print(f"[green]P3-A dogfood evidence {evidence.bundle_run_id} verified[/green]")


@app.command("collect-canonical-checkpoint")
def collect_canonical_checkpoint_command(
    vault: Path = typer.Option(..., "--vault"),
    output: Path = typer.Option(..., "--output"),
    bundle_run_id: str = typer.Option(..., "--bundle-run-id"),
    phase: str = typer.Option(..., "--phase"),
    daemon_instance_id: str = typer.Option(..., "--daemon-instance-id"),
    sequence: int = typer.Option(..., "--sequence"),
) -> None:
    """Collect one owner-only canonical memory-ledger checkpoint."""
    from plico_benchmarks.core.dogfood_collectors import collect_canonical_checkpoint

    try:
        collect_canonical_checkpoint(
            vault=vault,
            output=output,
            bundle_run_id=bundle_run_id,
            phase=phase,
            daemon_instance_id=daemon_instance_id,
            sequence=sequence,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]Canonical checkpoint rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print("[green]Canonical checkpoint collected[/green]")


@app.command("collect-ollama-probe")
def collect_ollama_probe_command(
    base_url: str = typer.Option(..., "--base-url"),
    configured_tag: str = typer.Option(..., "--configured-tag"),
    output: Path = typer.Option(..., "--output"),
    requested_target_dimension: Optional[int] = typer.Option(None, "--target-dimension"),
    adaptive_prefix_contract_id: str = typer.Option(
        "provider-native-input-v1", "--adaptive-prefix-contract"
    ),
) -> None:
    """Collect redacted exact-tag Ollama identity and embedding-shape evidence."""
    from plico_benchmarks.core.dogfood_collectors import collect_ollama_probe

    try:
        collect_ollama_probe(
            base_url=base_url,
            configured_tag=configured_tag,
            output=output,
            requested_target_dimension=requested_target_dimension,
            adaptive_prefix_contract_id=adaptive_prefix_contract_id,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]Ollama probe rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print("[green]Ollama probe collected[/green]")


@app.command("collect-v1-zero-state")
def collect_v1_zero_state_command(
    vault: Path = typer.Option(..., "--vault"),
    output: Path = typer.Option(..., "--output"),
    bundle_run_id: str = typer.Option(..., "--bundle-run-id"),
    phase: str = typer.Option(..., "--phase"),
    daemon_instance_id: str = typer.Option(..., "--daemon-instance-id"),
    sequence: int = typer.Option(..., "--sequence"),
) -> None:
    """Collect canonical plus projection zero-state around a rejected v1 request."""
    from plico_benchmarks.core.dogfood_collectors import collect_v1_zero_state_checkpoint

    try:
        collect_v1_zero_state_checkpoint(
            vault=vault,
            output=output,
            bundle_run_id=bundle_run_id,
            phase=phase,
            daemon_instance_id=daemon_instance_id,
            sequence=sequence,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        console.print(f"[red]V1 zero-state checkpoint rejected: {error}[/red]")
        raise typer.Exit(1) from error
    console.print("[green]V1 zero-state checkpoint collected[/green]")


if __name__ == "__main__":
    app()
