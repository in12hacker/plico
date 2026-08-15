"""Exact vector-only test baseline over an injected, deterministic encoder."""

from __future__ import annotations

from collections.abc import Callable

import numpy as np

from plico_benchmarks.baselines.base import SearchResult


class ExactVectorCandidate:
    name = "vector_only"
    domain = "benchmark_text_corpus"

    def __init__(
        self,
        documents: dict[str, str],
        *,
        encoder: Callable[[list[str]], np.ndarray],
        model: str,
    ) -> None:
        if not documents:
            raise ValueError("vector baseline requires at least one document")
        self._document_ids = list(documents)
        self._encode = encoder
        self._model = model
        self._vectors = self._normalized(self._encode(list(documents.values())), len(documents))

    def search(self, query: str, *, limit: int) -> list[SearchResult]:
        query_vector = self._normalized(self._encode([query]), 1)[0]
        scores = self._vectors @ query_vector
        ranked = np.argsort(-scores, kind="stable")[: min(limit, len(self._document_ids))]
        return [
            SearchResult(self._document_ids[int(index)], float(scores[int(index)]))
            for index in ranked
        ]

    def manifest(self) -> dict[str, object]:
        manifest: dict[str, object] = {
            "candidate": self.name,
            "domain": self.domain,
            "implementation": "exact_numpy_cosine",
            "model": self._model,
            "dimension": int(self._vectors.shape[1]),
            "normalization": "l2",
            "ann": False,
        }
        manifest["identity"] = "deterministic_test_fixture"
        return manifest

    @staticmethod
    def _normalized(values: np.ndarray, expected_rows: int) -> np.ndarray:
        array = np.asarray(values, dtype=np.float32)
        if array.ndim != 2 or array.shape[0] != expected_rows or array.shape[1] == 0:
            raise ValueError("vector encoder returned an invalid shape")
        if not np.isfinite(array).all():
            raise ValueError("vector encoder returned a non-finite value")
        norms = np.linalg.norm(array, axis=1, keepdims=True)
        if np.any(norms == 0):
            raise ValueError("vector encoder returned a zero vector")
        return array / norms
