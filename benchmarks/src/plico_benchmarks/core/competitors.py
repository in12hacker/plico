"""Competitor baseline loader — reads from configs/competitor_baselines.yaml."""

from __future__ import annotations

from pathlib import Path
from typing import Any


_CACHE: dict[str, Any] | None = None


def load_baselines() -> dict[str, Any]:
    """Load competitor baselines from YAML. Cached after first load."""
    global _CACHE
    if _CACHE is not None:
        return _CACHE
    # Navigate from core/ → plico_benchmarks/ → src/ → benchmarks/ → configs/
    config_path = Path(__file__).resolve().parent.parent.parent.parent / "configs" / "competitor_baselines.yaml"
    if not config_path.exists():
        _CACHE = {}
        return _CACHE
    try:
        import yaml
        with open(config_path, "r", encoding="utf-8") as f:
            _CACHE = yaml.safe_load(f) or {}
    except ImportError:
        # Fallback: parse minimal YAML manually for key values
        _CACHE = _parse_yaml_minimal(config_path)
    except Exception:
        _CACHE = {}
    return _CACHE


def get_memory_competitors(benchmark: str = "longmemeval") -> list[dict[str, Any]]:
    """Get competitor data for memory benchmarks (longmemeval or locomo)."""
    baselines = load_baselines()
    return baselines.get("memory", {}).get(benchmark, {}).get("competitors", [])


def get_retrieval_competitors() -> list[dict[str, Any]]:
    """Get MTEB embedding model competitor data."""
    baselines = load_baselines()
    return baselines.get("retrieval", {}).get("mteb", {}).get("competitors", [])


def get_token_efficiency_competitors() -> list[dict[str, Any]]:
    """Get token efficiency competitor data."""
    baselines = load_baselines()
    return baselines.get("memory", {}).get("token_efficiency", {}).get("competitors", [])


def get_agent_frameworks() -> list[dict[str, Any]]:
    """Get agent framework feature comparison data."""
    baselines = load_baselines()
    return baselines.get("agent_frameworks", {}).get("competitors", [])


def get_plico_state() -> dict[str, Any]:
    """Get Plico's current state from baselines."""
    baselines = load_baselines()
    return baselines.get("plico", {})


def get_ragas_baselines() -> dict[str, Any]:
    """Get RAGAS production baseline metrics."""
    baselines = load_baselines()
    return baselines.get("rag_quality", {})


def get_cross_benchmarks() -> dict[str, Any]:
    """Get cross-cutting benchmark references (HotpotQA, AgentBench, BigBench-Hard)."""
    baselines = load_baselines()
    return baselines.get("cross_benchmarks", {})


def _parse_yaml_minimal(path: Path) -> dict[str, Any]:
    """Minimal YAML parser for when PyYAML is not available."""
    # This is a fallback — real usage should have PyYAML
    return {}
