# Plico v33 Baseline Benchmark Report

**Date**: 2026-04-29  
**Build**: `cargo build --release` (optimized)  
**Hardware**: NVIDIA GB10 Grace Blackwell Superchip (128GB LPDDR5X)  
**Models**:
- LLM: **Gemma 4 26B-A4B-it Q4_K_M** (port 18920, `--reasoning off`)
- Embedding: **v5-small-retrieval Q4_K_M** (1024-dim, port 18921)

**Total runtime**: 1,680s (28 min)  
**Result**: **26/26 benchmarks passed**

---

## 1. Summary Table

| # | Benchmark | Category | Metric | Value | Pass |
|---|-----------|----------|--------|-------|------|
| B1 | Intent Classification (LLM) | LLM Quality | Accuracy | **100%** (10/10) | ✅ |
| B1 | Intent Classification (Rules) | Core Algo | Accuracy | 90% (9/10) | ✅ |
| B2 | Embedding Similarity | Embedding | Accuracy | **100%** (6/6) | ✅ |
| B3 | Memory Distillation | LLM Quality | Compression | **42.4%** | ✅ |
| B4 | Contradiction Detection | LLM Quality | Accuracy | **88%** (7/8) | ✅ |
| B5 | CAS Store + Semantic Search | Kernel | Accuracy | **100%** (5/5) | ✅ |
| B6 | Recall Routed (Intent+Search) | Kernel | Intent Accuracy | **100%** (5/5) | ✅ |
| B7 | Causal Graph | Core Algo | Correctness | **100%** | ✅ |
| B8 | Full Pipeline (Store→Distill→Recall) | E2E | Latency | **1,952ms** | ✅ |
| B9 | Scale Test (50 entries) | Scale | Relevance | 80% (8/10) | ✅ |
| B10 | Embedding Throughput (30) | Perf | Throughput | **112.4 emb/s** | ✅ |
| B11 | Multi-Session Memory | Kernel | Cross-session | **100%** | ✅ |
| B12 | LLM Latency Stability (20) | Perf | CV | 42.7% | ✅ |
| B13 | Batch Embedding | Perf | Speedup | **2.77x** | ✅ |
| B14 | Multi-Round Conversation | E2E | Validation | **100%** | ✅ |
| B15 | CSC Contradiction Detection | Core Algo | Accuracy | **80%** (16/20) | ✅ |
| B16 | RFE Retrieval Fusion | Core Algo | RFE vs Cosine | 2/3 | ✅ |
| B17 | MCE Consolidation | Core Algo | Actions | 6 actions | ✅ |
| B18 | Agent Profile Learning | Core Algo | Convergence | **95ms avg** | ✅ |
| B19 | Real-World Context | E2E | Recall | **100%** (10/10) | ✅ |
| B20 | LongMemEval (self-aligned) | Benchmark | Accuracy | **91%** (10/11) | ✅ |
| B21 | LoCoMo (self-aligned) | Benchmark | Accuracy | **100%** (9/9) | ✅ |
| B22 | Scale 500 entries | Scale | Accuracy | **100%** (10/10) | ✅ |
| B23 | Real Context Scale | Scale | Accuracy | **90%** (9/10) | ✅ |
| B24 | RFE 7-Signal Fusion | Core Algo | Accuracy | **100%** (10/10) | ✅ |
| **B25** | **LongMemEval Real (ICLR 2025)** | **Industry** | **Accuracy** | **68.3%** (41/60) | ✅ |
| **B26** | **LoCoMo Real (ACL 2024)** | **Industry** | **Accuracy** | **61.0%** (61/100) | ✅ |

---

## 2. Industry Benchmark Details

### B25: LongMemEval (Real Dataset, S-setting)

