"""Dataset cache management — auto-download to ~/.cache/plico-benchmarks/."""

from __future__ import annotations

import hashlib
import json
import shutil
from pathlib import Path
from typing import Any

import requests

CACHE_ROOT = Path.home() / ".cache" / "plico-benchmarks"


def cache_path(key: str, suffix: str = ".json") -> Path:
    """Return a cache file path for a given key."""
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    safe_key = hashlib.sha256(key.encode()).hexdigest()[:16]
    return CACHE_ROOT / f"{safe_key}{suffix}"


def cache_meta_path(key: str) -> Path:
    return cache_path(key, ".meta.json")


def is_cached(key: str, min_size: int = 0) -> bool:
    """Check if a file is cached and valid."""
    path = cache_path(key)
    meta_path = cache_meta_path(key)
    if not path.exists() or path.stat().st_size < min_size:
        return False
    if meta_path.exists():
        try:
            meta = json.loads(meta_path.read_text())
            if meta.get("size") != path.stat().st_size:
                return False
        except Exception:
            pass
    return True


def save_cache(key: str, data: bytes | str, meta: dict[str, Any] | None = None) -> Path:
    """Save data to cache."""
    path = cache_path(key)
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    if isinstance(data, str):
        path.write_text(data, encoding="utf-8")
    else:
        path.write_bytes(data)
    meta_path = cache_meta_path(key)
    meta_data = meta or {}
    meta_data["size"] = path.stat().st_size
    meta_path.write_text(json.dumps(meta_data), encoding="utf-8")
    return path


def download(url: str, key: str | None = None, chunk_size: int = 8192) -> Path:
    """Download a URL to cache if not already present."""
    key = key or url
    path = cache_path(key)
    if is_cached(key):
        return path
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    with requests.get(url, stream=True, timeout=120) as r:
        r.raise_for_status()
        total = int(r.headers.get("content-length", 0))
        with open(path, "wb") as f:
            downloaded = 0
            for chunk in r.iter_content(chunk_size=chunk_size):
                if chunk:
                    f.write(chunk)
                    downloaded += len(chunk)
    save_cache(key, b"", meta={"url": url, "size": path.stat().st_size})
    return path


def load_json_cache(key: str) -> Any:
    """Load a JSON file from cache."""
    path = cache_path(key)
    if not path.exists():
        raise FileNotFoundError(f"Cache miss for key: {key}")
    return json.loads(path.read_text(encoding="utf-8"))


def clear_cache() -> None:
    """Remove all cached files."""
    if CACHE_ROOT.exists():
        shutil.rmtree(CACHE_ROOT)
