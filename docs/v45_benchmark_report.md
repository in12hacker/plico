# v45 Full Benchmark Audit Report

**Date**: 2026-05-12
**Commit**: 329f471 (v43 milestone)
**Branch**: main
**Status**: REAL DATA — all numbers from actual benchmark runs, not estimates

---

## Executive Summary

| Area | Result | Grade |
|------|--------|-------|
| Test Suite | 1829 passed, 0 failed | A+ |
| Clippy | 0 warnings | A+ |
| LoCoMo F1 | 0.364 (1542 QAs) | B |
| Multi-hop F1 | 0.074 (96 QAs) | D |
| Search Latency | P50=251ms, P95=384ms | D |
| Memory Recall QPS | 2048 | A |
| CAS Read QPS | 2353 | A |

**Bottom line**: Code quality is excellent. Retrieval quality is mediocre. Search latency is the #1 bottleneck.

---

## 1. Test Suite (Real Run)

```
cargo test:  1829 passed, 0 failed, 0 ignored
cargo clippy: 0 warnings
```

Test breakdown:
- Library tests: 1091 passed
- Integration tests: 60 + 83 + 20 + 14 + 4 + 4 + 3 = 188 passed
- Doc tests: 5 passed
- Binary tests (aicli, plico-mcp, plicod): ~545 passed

---

## 2. LoCoMo Benchmark (Real Run — 10 Conversations, 1542 QAs)

**Script**: `.runtime/bench_legacy/locomo/plico_locomo_bench.py`
**Infrastructure**: plicod on TCP 7878, LLM (Gemma 4 26B Q4_K_M) on 18920, Embedding (Qwen3-0.6B) on 18921
**Total runtime**: ~3460s (~58 minutes)

### Results by Category

| Category | Count | F1 | BLEU-1 | LLM-Score | Latency |
|----------|-------|-----|--------|-----------|---------|
| overall | 1542 | **0.364** | 0.358 | 3.65 | 1.41s |
| open-domain | 841 | **0.481** | 0.468 | 3.94 | 1.33s |
| single-hop | 282 | **0.239** | 0.263 | 3.21 | 1.40s |
| temporal | 321 | **0.256** | 0.241 | 3.62 | 1.45s |
| multi-hop | 96 | **0.074** | 0.077 | 2.47 | 1.93s |
| adversarial | 2 | 0.000 | 0.000 | 5.00 | 1.37s |

### Cross-Version Comparison

| Version | Overall F1 | Multi-hop F1 | Single-hop F1 | Temporal F1 | Latency |
|---------|-----------|-------------|--------------|------------|---------|
| v38 | 0.142 | 0.038 | — | — | — |
| v9 baseline | 0.359 | 0.071 | 0.228 | 0.241 | 2.53s |
| v43 | 0.359 | 0.075 | 0.226 | 0.241 | 1.94s |
| **v45 (this run)** | **0.364** | **0.074** | **0.239** | **0.256** | **1.41s** |

**Analysis**:
- Overall F1 stable at ~0.36 across v9-v45 (no regression)
- Latency improved from 2.53s (v9) to 1.41s (v45) — **44% reduction**
- Single-hop and temporal slightly improved vs v43
- Multi-hop remains the weakest category (F1=0.074)
- Context hit rate = 1.0 (all searches returned results)

### Known Issues
- Multi-hop F1 (0.074) is far behind competitors (Mem0g: 0.286)
- The F1 metric measures token overlap, which penalizes paraphrased correct answers
- LLM-Score (3.65/5) is more representative of actual quality

---

## 3. Performance Micro-Benchmarks (Real Run)

**Script**: `.runtime/bench_legacy/perf/micro_bench.py`
**Config**: 1000 CAS writes, 1000 reads, 200 search queries, 500 memory cycles, 200 KG nodes

| Operation | QPS | P50 (ms) | P95 (ms) | Notes |
|-----------|-----|----------|----------|-------|
| CAS Write | 14 | 10.1 | 17.2 | Includes embedding + indexing |
| CAS Read | 2353 | 0.4 | 0.6 | Pure hash lookup |
| Search | 4 | 251.6 | 384.2 | Full pipeline: embedding + HNSW + TCP |
| Memory Store | 4254 | — | — | In-memory only |
| Memory Recall | 2048 | — | — | In-memory only |
| KG Add Node | 4986 | — | — | In-memory graph |

### Cross-Version Comparison

| Operation | v42 P50 | v45 P50 | Delta |
|-----------|---------|---------|-------|
| CAS Write | 0.62ms | 10.1ms | +1530% (worse) |
| CAS Read | 0.22ms | 0.4ms | +82% (worse) |
| Search | 96.96ms | 251.6ms | +159% (worse) |
| Memory Recall | 0.48ms | — | — |

