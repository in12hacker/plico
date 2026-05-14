# Plico v41 Milestone Plan — Asynchronous Cognitive Symbiont

> **Status**: In Progress
> **Target Version**: v41.0
> **Parent Vision**: [Soul 3.0 (system-v3.md)](../../system-v3.md)
> **Baseline**: v40 (Audit Report 2026-05-09)

## 0. Core Objectives

1.  **Maximize Throughput**: Unblock the `store` (CAS write) path by moving all high-latency cognitive processing to a decoupled background pipeline.
2.  **Self-Healing (Explanatory Recovery)**: Enable the kernel to report internal processing failures to the Agent with actionable recovery suggestions.
3.  **Intelligent Evolution**: Shift from mechanical "hit-counters" to semantic-driven skill extraction recommendations.
4.  **Harness-Ready**: Fully support the 11 polymorphic core verbs as the primary interface.

---

## 1. Milestones & Deliverables

### Milestone 1: [ACP-01] Async Cognitive Pipeline (ACP) 核心构建
*   **Goal**: Decouple cognitive tasks from the main API thread.
*   **Deliverables**:
    *   `src/kernel/ops/cognitive_pipeline.rs`: A DAG-aware task scheduler for cognitive jobs.
    *   Integration: Move `summarize`, `kg_extract`, and `add_similar_to_edges` into the ACP.
    *   Dependency Management: Ensure tasks like `chunking -> embedding` maintain correct order.
*   **Verification**:
    *   `cas_write` QPS returns to >500 (release mode).
    *   Micro-bench verify "eventual consistency" (objects searchable within <5s).

### Milestone 2: [CO-02] 诊断与共生接口 (CoreObserve Diagnostic)
*   **Goal**: Surface processing state and failures to Agents according to Axiom 7.
*   **Deliverables**:
    *   `CoreObserve(variant="diagnostic")` implementation.
    *   `DiagnosticObject` creation on task failure (e.g., embedding server 500).
    *   `CoreExec(action="retry_diagnostic")` for Agent-driven recovery.
*   **Verification**:
    *   Reproduce MAB AR 500 error; verify Agent receives recovery hint.
    *   Verify MAB AR hit rate becomes non-zero after manual/auto retry.

### Milestone 3: [SF-03] 智能化技能铁匠 (Intelligent SkillForge)
*   **Goal**: Pattern-driven skill recommendations instead of mechanical counting.
*   **Deliverables**:
    *   Semantic cluster detection in the cognitive loop.
    *   `KernelEvent::SkillCandidate` event emission.
    *   Agent-mediated skill solidification via `CoreExec`.
*   **Verification**:
    *   20-round `LoCoMo` test; verify system identifies repeated intent patterns.

### Milestone 4: [V41-FINAL] 全量对齐与精品基线
*   **Goal**: Final performance and quality validation.
*   **Deliverables**:
    *   v41 Baseline Benchmark Report.
*   **Success Metrics**:
    *   `cas_write` QPS > 500.
    *   `MemoryAgentBench AR` Hit Rate > 10% (via self-healing).
    *   `KG Multi-hop` Hit Rate > 30%.

---

## 2. Engineering Red Lines (v41 Specific)

- **Atomic Integrity**: All asynchronous metadata updates must use atomic writes.
- **Resource Guardrails**: Background tasks must be rate-limited to 2x the number of CPU cores to prevent starvation.
- **Zero Panic**: String slicing must use `safe_truncate` / `safe_range`.
- **Naming Hygiene**: No temporary artifacts in root; use `.runtime/`.
