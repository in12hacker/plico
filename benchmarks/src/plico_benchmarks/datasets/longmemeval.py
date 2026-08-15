"""LongMemEval dataset cache descriptor."""

from __future__ import annotations

from plico_benchmarks.datasets.base import Dataset


class LongMemEvalDataset(Dataset):
    name = "longmemeval"
    cache_key = "longmemeval_s_cleaned.json"
