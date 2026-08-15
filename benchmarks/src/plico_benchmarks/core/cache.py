"""Local dataset cache management under ~/.cache/plico-benchmarks/."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import tempfile
from pathlib import Path
from typing import Any

CACHE_ROOT = Path.home() / ".cache" / "plico-benchmarks"


def cache_path(key: str, suffix: str = ".json") -> Path:
    """Return a cache file path for a given key."""
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    safe_key = hashlib.sha256(key.encode()).hexdigest()[:16]
    return CACHE_ROOT / f"{safe_key}{suffix}"


def cache_meta_path(key: str) -> Path:
    return cache_path(key, ".meta.json")


def is_cached(key: str, min_size: int = 0) -> bool:
    """Check a cache entry against its fail-closed size and SHA-256 manifest."""
    path = cache_path(key)
    meta_path = cache_meta_path(key)
    if not path.is_file() or not meta_path.is_file() or path.stat().st_size < min_size:
        return False
    try:
        meta = json.loads(meta_path.read_text(encoding="utf-8"))
        expected_hash = meta["sha256"]
        expected_size = meta["size"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError):
        return False
    if not isinstance(expected_hash, str) or len(expected_hash) != 64:
        return False
    if not isinstance(expected_size, int) or expected_size != path.stat().st_size:
        return False
    return _sha256_file(path) == expected_hash


def save_cache(key: str, data: bytes | str, meta: dict[str, Any] | None = None) -> Path:
    """Save data and its integrity manifest atomically with owner-only mode."""
    path = cache_path(key)
    CACHE_ROOT.mkdir(parents=True, exist_ok=True)
    payload = data.encode("utf-8") if isinstance(data, str) else data
    _write_private_atomic(path, payload)
    meta_path = cache_meta_path(key)
    meta_data = dict(meta or {})
    meta_data["size"] = len(payload)
    meta_data["sha256"] = hashlib.sha256(payload).hexdigest()
    _write_private_atomic(meta_path, json.dumps(meta_data).encode("utf-8"))
    return path


def load_json_cache(key: str) -> Any:
    """Load a JSON cache entry only after manifest verification."""
    path = cache_path(key)
    if not is_cached(key):
        raise FileNotFoundError(f"Missing or invalid cache entry for key: {key}")
    return json.loads(path.read_text(encoding="utf-8"))


def clear_cache() -> None:
    """Remove all cached files."""
    if CACHE_ROOT.exists():
        shutil.rmtree(CACHE_ROOT)


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _write_private_atomic(path: Path, payload: bytes) -> None:
    fd, temporary_name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    temporary = Path(temporary_name)
    try:
        with os.fdopen(fd, "wb") as stream:
            stream.write(payload)
            stream.flush()
            os.fsync(stream.fileno())
        temporary.chmod(0o600)
        os.replace(temporary, path)
        path.chmod(0o600)
    finally:
        temporary.unlink(missing_ok=True)
