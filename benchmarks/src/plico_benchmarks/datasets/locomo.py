"""LoCoMo dataset cache descriptor."""

from __future__ import annotations

from plico_benchmarks.datasets.base import Dataset


class LoCoMoDataset(Dataset):
    name = "locomo"
    cache_key = "locomo10.json"
