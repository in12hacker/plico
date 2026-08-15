"""BM25 baseline backed exclusively by the maintained bm25s package."""

from __future__ import annotations

import math
from typing import Any

import bm25s

from plico_benchmarks.baselines.base import SearchResult


class Bm25Candidate:
    name = "bm25_only"
    domain = "benchmark_text_corpus"

    def __init__(
        self,
        documents: dict[str, str],
        *,
        method: str,
        k1: float,
        b: float,
        tokenizer_contract: str,
        lower: bool,
        token_pattern: str,
        stopwords: str,
        stemmer: str,
    ) -> None:
        if not documents:
            raise ValueError("BM25 baseline requires at least one document")
        if (
            method != "lucene"
            or tokenizer_contract != "bm25s_regex_words_v1"
            or lower is not True
            or token_pattern != r"(?u)\b\w\w+\b"
            or stemmer != "none"
            or not math.isfinite(k1)
            or not math.isfinite(b)
            or k1 <= 0
            or not 0 <= b <= 1
        ):
            raise ValueError("unsupported BM25 tokenizer contract")
        self._document_ids = list(documents)
        self._method = method
        self._k1 = float(k1)
        self._b = float(b)
        self._token_pattern = token_pattern
        self._stopwords = stopwords
        self._retriever = bm25s.BM25(k1=self._k1, b=self._b, method=method)
        tokens = self._tokenize(list(documents.values()))
        self._retriever.index(tokens, show_progress=False)

    @classmethod
    def from_config(cls, documents: dict[str, str], config: dict[str, Any]) -> Bm25Candidate:
        if config.get("implementation") != "bm25s" or config.get("version") != bm25s.__version__:
            raise ValueError("BM25 implementation/version differs from the pinned runtime")
        tokenizer = config.get("tokenizer")
        if not isinstance(tokenizer, dict):
            raise ValueError("BM25 tokenizer configuration is missing")
        return cls(
            documents,
            method=str(config["method"]),
            k1=float(config["k1"]),
            b=float(config["b"]),
            tokenizer_contract=str(tokenizer["contract"]),
            lower=tokenizer["lower"],
            token_pattern=str(tokenizer["token_pattern"]),
            stopwords=str(tokenizer["stopwords"]),
            stemmer=str(tokenizer["stemmer"]),
        )

    def search(self, query: str, *, limit: int) -> list[SearchResult]:
        if limit <= 0:
            raise ValueError("BM25 retrieval limit must be positive")
        documents, scores = self._retriever.retrieve(
            self._tokenize([query]),
            corpus=self._document_ids,
            k=min(limit, len(self._document_ids)),
            show_progress=False,
        )
        results = [
            SearchResult(str(document_id), float(score))
            for document_id, score in zip(documents[0], scores[0], strict=True)
        ]
        return sorted(results, key=lambda result: (-result.score, result.document_id))

    def manifest(self) -> dict[str, object]:
        return {
            "candidate": self.name,
            "domain": self.domain,
            "implementation": "bm25s",
            "version": bm25s.__version__,
            "method": self._method,
            "k1": self._k1,
            "b": self._b,
            "tokenizer": {
                "contract": "bm25s_regex_words_v1",
                "lower": True,
                "token_pattern": self._token_pattern,
                "stopwords": self._stopwords,
                "stemmer": "none",
            },
        }

    def _tokenize(self, texts: list[str]):
        return bm25s.tokenize(
            texts,
            lower=True,
            token_pattern=self._token_pattern,
            stopwords=self._stopwords,
            stemmer=None,
            show_progress=False,
        )
