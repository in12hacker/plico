"""Dataset base class backed by an explicitly populated local cache."""

from __future__ import annotations

import json
from typing import Any

from plico_benchmarks.core.cache import cache_meta_path, cache_path, is_cached, load_json_cache


class Dataset:
    """Base class for benchmark datasets."""

    name: str = ""
    cache_key: str = ""

    def load(self) -> Any:
        """Load a dataset only from the explicit integrity-checked cache."""
        if is_cached(self.cache_key):
            return load_json_cache(self.cache_key)
        raise FileNotFoundError(
            f"Dataset {self.name} not found in the local cache at {cache_path(self.cache_key)}. "
            "Populate and verify the cache explicitly before running the suite."
        )

    def save_to_cache(self, data: Any) -> None:
        from plico_benchmarks.core.cache import save_cache

        save_cache(self.cache_key, json.dumps(data, ensure_ascii=False))

    def artifact_manifest(self) -> dict[str, Any]:
        """Return the already-verified cache artifact without exposing host paths."""
        if not is_cached(self.cache_key):
            raise FileNotFoundError(f"Missing or invalid cache entry for key: {self.cache_key}")
        path = cache_path(self.cache_key)
        metadata = json.loads(cache_meta_path(self.cache_key).read_text(encoding="utf-8"))
        return {
            "logical_name": self.name,
            "cache_key": self.cache_key,
            "file_name": path.name,
            "bytes": metadata["size"],
            "sha256": metadata["sha256"],
        }