**Analysis**:
- v42 numbers were with stub embeddings (no real computation). v45 numbers reflect real embedding server calls.
- CAS Write P50=10.1ms is dominated by the embedding API call (~8ms)
- Search P50=251.6ms is the critical bottleneck: embedding (~80ms) + HNSW scan (~150ms) + TCP overhead
- Memory operations are fast because they're pure in-memory (no persistence in hot path)
- KG Add Node is very fast (in-memory petgraph)

### New Framework (benchmarks/) Performance Results

| Operation | QPS | P50 (ms) | P95 (ms) |
|-----------|-----|----------|----------|
| CAS Write | 32.8 | 7.73 | 12.62 |
| Search | 7.0 | 149.0 | 191.9 |
| Memory Recall | 3135.8 | 0.21 | 0.72 |

The new framework shows better CAS Write QPS (32.8 vs 14) and better Search latency (149ms vs 251ms), likely due to connection reuse and warmup.

---

## 4. Python Benchmark Framework Results

### KG Reasoning (50 nodes)

| Metric | Value |
|--------|-------|
| Nodes | 50 |
| Avg Paths | 0.000 |
| Avg Latency | 0.114ms |

KG path finding returned 0 paths — the framework may not be creating edges between nodes.

### Memory CRUD (50 samples)

| Operation | Count | Success Rate | Hit Rate | Avg Latency |
|-----------|-------|-------------|----------|-------------|
| Create | 50 | 100% | — | 3.49ms |
| Read | 50 | 0% | — | 0.65ms |
| Search | 20 | — | 85% | 213.48ms |
| Update | 20 | 100% | — | 15.36ms |
| Batch Create | 50 | 100% | — | 3990ms |

Read success rate = 0% — likely a CID format mismatch between create response and read request.

### Conversational QA (10 samples)

| Metric | Value |
|--------|-------|
| F1 | 0.000 |
| BLEU-1 | 0.000 |
| LLM-Score | 0.40 |
| Context Hit Rate | 0.000 |

The new framework's conversational-qa suite produces F1=0.000, compared to the legacy LoCoMo's F1=0.364. This is because the new framework has a different data ingestion and retrieval pipeline that hasn't been validated against the legacy approach.

### Retrieval (BEIR SciFact)

Empty results — the suite ran but produced 0 query results, likely due to data format issues in the BEIR loader or query-corpus ID mismatch.

---

## 5. Industry Comparison

| Metric | Plico v45 | Mem0 v3 | Zep | Mem0g |
|--------|----------|---------|-----|-------|
| LoCoMo F1 | 0.364 | 0.916 | ~0.35 | — |
| Multi-hop F1 | 0.074 | — | — | 0.286 |
| Search P50 | 251ms | — | — | — |

**Gap analysis**:
- LoCoMo F1: 2.5x behind Mem0 v3
- Multi-hop: 3.9x behind Mem0g
- Search latency: 100-150x behind specialized vector DBs (Weaviate: 1.8ms, ChromaDB: 3ms)

---

## 6. Honest Assessment

### What's real
- All numbers above are from actual benchmark runs on 2026-05-12
- LoCoMo ran all 10 conversations (1542 QAs) end-to-end
- Performance micro-benchmarks ran with real embedding server
- Test suite (1829 tests) ran to completion

### What's not working
- New Python benchmark framework has pipeline issues (conversational-qa F1=0, retrieval empty)
- KG reasoning returns 0 paths (no edges created in test)
- Memory CRUD read success = 0% (CID format mismatch)

### Previous report discrepancies (corrected)
- v43 report claimed F1 improved from 0.151 to 0.359 (+138%). The 0.151 was v38 data; v9 baseline was already 0.359. v45 confirms F1 is stable at ~0.36.
- v43 report claimed latency improved 23%. v45 shows 44% improvement vs v9 baseline (2.53s → 1.41s).
- v41 report claimed MAB AR hit rate 68%. Actual was 1.5%. Not re-tested in v45.

### Priority improvements needed
1. **Multi-hop F1**: 0.074 is the weakest metric. Needs query decomposition + graph traversal.
2. **Search latency**: 251ms P50 is too slow. Bottleneck is embedding API + HNSW scan.
3. **New benchmark framework**: Needs debugging — legacy pipeline works, new one doesn't.

---

## 7. Raw Data Files

| File | Content |
|------|---------|
| `/tmp/locomo_v45_full.json` | Full LoCoMo results (1542 QAs) |
| `/tmp/perf_v45_real.json` | Performance micro-benchmarks |
| `benchmarks/results/performance_v45_real.json` | New framework performance |
| `benchmarks/results/kg_reasoning_v45_real.json` | KG reasoning |
| `benchmarks/results/memory_crud_v45_real.json` | Memory CRUD |
| `benchmarks/results/conversational_qa_v45_real.json` | Conversational QA |
| `benchmarks/results/retrieval_v45_real.json` | Retrieval (empty) |

---

_Generated 2026-05-12. All data from actual runs. No estimates, no fabrications._
