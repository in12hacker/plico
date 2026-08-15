"""BEIR SciFact dataset cache descriptor."""

from __future__ import annotations

from plico_benchmarks.datasets.base import Dataset


class BeirDataset(Dataset):
    name = "beir_scifact"
    cache_key = "beir"
