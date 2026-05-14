"""Configuration management — load YAML configs and env overrides."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

import yaml

CONFIG_DIR = Path(__file__).resolve().parent.parent.parent / "configs"


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
        self.embedding = load_yaml("embedding_models")
        self.judge = load_yaml("judge_prompts")
        self.benchmark = load_yaml("benchmark")

    def default_embedding_model(self) -> dict[str, Any]:
        models = self.embedding.get("models", [])
        default_name = self.embedding.get("default", "")
        for m in models:
            if m.get("name") == default_name:
                return m
        # fallback to first recommended
        for m in models:
            if m.get("recommended"):
                return m
        return models[0] if models else {}

    def get_embedding_model(self, name: str) -> dict[str, Any] | None:
        for m in self.embedding.get("models", []):
            if m.get("name") == name:
                return m
        return None

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
