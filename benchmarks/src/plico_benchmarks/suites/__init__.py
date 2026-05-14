"""Benchmark suites registry."""

from __future__ import annotations

from plico_benchmarks.suites.conversational_qa import ConversationalQASuite
from plico_benchmarks.suites.kg_reasoning import KGReasoningSuite
from plico_benchmarks.suites.memory_crud import MemoryCrudSuite
from plico_benchmarks.suites.performance import PerformanceSuite
from plico_benchmarks.suites.retrieval import RetrievalSuite
from plico_benchmarks.suites.temporal_reasoning import TemporalReasoningSuite

SUITE_REGISTRY: dict[str, type] = {
    "conversational-qa": ConversationalQASuite,
    "retrieval": RetrievalSuite,
    "kg-reasoning": KGReasoningSuite,
    "performance": PerformanceSuite,
    "temporal-reasoning": TemporalReasoningSuite,
    "memory-crud": MemoryCrudSuite,
}
