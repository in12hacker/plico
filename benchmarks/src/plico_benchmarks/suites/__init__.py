"""Benchmark suites registry."""

from __future__ import annotations

from plico_benchmarks.suites.conversational_qa import ConversationalQASuite
from plico_benchmarks.suites.memory_recall import MemoryRecallLexicalSuite
from plico_benchmarks.suites.performance import PerformanceSuite
from plico_benchmarks.suites.retrieval import RetrievalSuite
from plico_benchmarks.suites.v1b_release import V1BReleaseSuite

SUITE_REGISTRY: dict[str, type] = {
    "conversational-qa": ConversationalQASuite,
    "memory-recall-lexical": MemoryRecallLexicalSuite,
    "retrieval": RetrievalSuite,
    "performance": PerformanceSuite,
    "v1b-release": V1BReleaseSuite,
}
