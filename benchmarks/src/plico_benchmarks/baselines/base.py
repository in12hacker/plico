"""Common typed boundary for benchmark-only retrieval candidates."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Protocol


@dataclass(frozen=True)
class SearchResult:
    document_id: str
    score: float


class RetrievalCandidate(Protocol):
    name: str
    domain: str

    def search(self, query: str, *, limit: int) -> list[SearchResult]: ...

    def manifest(self) -> dict[str, object]: ...
