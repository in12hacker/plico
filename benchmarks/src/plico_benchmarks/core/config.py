"""Configuration management — load YAML configs and env overrides."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import yaml

CONFIG_DIR = Path(__file__).resolve().parent.parent.parent.parent / "configs"


def load_yaml(name: str) -> dict[str, Any]:
    """Load a YAML config file from configs/."""
    path = CONFIG_DIR / f"{name}.yaml"
    if not path.exists():
        raise FileNotFoundError(f"Config not found: {path}")
    with open(path, encoding="utf-8") as f:
        return yaml.safe_load(f) or {}


def get_env_or_default(key: str, default: str) -> str:
    return os.environ.get(key, default)


class BenchmarkConfig:
    """Unified benchmark configuration."""

    def __init__(self) -> None:
        self.judge = load_yaml("judge_prompts")
        self.benchmark = load_yaml("benchmark")

    def judge_prompt_for(self, dataset: str, prompt_type: str = "default") -> str:
        prompts = self.judge.get("prompts", {})
        ds_prompts = prompts.get(dataset, {})
        return ds_prompts.get(prompt_type, prompts.get("default", {}))


# Global singleton
_config: BenchmarkConfig | None = None


def get_config() -> BenchmarkConfig:
    global _config
    if _config is None:
        _config = BenchmarkConfig()
    return _config
