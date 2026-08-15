"""MemoryAgentBench AR dataset cache descriptor."""

from __future__ import annotations

from plico_benchmarks.datasets.base import Dataset


class MABDataset(Dataset):
    name = "memoryagentbench_ar"
    cache_key = "memoryagentbench_ar.json"
