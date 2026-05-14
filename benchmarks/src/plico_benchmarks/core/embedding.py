"""Embedding provider abstraction — OpenAI-compatible / HF / llama.cpp."""

from __future__ import annotations

import os
from typing import Any, Protocol

import numpy as np
import requests


class EmbeddingProvider(Protocol):
    """Abstract embedding provider."""

    def embed(self, texts: list[str]) -> list[list[float]]:
        ...

    def embed_query(self, text: str) -> list[float]:
        ...

    def is_available(self) -> bool:
        ...

    @property
    def dims(self) -> int:
        ...


class OpenAiCompatibleEmbedding:
    """OpenAI-compatible embedding endpoint (llama.cpp, jina, etc.)."""

    def __init__(
        self,
        api_base: str | None = None,
        model: str = "default",
        timeout: float = 60.0,
    ):
        self.api_base = (
            api_base or os.environ.get("EMBEDDING_API_BASE", "http://127.0.0.1:8080/v1")
        ).rstrip("/")
        self.model = model or os.environ.get("EMBEDDING_MODEL", "default")
        self.timeout = timeout
        self._dims: int | None = None

    def embed(self, texts: list[str]) -> list[list[float]]:
        url = f"{self.api_base}/embeddings"
        payload = {"model": self.model, "input": texts}
        resp = requests.post(url, json=payload, timeout=self.timeout)
        resp.raise_for_status()
        data = resp.json()["data"]
        vectors = [item["embedding"] for item in data]
        if vectors:
            self._dims = len(vectors[0])
        return vectors

    def embed_query(self, text: str) -> list[float]:
        return self.embed([text])[0]

    def is_available(self) -> bool:
        try:
            r = requests.get(f"{self.api_base}/models", timeout=5)
            return r.status_code == 200
        except Exception:
            return False

    @property
    def dims(self) -> int:
        if self._dims is None:
            self._dims = len(self.embed_query("test"))
        return self._dims


class HuggingFaceEmbedding:
    """Local HuggingFace embedding via sentence-transformers."""

    def __init__(self, model_name: str = "BAAI/bge-m3", device: str = "cpu"):
        self.model_name = model_name
        self.device = device
        self._model: Any | None = None
        self._dims: int | None = None

    def _load(self) -> Any:
        if self._model is None:
            from sentence_transformers import SentenceTransformer
            self._model = SentenceTransformer(self.model_name, device=self.device)
        return self._model

    def embed(self, texts: list[str]) -> list[list[float]]:
        model = self._load()
        vectors = model.encode(texts, normalize_embeddings=True)
        return vectors.tolist()

    def embed_query(self, text: str) -> list[float]:
        return self.embed([text])[0]

    def is_available(self) -> bool:
        try:
            self._load()
            return True
        except Exception:
            return False

    @property
    def dims(self) -> int:
        if self._dims is None:
            self._dims = len(self.embed_query("test"))
        return self._dims


class StubEmbedding:
    """Stub embedding for testing — returns random vectors."""

    def __init__(self, dims: int = 768, seed: int = 42):
        self._dims = dims
        self.rng = np.random.default_rng(seed)

    def embed(self, texts: list[str]) -> list[list[float]]:
        vecs = self.rng.random((len(texts), self._dims)).astype(float).tolist()
        # normalize
        return [list(v / np.linalg.norm(v)) for v in vecs]

    def embed_query(self, text: str) -> list[float]:
        return self.embed([text])[0]

    def is_available(self) -> bool:
        return True

    @property
    def dims(self) -> int:
        return self._dims


def default_embedding_provider(name: str | None = None) -> EmbeddingProvider:
    """Factory — create embedding provider by name or env."""
    from plico_benchmarks.core.config import get_config

    cfg = get_config()
    model_cfg = cfg.get_embedding_model(name) if name else cfg.default_embedding_model()
    provider = model_cfg.get("provider", "openai-compatible")

    if provider == "huggingface":
        return HuggingFaceEmbedding(model_name=model_cfg.get("repo", "BAAI/bge-m3"))
    if provider == "openai-compatible":
        return OpenAiCompatibleEmbedding(model=model_cfg.get("name", "default"))
    if provider == "llama.cpp":
        return OpenAiCompatibleEmbedding(
            api_base=os.environ.get("EMBEDDING_API_BASE", "http://127.0.0.1:8080/v1"),
            model=model_cfg.get("name", "default"),
        )
    if provider == "stub":
        return StubEmbedding(dims=model_cfg.get("dims", 768))
    return OpenAiCompatibleEmbedding()