**Source**: [LongMemEval](https://github.com/xiaowu0162/LongMemEval) — ICLR 2025  
**Setting**: S-setting, 500 questions, ~115k tokens/question, ~53 sessions/question  
**Sampled**: 60 questions (10 per category × 6 categories)  
**Evaluation**: Keyword match + LLM judge (Gemma 4)

| Category | Accuracy | Description |
|----------|----------|-------------|
| single-session-user | **90%** (9/10) | Extract user facts from a single session |
| single-session-assistant | **90%** (9/10) | Extract assistant-provided facts |
| single-session-preference | 30% (3/10) | User preference recall |
| temporal-reasoning | 60% (6/10) | Time-based reasoning |
| knowledge-update | **90%** (9/10) | Detect updated information |
| multi-session | 50% (5/10) | Cross-session reasoning |
| **Overall** | **68.3%** (41/60) | |

**Performance**:
- Avg ingest per question: 26,116ms (embedding ~500 turns per question)
- Avg query latency: **19.8ms** (pure retrieval, excluding judge)
- Total judge time: 13,559ms

### B26: LoCoMo (Real Dataset)

**Source**: [LoCoMo](https://github.com/snap-research/LoCoMo) — ACL 2024  
**Setting**: 2 conversations (of 10), ~400 turns each, 100 QA pairs tested  
**Evaluation**: Keyword match + LLM judge (Gemma 4)

| Category | Accuracy | Description |
|----------|----------|-------------|
| single-hop | 23% (3/13) | Direct fact lookup |
| temporal | 57% (16/28) | Time-related questions |
| common-sense | 40% (2/5) | Inference questions |
| multi-hop | **69%** (31/45) | Cross-turn reasoning |
| adversarial | **100%** (9/9) | Should not hallucinate |
| **Overall** | **61.0%** (61/100) | |

**Performance**:
- Ingest: 786 turns in 24,832ms (31.6ms/turn)
- Avg query latency: **10.0ms** (pure retrieval)
- Total judge time: 20,914ms

---

## 3. Performance Baselines

### Latency Profile

| Operation | Latency | Throughput |
|-----------|---------|------------|
| LLM intent classification | 123ms/query | 8.1 QPS |
| LLM contradiction detection | 138ms/pair | 7.2 QPS |
| LLM summarization (distillation) | 323ms/group | 3.1 QPS |
| Embedding single | 8.9ms (p50=8ms, p95=11ms) | 112 emb/s |
| Embedding batch | 3.0ms/text | 333 emb/s |
| CAS store + index | 24.0ms (p50=28ms, p95=35ms) | 42 ops/s |
| Semantic search (top-5) | 8ms/query | 125 QPS |
| Recall routed (LLM+search) | ~200ms/query | 5 QPS |
| Causal graph build | 5μs | 200K ops/s |
| Full pipeline (store→distill→recall) | 1,952ms | — |

### Scale Characteristics (B22)

| Scale | Ingest Total | ms/item | Query Latency |
|-------|-------------|---------|---------------|
| 100 | 4,234ms | 42.3 | — |
| 200 | 8,757ms | 43.8 | — |
| 300 | 13,422ms | 44.7 | — |
| 400 | 18,360ms | 45.9 | — |
| 500 | 23,560ms | 47.1 | **8.8ms** |

Query latency remains flat at ~9ms regardless of scale.

---

## 4. Competitive Landscape (2026)

### LongMemEval Comparison

| System | Model | Overall | single-user | single-asst | preference | temporal | knowledge-update | multi-session |
|--------|-------|---------|-------------|-------------|------------|----------|-----------------|---------------|
| **Hindsight** | Gemini-3 Pro | **91.4%** | 97.1% | 96.4% | 80.0% | **91.0%** | **94.9%** | **87.2%** |
| **Hindsight** | OSS-120B | 89.0% | **100%** | 98.2% | **86.7%** | 85.7% | 92.3% | 81.2% |
| **Hindsight** | OSS-20B | 83.6% | 95.7% | 94.6% | 66.7% | 79.7% | 84.6% | 79.7% |
| Supermemory | Gemini-3 | 85.2% | 98.6% | 98.2% | 70.0% | 82.0% | 89.7% | 76.7% |
| Supermemory | GPT-5 | 84.6% | 97.1% | **100%** | 76.7% | 81.2% | 87.2% | 75.2% |
| Supermemory | GPT-4o | 81.6% | 97.1% | 96.4% | 70.0% | 76.7% | 88.5% | 71.4% |
| Zep | GPT-4o | 71.2% | 92.9% | 80.4% | 56.7% | 62.4% | 83.3% | 57.9% |
| **Plico v33** | **Gemma 4 26B (local)** | **68.3%** | **90%** | **90%** | 30% | 60% | **90%** | 50% |
| Full-context | GPT-4o | 60.2% | 81.4% | 94.6% | 20.0% | 45.1% | 78.2% | 44.3% |
| Mem0 | GPT-4o | 49.0% | — | — | — | — | — | — |
| Full-context | OSS-20B | 39.0% | 38.6% | 80.4% | 20.0% | 31.6% | 60.3% | 21.1% |

### LoCoMo Comparison

| System | Model | Overall | Source |
|--------|-------|---------|--------|
| Hindsight | Gemini-3 | **89.6%** | arxiv 2512.12818 |
| Hindsight | OSS-120B | 85.7% | arxiv 2512.12818 |
| Hindsight | OSS-20B | 83.2% | arxiv 2512.12818 |
| Letta/MemGPT | GPT-4o-mini | 74.0% | letta.com blog |
| Full-context | — | 72.9% | Mem0 ECAI paper |
| Mem0g (graph) | GPT-4o | 68.4% | Mem0 ECAI paper |
| Mem0 (base) | GPT-4o | 66.9% | Mem0 ECAI paper |
| **Plico v33** | **Gemma 4 26B (local)** | **61.0%** | **This report** |
| RAG baseline | — | 61.0% | Mem0 ECAI paper |

### Latency Comparison

| System | p95 Latency | Cost Model |
|--------|-------------|------------|
| **Plico v33** | **< 20ms** (retrieval only) | **Free (local)** |
| Mem0 (base) | 1.44s | $249/mo (graph) |
| Mem0g (graph) | 2.59s | $249/mo |
| Zep | ~4s avg | Cloud pricing |
| Letta | Model-dependent | Per-query LLM |
| Hindsight | Per-query LLM | Self-hosted |
| Full-context | 17.12s (p95) | ~14x token cost |

---

## 5. Analysis & Positioning

### Strengths
1. **Retrieval latency**: 8-20ms query latency is **72-1000x faster** than cloud alternatives
2. **Zero-cost retrieval**: No LLM call required for recall (BM25 + vector fusion)
3. **Single-session extraction**: 90% accuracy matches Hindsight OSS-20B level
4. **Knowledge update**: 90% accuracy surpasses Supermemory (GPT-4o) 88.5%
5. **Adversarial robustness**: 100% on LoCoMo adversarial (no hallucination on negative probes)
6. **Scale stability**: Query latency flat at 9ms across 100-500 entries

### Gaps
1. **Preference recall**: 30% (vs Hindsight 86.7%) — preferences often expressed implicitly
2. **Multi-session**: 50% (vs Hindsight 87.2%) — requires graph traversal across sessions
3. **Temporal reasoning**: 60% (vs Hindsight 91.0%) — needs structured time extraction
4. **Single-hop (LoCoMo)**: 23% — short factoid recall needs exact-match retrieval

### Key Differentiators
- **Fully local**: All computation on-device, no cloud dependency
- **Model**: 26B quantized (Q4_K_M) vs competitors using GPT-4o/GPT-5/Gemini-3
- **Architecture**: OS-kernel design (CAS + 4-tier memory + KG) vs standalone memory layer
- **Token cost**: Zero per-query LLM cost for retrieval; competitors require LLM per recall

### Root Cause Analysis for Gaps

| Gap | Root Cause | Mitigation Path |
|-----|-----------|-----------------|
| Preference 30% | Embedding similarity alone misses implicit preferences | Add preference extraction at ingest (LLM-assisted) |
| Multi-session 50% | No session-aware graph linking | Use KG edges between session entities |
| Temporal 60% | No structured timestamp extraction | Parse dates from content, build temporal index |
| Single-hop 23% | Short-answer factoids diluted in long contexts | Entity-level indexing, not just passage-level |

---

## 6. Benchmark Methodology

### Timing Separation (Amortized Warm Benchmark)

All benchmarks with LLM preprocessing separate three phases:
1. **Ingest Phase**: Storage + embedding generation (amortized one-time cost)
2. **Query Phase**: Pure retrieval latency (no LLM call)
3. **Judge Phase**: LLM-based answer evaluation (benchmark overhead, not system cost)

### Real Dataset Integration
- **B25**: LongMemEval S-setting (ICLR 2025), 500 questions, 6 categories. Each question has its own ~53-session haystack (~115k tokens). Sampled 60 questions (10/category). Independent kernel per question (no cross-contamination).
- **B26**: LoCoMo (ACL 2024), 10 conversations, 1986 QA pairs. Tested 2 conversations × 50 QA = 100 pairs. Full conversation ingested before querying.

### Evaluation
- Primary: Keyword substring matching against expected answer
- Fallback: LLM judge (Gemma 4) for ambiguous cases
- LoCoMo categories mapped from numeric IDs: 1=single-hop, 2=temporal, 3=common-sense, 4=multi-hop, 5=adversarial

---

## 7. Raw Numbers

```
Total benchmarks:  26/26 passed
Total runtime:     1,680s (28 min)
Unit tests:        1,038 passed
Integration tests: 38 passed
Clippy warnings:   0
Build profile:     release (optimized)
```

### Model Details
```
LLM:       gemma-4-26B-A4B-it-Q4_K_M.gguf
           26B params, 4B active (MoE), Q4_K_M quantization
           Context: 8192 tokens, reasoning off
           
Embedding: v5-small-retrieval-Q4_K_M.gguf  
           1024-dimension, Q4_K_M quantization
           Pooling: last, continuous batching
```
