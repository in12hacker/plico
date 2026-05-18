"""Benchmark suites registry."""

from __future__ import annotations

from plico_benchmarks.suites.causal_reasoning import CausalReasoningSuite
from plico_benchmarks.suites.conversational_qa import ConversationalQASuite
from plico_benchmarks.suites.intent_routing import IntentRoutingSuite
from plico_benchmarks.suites.kg_reasoning import KGReasoningSuite
from plico_benchmarks.suites.memory_lifecycle import MemoryLifecycleSuite
from plico_benchmarks.suites.performance import PerformanceSuite
from plico_benchmarks.suites.proactive_optimization import ProactiveOptimizationSuite
from plico_benchmarks.suites.retrieval import RetrievalSuite
from plico_benchmarks.suites.scope_isolation import ScopeIsolationSuite
from plico_benchmarks.suites.session_lifecycle import SessionLifecycleSuite
from plico_benchmarks.suites.token_efficiency import TokenEfficiencySuite

SUITE_REGISTRY: dict[str, type] = {
    "conversational-qa": ConversationalQASuite,
    "retrieval": RetrievalSuite,
    "kg-reasoning": KGReasoningSuite,
    "performance": PerformanceSuite,
    "memory-lifecycle": MemoryLifecycleSuite,
    "token-efficiency": TokenEfficiencySuite,
    "scope-isolation": ScopeIsolationSuite,
    "session-lifecycle": SessionLifecycleSuite,
    "causal-reasoning": CausalReasoningSuite,
    "intent-routing": IntentRoutingSuite,
    "proactive-optimization": ProactiveOptimizationSuite,
}
