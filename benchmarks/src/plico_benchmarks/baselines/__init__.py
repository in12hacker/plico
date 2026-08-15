"""Evaluation-only retrieval baselines; never part of the Plico runtime."""

from plico_benchmarks.baselines.base import RetrievalCandidate, SearchResult
from plico_benchmarks.baselines.bm25 import Bm25Candidate
from plico_benchmarks.baselines.vector import ExactVectorCandidate

__all__ = [
    "Bm25Candidate",
    "ExactVectorCandidate",
    "RetrievalCandidate",
    "SearchResult",
]
