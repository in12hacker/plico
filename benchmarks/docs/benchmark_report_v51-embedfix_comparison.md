# Plico Benchmark Report

> Generated: 2026-05-16 22:14:15
> Compared against: v50

## 1. Summary

| Suite | Key Metric | Value | Competitor Best | Gap |
|-------|-----------|-------|----------------|-----|
| causal-reasoning | cause_finds_effect | 0.000 | — | — |
| conversational-qa | accuracy_pct | 37.500 | 95.4 (OMEGA) | -57.9 |
| intent-routing | avg_intent_hit | 0.664 | — | — |
| kg-reasoning | avg_latency_ms | 0.132 | — | — |
| memory-lifecycle | create.success_rate | 1.000 | — | — |
| performance | search.p50_ms | 78.279 | — | — |
| proactive-optimization | L0.avg_tokens | 113.000 | — | — |
| retrieval | beir_scifact.recall@5 | 0.627 | 72.31 (NV-Embed-v2) | -71.7 |
| scope-isolation | leak_rate | 0.000 | — | — |
| session-lifecycle | success_rate | 1.000 | — | — |
| token-efficiency | context_l0.avg_tokens | 532.700 | 1294 (Memori) | -761.3 |

## 2. Suite Results

### causal-reasoning

| Metric | Value |
|--------|-------|
| causal_chain_construction | count=30, avg_node_latency_ms=0.57 |
| causal_path_depth_2 | paths_found=1, latency_ms=0.26 |
| causal_path_depth_3 | paths_found=2, latency_ms=0.26 |
| causal_path_depth_4 | paths_found=5, latency_ms=0.36 |
| causal_path_depth_10 | paths_found=9, latency_ms=0.49 |
| causal_retrieval | count=5, cause_finds_effect_rate=0.00, effect_finds_cause_rate=0.00, bidirectional_rate=0.00, accuracy_pct=0.00 |

#### temporal_multi_hop Competitors

#### hotpotqa Competitors

#### bigbench_hard Competitors

_Causal reasoning has no direct benchmark. Temporal/multi-hop scores are proxies._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| causal_chain_construction.count | 30 | 30 | = |
| causal_chain_construction.avg_node_latency_ms | 0.698 | 0.570 | -0.129 |
| causal_path_depth_2.paths_found | 1 | 1 | = |
| causal_path_depth_2.latency_ms | 0.254 | 0.261 | +0.007 |
| causal_path_depth_3.paths_found | 2 | 2 | = |
| causal_path_depth_3.latency_ms | 0.265 | 0.260 | -0.005 |
| causal_path_depth_4.paths_found | 5 | 5 | = |
| causal_path_depth_4.latency_ms | 0.243 | 0.364 | +0.122 |
| causal_path_depth_10.paths_found | 9 | 9 | = |
| causal_path_depth_10.latency_ms | 0.497 | 0.490 | -0.007 |
| causal_retrieval.count | 5 | 5 | = |
| causal_retrieval.cause_finds_effect_rate | 0.000 | 0.000 | = |
| causal_retrieval.effect_finds_cause_rate | 0.000 | 0.000 | = |
| causal_retrieval.bidirectional_rate | 0.000 | 0.000 | = |
| causal_retrieval.accuracy_pct | 0.000 | 0.000 | = |

### conversational-qa

| Metric | Value |
|--------|-------|
| count | 40 |
| f1 | 0.240 |
| bleu1 | 0.204 |
| llm_score | 2.475 |
| accuracy_pct | 37.500 |
| context_hit_rate | 1.000 |
| mean | 0.240 |
| std | 0.369 |
| ci95_low | 0.126 |
| ci95_high | 0.355 |

#### RAGAS Metrics

| Metric | Score | Target |
|--------|-------|--------|
| faithfulness | 0.900 | 0.85 (+0.05) |
| answer_relevancy | 0.650 | 0.80 (-0.15) |
| context_precision | 0.655 | 0.65 (+0.01) |
| context_recall | 0.655 | 0.75 (-0.09) |

#### longmemeval Competitors

| Competitor | Overall | User | Asst | Pref | Update | Temporal | Notes | Source | Date |
|-----------|---------|-----|-----|-----|-----|-----|-------|--------|------|
| Full-context GPT-4o (ceiling) | 60.2% | 81.4% | 94.6% | 20.0% | 78.2% | 45.1% | Passes entire conversation to GPT-4o. Impractical at scale. | hindsight-benchmarks README | 2026-02 |
| Hindsight (Gemini-3) | 91.4% | 97.1% | 96.4% | 80.0% | 94.9% | 87.2% | Best overall. Local PostgreSQL, no cloud. Memory architecture drives performance. | hindsight-benchmarks README | 2026-02 |
| Hindsight (OSS-20B) | 83.6% | 92.9% | 80.4% | 56.7% | 83.3% | 62.4% | +44.6pp vs full-context OSS-20B baseline (39.0%). Architecture > model size. | hindsight-benchmarks README | 2026-02 |
| OMEGA | 95.4% | 99.2% | 99.2% | 100.0% | 96.2% | 94.0% | 466/500 raw. bge-small ONNX, local M1 MacBook. 6-stage search pipeline. Hardest: multi-session reasoning (83.5%). | https://omegamax.co/benchmarks | 2026-02 |
| Supermemory (Gemini-3) | 85.2% | — | — | — | — | — | Per-category breakdown not published. GPT-4o variant: 73% multi-session. | hindsight-benchmarks README | 2026-02 |
| Supermemory (GPT-4o) | 81.6% | — | — | — | — | — | Per-category breakdown not published. | hindsight-benchmarks README | 2026-02 |
| Mastra Observational Memory | 94.87% | — | — | — | — | — | Observer + Reflector background agents, stable context window (prompt-cacheable). Open-source. | Mastra AI blog | 2026-02 |
| Supermemory (experimental) | 99.0% | — | — | — | — | — | Claimed 'agent memory frontier breakthrough'. Methodology details pending. | Supermemory blog | 2026-03 |
| Mem0 (new algorithm) | 93.4% | — | — | — | — | — | April 2026 algorithm update. Jumped from 67.8%. 6.8K tokens, 1.09s latency. | https://github.com/mem0ai/mem0/ | 2026-04 |

#### locomo Competitors

| Competitor | Overall | 1-Hop | M-Hop | Open | Temporal | Notes | Source | Date |
|-----------|---------|-----|-----|-----|-----|-------|--------|------|
| Full-Context (ceiling) | 87.52% | 88.53% | 77.7% | 71.88% | 92.7% | Impractical: 26,031 tokens/query average. | Memori Labs | 2026-02 |
| Memori | 81.95% | 87.87% | 72.7% | 63.54% | 80.37% | Best retrieval-based. 1,294 tokens/query (4.97% of full context). | Memori Labs | 2026-02 |
| Zep | 79.09% | 79.43% | 69.16% | 73.96% | 83.33% | 3,911 tokens/query. | Memori Labs | 2026-02 |
| LangMem | 78.05% | 74.47% | 61.06% | 67.71% | 86.92% | Best temporal reasoning among retrieval systems. | Memori Labs | 2026-02 |
| Mem0 (legacy) | 62.47% | 62.41% | 57.32% | 44.79% | 66.47% | Pre-2026 algorithm. Weakest across all categories. | Memori Labs | 2026-02 |
| Mem0 (new algorithm) | 91.6% | — | — | — | — | April 2026 algorithm update. Dramatic jump from 62.47%. 7K tokens, 0.88s latency. | https://github.com/mem0ai/mem0/ | 2026-04 |
| EverMind HyperMem | 92.73% | — | — | — | — | Hypergraph-based hierarchical memory. Best publicly claimed LoCoMo score. | https://github.com/EverMind-AI | 2026-04 |
| delta-mem | None% | — | — | — | — | 1.20x improvement over baseline. 0.12% of backbone params. TTL subtask: 26.14→50.50. | arXiv May 2026 | 2026-05 |

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| count | 40 | 40 | = |
| f1 | 0.204 | 0.240 | +0.037 |
| bleu1 | 0.165 | 0.204 | +0.039 |
| llm_score | 2.225 | 2.475 | +0.250 |
| accuracy_pct | 30.000 | 37.500 | +7.500 |
| context_hit_rate | 1.000 | 1.000 | = |
| mean | 0.204 | 0.240 | +0.037 |
| std | 0.330 | 0.369 | +0.040 |
| ci95_low | 0.101 | 0.126 | +0.025 |
| ci95_high | 0.306 | 0.355 | +0.049 |

### intent-routing

| Metric | Value |
|--------|-------|
| intent_factual | query_count=5, hit_rate=0.40, accuracy_pct=0.00, avg_latency_ms=435.03 |
| intent_temporal | query_count=5, hit_rate=0.60, accuracy_pct=0.00, avg_latency_ms=435.49 |
| intent_multi_hop | query_count=5, hit_rate=0.92, accuracy_pct=20.00, avg_latency_ms=470.12 |
| intent_preference | query_count=5, hit_rate=0.80, accuracy_pct=0.00, avg_latency_ms=506.13 |
| intent_aggregation | query_count=5, hit_rate=0.60, accuracy_pct=0.00, avg_latency_ms=453.10 |
| intent_vs_no_intent_factual | with_intent_hits=10, without_intent_hits=10, improvement_pct=0.00 |
| intent_vs_no_intent_temporal | with_intent_hits=20, without_intent_hits=20, improvement_pct=0.00 |
| intent_vs_no_intent_multi_hop | with_intent_hits=26, without_intent_hits=26, improvement_pct=0.00 |
| intent_vs_no_intent_preference | with_intent_hits=20, without_intent_hits=20, improvement_pct=0.00 |
| intent_vs_no_intent_aggregation | with_intent_hits=20, without_intent_hits=20, improvement_pct=0.00 |

#### intent_category_scores Competitors

#### ragas Competitors

_No direct intent-routing benchmark. LoCoMo per-category scores show how different query types perform. RAGAS Context Precision/Recall are affected by intent routing quality._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| intent_factual.query_count | 5 | 5 | = |
| intent_factual.hit_rate | 0.680 | 0.400 | -0.280 |
| intent_factual.accuracy_pct | 0.000 | 0.000 | = |
| intent_factual.avg_latency_ms | 96.520 | 435.030 | +338.510 |
| intent_temporal.query_count | 5 | 5 | = |
| intent_temporal.hit_rate | 0.760 | 0.600 | -0.160 |
| intent_temporal.accuracy_pct | 20.000 | 0.000 | -20.000 |
| intent_temporal.avg_latency_ms | 99.020 | 435.490 | +336.470 |
| intent_multi_hop.query_count | 5 | 5 | = |
| intent_multi_hop.hit_rate | 1.000 | 0.920 | -0.080 |
| intent_multi_hop.accuracy_pct | 20.000 | 20.000 | = |
| intent_multi_hop.avg_latency_ms | 108.560 | 470.120 | +361.560 |
| intent_preference.query_count | 5 | 5 | = |
| intent_preference.hit_rate | 0.600 | 0.800 | +0.200 |
| intent_preference.accuracy_pct | 0.000 | 0.000 | = |
| intent_preference.avg_latency_ms | 137.830 | 506.130 | +368.300 |
| intent_aggregation.query_count | 5 | 5 | = |
| intent_aggregation.hit_rate | 0.500 | 0.600 | +0.100 |
| intent_aggregation.accuracy_pct | 0.000 | 0.000 | = |
| intent_aggregation.avg_latency_ms | 100.970 | 453.100 | +352.130 |
| intent_vs_no_intent_factual.with_intent_hits | 24 | 10 | -14.000 |
| intent_vs_no_intent_factual.without_intent_hits | 24 | 10 | -14.000 |
| intent_vs_no_intent_factual.improvement_pct | 0.000 | 0.000 | = |
| intent_vs_no_intent_temporal.with_intent_hits | 30 | 20 | -10.000 |
| intent_vs_no_intent_temporal.without_intent_hits | 30 | 20 | -10.000 |
| intent_vs_no_intent_temporal.improvement_pct | 0.000 | 0.000 | = |
| intent_vs_no_intent_multi_hop.with_intent_hits | 30 | 26 | -4.000 |
| intent_vs_no_intent_multi_hop.without_intent_hits | 30 | 26 | -4.000 |
| intent_vs_no_intent_multi_hop.improvement_pct | 0.000 | 0.000 | = |
| intent_vs_no_intent_preference.with_intent_hits | 20 | 20 | = |
| intent_vs_no_intent_preference.without_intent_hits | 20 | 20 | = |
| intent_vs_no_intent_preference.improvement_pct | 0.000 | 0.000 | = |
| intent_vs_no_intent_aggregation.with_intent_hits | 20 | 20 | = |
| intent_vs_no_intent_aggregation.without_intent_hits | 20 | 20 | = |
| intent_vs_no_intent_aggregation.improvement_pct | 0.000 | 0.000 | = |

### kg-reasoning

| Metric | Value |
|--------|-------|
| n_nodes | 50 |
| avg_paths_unweighted | 2.500 |
| avg_paths_weighted | 0.000 |
| avg_latency_ms | 0.132 |
| path_validity_rate | 1.000 |
| total_paths | 11 |
| valid_paths | 11 |
| causal_paths_found | 1 |
| causal_valid_paths | 1 |
| causal_latency_ms | 0.037 |

#### hotpotqa Competitors

#### agentbench Competitors

| Competitor | Accuracy | Notes | Source | Date |
|-----------|----------|-------|--------|------|
| GPT-4o | ~65% | Across 8 agent environments |  | 2024 |
| Claude 3.5 Sonnet | ~60% | Competitive on coding/web agent tasks |  | 2024 |

#### kg_frameworks Competitors

_No direct KG path-finding benchmark exists. HotpotQA measures multi-hop QA (closest)._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| n_nodes | 50 | 50 | = |
| avg_paths_unweighted | 2.500 | 2.500 | = |
| avg_paths_weighted | 0.000 | 0.000 | = |
| avg_latency_ms | 0.356 | 0.132 | -0.224 |
| path_validity_rate | 1.000 | 1.000 | = |
| total_paths | 11 | 11 | = |
| valid_paths | 11 | 11 | = |
| causal_paths_found | 1 | 1 | = |
| causal_valid_paths | 1 | 1 | = |
| causal_latency_ms | 0.258 | 0.037 | -0.220 |

### memory-lifecycle

| Metric | Value |
|--------|-------|
| create | count=100, success_rate=1.00, avg_latency_ms=3.44 |
| read | count=100, success_rate=1.00, avg_latency_ms=0.09 |
| search | count=20, hit_rate=1.00, avg_latency_ms=65.87 |
| update | count=20, success_rate=1.00, avg_latency_ms=3.38 |
| delete | count=10, success_rate=1.00, avg_latency_ms=0.04, search_gone_rate=0.00 |
| batch_create | count=50, success_rate=1.00, avg_latency_ms=405.69 |
| layer_migration | count=15, cross_layer_hit_rate=0.33, recall_hit_rate=0.00, accuracy_pct=13.30 |
| checkpoint_restore | count=10, cp1_hit_rate=1.00, cp1_persistence_rate=1.00, cp2_hit_rate=1.00 |

#### longmemeval Competitors

| Competitor | Overall | User | Asst | Pref | Update | Temporal | Notes | Source | Date |
|-----------|---------|-----|-----|-----|-----|-----|-------|--------|------|
| Full-context GPT-4o (ceiling) | 60.2% | 81.4% | 94.6% | 20.0% | 78.2% | 45.1% | Passes entire conversation to GPT-4o. Impractical at scale. | hindsight-benchmarks README | 2026-02 |
| Hindsight (Gemini-3) | 91.4% | 97.1% | 96.4% | 80.0% | 94.9% | 87.2% | Best overall. Local PostgreSQL, no cloud. Memory architecture drives performance. | hindsight-benchmarks README | 2026-02 |
| Hindsight (OSS-20B) | 83.6% | 92.9% | 80.4% | 56.7% | 83.3% | 62.4% | +44.6pp vs full-context OSS-20B baseline (39.0%). Architecture > model size. | hindsight-benchmarks README | 2026-02 |
| OMEGA | 95.4% | 99.2% | 99.2% | 100.0% | 96.2% | 94.0% | 466/500 raw. bge-small ONNX, local M1 MacBook. 6-stage search pipeline. Hardest: multi-session reasoning (83.5%). | https://omegamax.co/benchmarks | 2026-02 |
| Supermemory (Gemini-3) | 85.2% | — | — | — | — | — | Per-category breakdown not published. GPT-4o variant: 73% multi-session. | hindsight-benchmarks README | 2026-02 |
| Supermemory (GPT-4o) | 81.6% | — | — | — | — | — | Per-category breakdown not published. | hindsight-benchmarks README | 2026-02 |
| Mastra Observational Memory | 94.87% | — | — | — | — | — | Observer + Reflector background agents, stable context window (prompt-cacheable). Open-source. | Mastra AI blog | 2026-02 |
| Supermemory (experimental) | 99.0% | — | — | — | — | — | Claimed 'agent memory frontier breakthrough'. Methodology details pending. | Supermemory blog | 2026-03 |
| Mem0 (new algorithm) | 93.4% | — | — | — | — | — | April 2026 algorithm update. Jumped from 67.8%. 6.8K tokens, 1.09s latency. | https://github.com/mem0ai/mem0/ | 2026-04 |

#### locomo Competitors

| Competitor | Overall | 1-Hop | M-Hop | Open | Temporal | Notes | Source | Date |
|-----------|---------|-----|-----|-----|-----|-------|--------|------|
| Full-Context (ceiling) | 87.52% | 88.53% | 77.7% | 71.88% | 92.7% | Impractical: 26,031 tokens/query average. | Memori Labs | 2026-02 |
| Memori | 81.95% | 87.87% | 72.7% | 63.54% | 80.37% | Best retrieval-based. 1,294 tokens/query (4.97% of full context). | Memori Labs | 2026-02 |
| Zep | 79.09% | 79.43% | 69.16% | 73.96% | 83.33% | 3,911 tokens/query. | Memori Labs | 2026-02 |
| LangMem | 78.05% | 74.47% | 61.06% | 67.71% | 86.92% | Best temporal reasoning among retrieval systems. | Memori Labs | 2026-02 |
| Mem0 (legacy) | 62.47% | 62.41% | 57.32% | 44.79% | 66.47% | Pre-2026 algorithm. Weakest across all categories. | Memori Labs | 2026-02 |
| Mem0 (new algorithm) | 91.6% | — | — | — | — | April 2026 algorithm update. Dramatic jump from 62.47%. 7K tokens, 0.88s latency. | https://github.com/mem0ai/mem0/ | 2026-04 |
| EverMind HyperMem | 92.73% | — | — | — | — | Hypergraph-based hierarchical memory. Best publicly claimed LoCoMo score. | https://github.com/EverMind-AI | 2026-04 |
| delta-mem | None% | — | — | — | — | 1.20x improvement over baseline. 0.12% of backbone params. TTL subtask: 26.14→50.50. | arXiv May 2026 | 2026-05 |

#### personamem Competitors

| Competitor | Accuracy | Notes | Source | Date |
|-----------|----------|-------|--------|------|
| Tencent AgentMemory | 76.1% | Top1. +59% over native OpenClaw. 79%+ user fact recall (from <30%). | Tencent Cloud | 2026-04 |
| EverMind (EverOS) | None% | #2 on PersonaMem. Open-source memory layer for AI agents. | EverOS GitHub | 2026-04 |

#### memory_frameworks Competitors

#### ragas Competitors

_CRUD and layer migration are unique to Plico. LongMemEval/LoCoMo measure memory retrieval quality._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| create.count | 100 | 100 | = |
| create.success_rate | 1.000 | 1.000 | = |
| create.avg_latency_ms | 13.095 | 3.444 | -9.651 |
| read.count | 100 | 100 | = |
| read.success_rate | 1.000 | 1.000 | = |
| read.avg_latency_ms | 0.204 | 0.089 | -0.115 |
| search.count | 20 | 20 | = |
| search.hit_rate | 0.000 | 1.000 | +1.000 |
| search.avg_latency_ms | 81.610 | 65.870 | -15.740 |
| update.count | 20 | 20 | = |
| update.success_rate | 1.000 | 1.000 | = |
| update.avg_latency_ms | 8.506 | 3.383 | -5.123 |
| delete.count | 10 | 10 | = |
| delete.success_rate | 0.000 | 1.000 | +1.000 |
| delete.avg_latency_ms | 0.001 | 0.037 | +0.036 |
| delete.search_gone_rate | 1.000 | 0.000 | -1.000 |
| batch_create.count | 50 | 50 | = |
| batch_create.success_rate | 1.000 | 1.000 | = |
| batch_create.avg_latency_ms | 350.447 | 405.692 | +55.245 |
| layer_migration.count | 15 | 15 | = |
| layer_migration.cross_layer_hit_rate | 0.000 | 0.333 | +0.333 |
| layer_migration.recall_hit_rate | 0.000 | 0.000 | = |
| layer_migration.accuracy_pct | 13.300 | 13.300 | = |
| checkpoint_restore.count | 10 | 10 | = |
| checkpoint_restore.cp1_hit_rate | 0.000 | 1.000 | +1.000 |
| checkpoint_restore.cp1_persistence_rate | 0.000 | 1.000 | +1.000 |
| checkpoint_restore.cp2_hit_rate | 0.000 | 1.000 | +1.000 |

### performance

| Metric | Value |
|--------|-------|
| cas_write | qps=239.30, p50_ms=3.73, p95_ms=6.19, p99_ms=12.24 |
| search | qps=12.30, p50_ms=78.28, p95_ms=104.91, p99_ms=145.95 |
| memory_recall | qps=5919.00, p50_ms=0.11, p95_ms=0.42, p99_ms=0.47 |
| kg_path | qps=None, p50_ms=None, p95_ms=None, p99_ms=None |

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| cas_write.qps | 21.300 | 239.300 | +218.000 |
| cas_write.p50_ms | 17.345 | 3.726 | -13.619 |
| cas_write.p95_ms | 26.518 | 6.186 | -20.331 |
| cas_write.p99_ms | 1696.189 | 12.237 | -1683.953 |
| search.qps | 1375.500 | 12.300 | -1363.200 |
| search.p50_ms | 0.079 | 78.279 | +78.200 |
| search.p95_ms | 0.152 | 104.915 | +104.762 |
| search.p99_ms | 0.562 | 145.950 | +145.388 |
| memory_recall.qps | 2218.700 | 5919.000 | +3700.300 |
| memory_recall.p50_ms | 0.446 | 0.106 | -0.339 |
| memory_recall.p95_ms | 0.513 | 0.417 | -0.096 |
| memory_recall.p99_ms | 2.602 | 0.470 | -2.132 |

### proactive-optimization

| Metric | Value |
|--------|-------|
| context_l0 | avg_tokens_per_query=113.00, hit_rate=1.00, pct_of_full=0.43 |
| context_l1 | avg_tokens_per_query=284.00, hit_rate=1.00, pct_of_full=1.09 |
| context_l2 | avg_tokens_per_query=568.80, hit_rate=1.00, pct_of_full=2.19 |
| repeated_query_latency | cold_avg_ms=369.14, warm_avg_ms=334.91, speedup_pct=9.30, cache_bust_queries=10 |
| pattern_detection | items_created=30, search_hits=10, recall_hits=0, search_recall_rate=0.33 |

#### token_efficiency Competitors

| Competitor | Tokens/Query | Cost/Query | Context % | Notes |
|-----------|-------------|-----------|-----------|-------|
| Full-Context | 26031 | $0.020825 | 100.0% | Unsustainable at scale. |
| Zep | 3911 | $0.003129 | 15.02% |  |
| Mem0 | 1764 | $0.001411 | 6.78% |  |
| Memori | 1294 | $0.001035 | 4.97% | Best accuracy/token ratio. |

_Context layering (L0/L1/L2) vs competitors' token usage. Mastra's prompt-cacheable architecture and TencentDB's 61% token reduction are comparable approaches._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| context_l0.avg_tokens_per_query | 113.000 | 113.000 | = |
| context_l0.hit_rate | 1.000 | 1.000 | = |
| context_l0.pct_of_full | 0.430 | 0.430 | = |
| context_l1.avg_tokens_per_query | 352.400 | 284.000 | -68.400 |
| context_l1.hit_rate | 1.000 | 1.000 | = |
| context_l1.pct_of_full | 1.350 | 1.090 | -0.260 |
| context_l2.avg_tokens_per_query | 1058.800 | 568.800 | -490.000 |
| context_l2.hit_rate | 1.000 | 1.000 | = |
| context_l2.pct_of_full | 4.070 | 2.190 | -1.880 |
| repeated_query_latency.cold_avg_ms | 40.510 | 369.140 | +328.630 |
| repeated_query_latency.warm_avg_ms | 0.270 | 334.910 | +334.640 |
| repeated_query_latency.speedup_pct | 99.300 | 9.300 | -90.000 |
| repeated_query_latency.cache_bust_queries | 10 | 10 | = |
| pattern_detection.items_created | 30 | 30 | = |
| pattern_detection.search_hits | 20 | 10 | -10.000 |
| pattern_detection.recall_hits | 0 | 0 | = |
| pattern_detection.search_recall_rate | 0.667 | 0.333 | -0.334 |

### retrieval

| Metric | Value |
|--------|-------|
| beir_scifact | count=30, recall@5=0.63, recall@10=0.69 |

#### mteb Competitors

| Competitor | MTEB Avg | Retrieval | Dims | Notes |
|-----------|----------|-----------|------|-------|
| Gemini Embedding 001 | 68.32 | 67.71 | 3072 | #1 English MTEB. Google API. |
| Qwen3-Embedding-8B | 70.58 | — | 4096 | #3 overall. Best self-hosted. vLLM/SGLang support. |
| NV-Embed-v2 | 72.31 | 62.65 | 4096 | Legacy MTEB score (56 tasks). NVIDIA. |
| BGE-en-ICL | 71.24 | — | 4096 | BAAI. Legacy MTEB. |
| Voyage-3.1-large | 67.4 | — | 2048 | Best non-Google API. April 2026. |
| Jina Embeddings v4 | 66.81 | — | 1024 | Native multimodal (text+image). 32K context. |
| BGE-M3 | 63.0 | — | 1024 | Dense+Sparse+ColBERT. BAAI. |
| text-embedding-3-large | 64.6 | — | 3072 | OpenAI. |
| text-embedding-3-small | 62.26 | — | 1536 | Cheapest defensible option. |
| Microsoft Harrier | None | — | None | #1 MTEB-v2 multilingual (April 2026). Open-sourced by Microsoft Bing team. |
| KaLM-Embedding-Gemma3-12B-2511 | None | — | None | #1 MTEB multilingual (May 2026). 12B params, Gemma3-based. Tencent WeChat team. |

_Plico uses BEIR SciFact recall@k. MTEB scores are from a different evaluation (56-task average). Not directly comparable but useful for embedding model selection context._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| beir_scifact.count | 30 | 30 | = |
| beir_scifact.recall@5 | 0.557 | 0.627 | +0.070 |
| beir_scifact.recall@10 | 0.770 | 0.693 | -0.077 |

### scope-isolation

| Metric | Value |
|--------|-------|
| private_isolation | count=30, leak_rate=0.00, own_access_rate=0.00, isolation_perfect=True |
| shared_access | count=30, cross_agent_access_rate=0.00 |
| group_isolation | count=30, outsider_leak_rate=0.00, isolation_perfect=True |
| multi_group_isolation | alpha_to_beta_leaks=0, beta_to_alpha_leaks=0, total_leak_rate=0.00, isolation_perfect=True |
| recall_isolation | count=5, cross_agent_leaks=0, own_recall_hits=0, isolation_perfect=True |

#### agent_frameworks Competitors

_Most agent frameworks have no native scope isolation. Plico enforces at OS level._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| private_isolation.count | 30 | 30 | = |
| private_isolation.leak_rate | 0.000 | 0.000 | = |
| private_isolation.own_access_rate | 0.000 | 0.000 | = |
| private_isolation.isolation_perfect | True | True | = |
| shared_access.count | 30 | 30 | = |
| shared_access.cross_agent_access_rate | 0.000 | 0.000 | = |
| group_isolation.count | 30 | 30 | = |
| group_isolation.outsider_leak_rate | 0.000 | 0.000 | = |
| group_isolation.isolation_perfect | True | True | = |
| multi_group_isolation.alpha_to_beta_leaks | 0 | 0 | = |
| multi_group_isolation.beta_to_alpha_leaks | 0 | 0 | = |
| multi_group_isolation.total_leak_rate | 0.000 | 0.000 | = |
| multi_group_isolation.isolation_perfect | True | True | = |
| recall_isolation.count | 5 | 5 | = |
| recall_isolation.cross_agent_leaks | 0 | 0 | = |
| recall_isolation.own_recall_hits | 0 | 0 | = |
| recall_isolation.isolation_perfect | True | True | = |

### session-lifecycle

| Metric | Value |
|--------|-------|
| session_lifecycle | count=20, success_rate=1.00, avg_start_latency_ms=42.80, avg_end_latency_ms=0.74 |
| cross_session_memory | count=10, search_persistence_rate=0.00, recall_persistence_rate=0.00 |
| session_vs_persistent | count=5, working_memory_persistence=0.00, longterm_memory_persistence=0.00 |
| warm_context_delta | session1_items=10, cross_session_hit_rate=0.00, assembly_latency_ms=47.24 |

#### agent_frameworks Competitors

#### locomo Competitors

| Competitor | Accuracy | Notes | Source | Date |
|-----------|----------|-------|--------|------|
| Full-Context (ceiling) | 87.52% |  |  |  |
| Memori | 81.95% |  |  |  |
| Zep | 79.09% |  |  |  |
| LangMem | 78.05% |  |  |  |
| Mem0 (legacy) | 62.47% |  |  |  |
| Mem0 (new algorithm) | 91.6% |  |  |  |
| EverMind HyperMem | 92.73% |  |  |  |
| delta-mem | None% |  |  |  |

_Cross-session memory persistence measured by LoCoMo. Most frameworks lack native session management._

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| session_lifecycle.count | 20 | 20 | = |
| session_lifecycle.success_rate | 0.000 | 1.000 | +1.000 |
| session_lifecycle.avg_start_latency_ms | 127.926 | 42.799 | -85.128 |
| session_lifecycle.avg_end_latency_ms | 0 | 0.743 | +0.743 |
| cross_session_memory.count | 10 | 10 | = |
| cross_session_memory.search_persistence_rate | 0.000 | 0.000 | = |
| cross_session_memory.recall_persistence_rate | 0.000 | 0.000 | = |
| session_vs_persistent.count | 5 | 5 | = |
| session_vs_persistent.working_memory_persistence | 0.000 | 0.000 | = |
| session_vs_persistent.longterm_memory_persistence | 0.000 | 0.000 | = |
| warm_context_delta.session1_items | 10 | 10 | = |
| warm_context_delta.cross_session_hit_rate | 0.000 | 0.000 | = |
| warm_context_delta.assembly_latency_ms | 138.174 | 47.240 | -90.934 |

### token-efficiency

| Metric | Value |
|--------|-------|
| context_l0 | avg_tokens_per_query=532.70, pct_of_full_context=2.05, vs_memori_ratio=0.41 |
| context_l1 | avg_tokens_per_query=885.30, pct_of_full_context=3.40, vs_memori_ratio=0.68 |
| context_l2 | avg_tokens_per_query=1808.40, pct_of_full_context=6.95, vs_memori_ratio=1.40 |
| recall_efficiency | avg_tokens_per_query=0.00, pct_of_full_context=0.00, vs_memori_ratio=0.00 |

#### token_efficiency Competitors

| Competitor | Tokens/Query | Cost/Query | Context % | Notes |
|-----------|-------------|-----------|-----------|-------|
| Full-Context | 26031 | $0.020825 | 100.0% | Unsustainable at scale. |
| Zep | 3911 | $0.003129 | 15.02% |  |
| Mem0 | 1764 | $0.001411 | 6.78% |  |
| Memori | 1294 | $0.001035 | 4.97% | Best accuracy/token ratio. |

#### vs Previous Version

| Metric | Prev | Current | Delta |
|--------|------|---------|-------|
| context_l0.avg_tokens_per_query | 350.000 | 532.700 | +182.700 |
| context_l0.pct_of_full_context | 1.340 | 2.050 | +0.710 |
| context_l0.vs_memori_ratio | 0.270 | 0.410 | +0.140 |
| context_l1.avg_tokens_per_query | 575.800 | 885.300 | +309.500 |
| context_l1.pct_of_full_context | 2.210 | 3.400 | +1.190 |
| context_l1.vs_memori_ratio | 0.440 | 0.680 | +0.240 |
| context_l2.avg_tokens_per_query | 1689.800 | 1808.400 | +118.600 |
| context_l2.pct_of_full_context | 6.490 | 6.950 | +0.460 |
| context_l2.vs_memori_ratio | 1.310 | 1.400 | +0.090 |
| recall_efficiency.avg_tokens_per_query | 0.000 | 0.000 | = |
| recall_efficiency.pct_of_full_context | 0.000 | 0.000 | = |
| recall_efficiency.vs_memori_ratio | 0.000 | 0.000 | = |

## 3. Competitor Analysis

### Memory Systems

#### Full-context GPT-4o (ceiling)
- **Score**: 60.2% (LongMemEval)
- **Per-category**: User 81.4%, Asst 94.6%, Pref 20.0%, Update 78.2%, Temporal 45.1%
- **Architecture**: Brute-force: passes entire conversation history to GPT-4o
- **Key insight**: Even with unlimited context, 60% is the ceiling — retrieval is necessary
- **What we learn**: Full-context is not the answer; smart retrieval beats brute force
- **Plico gap**: Our retrieval-based approach is correct in principle; the gap is in retrieval quality

#### Hindsight (Gemini-3)
- **Score**: 91.4% (LongMemEval)
- **Per-category**: User 97.1%, Asst 96.4%, Pref 80.0%, Update 94.9%, Temporal 87.2%
- **Architecture**: Local PostgreSQL + structured memory with explicit capability areas
- **Key insight**: Per-capability scoring reveals where memory systems excel vs struggle
- **What we learn**: Track temporal/multi-hop/preference accuracy separately; preference (80%) and temporal (87%) are hardest
- **Plico gap**: We don't track per-capability accuracy; F1 metric masks category-specific weaknesses

#### Hindsight (OSS-20B)
- **Score**: 83.6% (LongMemEval)
- **Per-category**: User 92.9%, Asst 80.4%, Pref 56.7%, Update 83.3%, Temporal 62.4%
- **Architecture**: Same Hindsight architecture with smaller 20B model
- **Key insight**: +44.6pp improvement over full-context baseline proves architecture > model size
- **What we learn**: Invest in memory architecture, not just bigger models
- **Plico gap**: Our Qwen3-0.6B is even smaller; architecture quality is the lever

#### OMEGA
- **Score**: 95.4% (LongMemEval)
- **Per-category**: User 99.2%, Asst 99.2%, Pref 100.0%, Update 96.2%, Temporal 94.0%
- **Architecture**: 6-stage search pipeline (retrieve → rerank → filter → merge → deduplicate → answer), bge-small ONNX
- **Key insight**: Multi-stage search dramatically outperforms single-pass retrieval
- **What we learn**: Add reranking stage to search; multi-session reasoning (83.5%) is the hardest category
- **Plico gap**: Our single-pass BM25+HNSW lacks reranking and multi-stage refinement

#### Supermemory (Gemini-3)
- **Score**: 85.2% (LongMemEval)
- **Architecture**: Experimental pipeline, cloud-dependent
- **Key insight**: 85% with Gemini-3 shows cloud models can compensate for architecture gaps
- **What we learn**: Cloud API quality can partially offset architecture limitations
- **Plico gap**: We are self-hosted; need architecture quality to compensate for smaller models

#### Supermemory (GPT-4o)
- **Score**: 81.6% (LongMemEval)
- **Architecture**: Experimental pipeline with GPT-4o
- **Key insight**: 4pp drop from Gemini-3 to GPT-4o shows model quality matters within same architecture
- **What we learn**: Model choice affects accuracy even with identical pipeline
- **Plico gap**: Our LLM (Gemma 4 26B) is smaller; need architecture to compensate

#### Mastra Observational Memory
- **Score**: 94.87% (LongMemEval)
- **Architecture**: Two background agents (Observer + Reflector) maintain dense observation log; stable context window enables prompt caching
- **Key insight**: 94.87% with gpt-5-mini — observation-based memory achieves near-OMEGA scores with simpler architecture
- **What we learn**: Background observation agents can replace complex retrieval pipelines; stable context enables prompt caching
- **Plico gap**: Our skill forge is conceptually similar but not benchmarked for memory quality

#### Supermemory (experimental)
- **Score**: 99.0% (LongMemEval)
- **Architecture**: Experimental pipeline, details not fully disclosed
- **Key insight**: 99% suggests near-perfect memory is achievable with sufficient engineering
- **What we learn**: The ceiling for memory systems is higher than OMEGA's 95.4% suggested
- **Plico gap**: Gap is enormous; need systematic improvement across all capability areas

#### Mem0 (new algorithm)
- **Score**: 93.4% (LongMemEval)
- **Architecture**: New memory algorithm with structured extraction, 6.8K tokens/query
- **Key insight**: 93.4% — now competitive with OMEGA (95.4%) and Mastra (94.87%)
- **What we learn**: Mem0's dramatic improvement shows the memory algorithm space is still rapidly evolving
- **Plico gap**: 93.4% vs our F1-based metrics — need to adopt accuracy_pct evaluation to compare

#### Full-Context (ceiling)
- **Score**: 87.52% (LoCoMo)
- **Per-category**: 1-Hop 88.53%, M-Hop 77.7%, Open 71.88%, Temporal 92.7%
- **Architecture**: Brute-force: passes entire conversation history to LLM
- **Key insight**: 87.52% ceiling with 26K tokens/query — retrieval systems can approach this at 5% token cost
- **What we learn**: The gap between full-context (87.52%) and best retrieval (81.95%) is only 5.57pp — retrieval is viable
- **Plico gap**: Our accuracy_pct needs to reach ~82% to match Memori; current F1-based metrics aren't comparable

#### Memori
- **Score**: 81.95% (LoCoMo)
- **Per-category**: 1-Hop 87.87%, M-Hop 72.7%, Open 63.54%, Temporal 80.37%
- **Architecture**: Structured memory extraction + semantic retrieval, 1,294 tokens/query
- **Key insight**: Best accuracy/token ratio: 81.95% at 4.97% of full context cost
- **What we learn**: Structured memory extraction beats raw retrieval; open_domain (63.54%) is weakest category
- **Plico gap**: Our token efficiency (L0/L1/L2) is competitive; accuracy gap is in retrieval quality

#### Zep
- **Score**: 79.09% (LoCoMo)
- **Per-category**: 1-Hop 79.43%, M-Hop 69.16%, Open 73.96%, Temporal 83.33%
- **Architecture**: Graph-based memory with temporal awareness, 3,911 tokens/query
- **Key insight**: Best open_domain (73.96%) among retrieval systems — graph structure helps cross-topic queries
- **What we learn**: Graph-based memory improves open-domain retrieval; 3x token cost vs Memori for 2.86pp less accuracy
- **Plico gap**: Our KG (redb, 17 edge types) is more sophisticated; should leverage it for open-domain queries

#### LangMem
- **Score**: 78.05% (LoCoMo)
- **Per-category**: 1-Hop 74.47%, M-Hop 61.06%, Open 67.71%, Temporal 86.92%
- **Architecture**: LangChain-integrated memory with temporal indexing
- **Key insight**: Best temporal reasoning (86.92%) among retrieval systems — temporal indexing works
- **What we learn**: Dedicated temporal indexing dramatically improves time-based queries; weakest at multi_hop (61.06%)
- **Plico gap**: Our temporal_reasoning suite should target 87%+; KG causal edges could enable multi-hop improvement

#### Mem0 (legacy)
- **Score**: 62.47% (LoCoMo)
- **Per-category**: 1-Hop 62.41%, M-Hop 57.32%, Open 44.79%, Temporal 66.47%
- **Architecture**: Simple extraction-based memory, minimal structuring
- **Key insight**: 62.47% overall — naive extraction without structure performs poorly across all categories
- **What we learn**: Memory architecture quality matters enormously: 19.48pp gap to Memori with same underlying models
- **Plico gap**: Our 4-layer memory (Ephemeral→Working→Long-term→Procedural) should far exceed Mem0's flat approach

#### Mem0 (new algorithm)
- **Score**: 91.6% (LoCoMo)
- **Architecture**: New memory algorithm with structured extraction + semantic retrieval, 7K tokens/query
- **Key insight**: 29pp improvement (62.47%→91.6%) proves algorithm quality matters more than incremental tuning
- **What we learn**: A single algorithm redesign can dramatically close the gap; don't assume competitors are static
- **Plico gap**: Now 91.6% vs our F1-based metrics — the gap is enormous; need accuracy_pct comparable to LoCoMo

#### EverMind HyperMem
- **Score**: 92.73% (LoCoMo)
- **Architecture**: Hypergraph hierarchical memory (HyperMem), part of EverOS self-evolving agent framework
- **Key insight**: 92.73% with hypergraph structure — graph-based memory outperforms flat retrieval
- **What we learn**: Hierarchical graph memory is the current state-of-the-art; our KG could serve similar role
- **Plico gap**: Our redb KG with 17 edge types is more sophisticated but not benchmarked for memory quality

#### delta-mem
- **Score**: None% (LoCoMo)
- **Architecture**: Lightweight memory mechanism (0.12% of backbone params), TTL-based subtask
- **Key insight**: 1.31x on MemoryAgentBench, 1.20x on LoCoMo — minimal parameters, maximal improvement
- **What we learn**: Architectural innovation matters more than scale; lightweight mechanisms can dramatically improve memory
- **Plico gap**: Our memory architecture is already sophisticated; need to measure improvement delta

#### Tencent AgentMemory
- **Score**: 76.1% (PersonaMem)
- **Architecture**: Production-grade agent memory with structured user profiles
- **Key insight**: 76.10% on realistic multi-profile scenario — production memory is harder than single-benchmark
- **What we learn**: Multi-profile scenarios expose weaknesses that single-profile benchmarks miss
- **Plico gap**: We haven't tested multi-profile scenarios; scope isolation tests are basic

#### EverMind (EverOS)
- **Score**: None% (PersonaMem)
- **Architecture**: Open-source memory layer, hierarchical storage
- **Key insight**: Open-source competitor to Tencent's proprietary solution
- **What we learn**: Open-source memory layers are competitive; ecosystem is maturing
- **Plico gap**: Our 4-layer memory is more granular; need to benchmark on PersonaMem

### Embedding Models

#### Gemini Embedding 001
- **Score**: MTEB 68.32 (MTEB)
- **Architecture**: Google's proprietary embedding model, 3072 dims, API-only
- **Key insight**: MTEB #1 but retrieval-specific score (67.71) is lower than general MTEB avg — retrieval is a distinct skill
- **What we learn**: High MTEB avg doesn't guarantee high retrieval; optimize for retrieval-specific metrics
- **Plico gap**: We use Qwen3-0.6B (1024 dims); Gemini's 3x dimension advantage shows in MTEB scores

#### Qwen3-Embedding-8B
- **Score**: MTEB 70.58 (MTEB)
- **Architecture**: Qwen3 family, 8B params, 4096 dims, open-weight, vLLM/SGLang support
- **Key insight**: Best self-hosted model at 70.58 MTEB — proves open-weight can compete with proprietary
- **What we learn**: Qwen3 family scales well: 0.6B→8B gives significant MTEB improvement; same family simplifies migration
- **Plico gap**: Our Qwen3-0.6B is 13x smaller; upgrading to 8B would require GPU but significantly improve retrieval

#### NV-Embed-v2
- **Score**: MTEB 72.31 (MTEB)
- **Architecture**: NVIDIA's embedding model, 4096 dims, legacy MTEB (56 tasks)
- **Key insight**: Highest MTEB avg (72.31) but retrieval score (62.65) is mediocre — general vs retrieval tradeoff
- **What we learn**: MTEB leaderboard position doesn't predict retrieval quality; always benchmark retrieval separately
- **Plico gap**: Our retrieval@5 metric is the right evaluation; MTEB avg alone is misleading

#### BGE-en-ICL
- **Score**: MTEB 71.24 (MTEB)
- **Architecture**: BAAI's in-context learning embedding, 4096 dims, legacy MTEB
- **Key insight**: 71.24 MTEB with in-context learning approach — shows prompt engineering works for embeddings
- **What we learn**: In-context learning embeddings can achieve high MTEB scores; consider prompt-based embedding strategies
- **Plico gap**: Our embedding model doesn't use in-context learning; adding query-time context could improve scores

#### Voyage-3.1-large
- **Score**: MTEB 67.4 (MTEB)
- **Architecture**: Voyage AI's flagship, 2048 dims, API-only, $0.05/M tokens
- **Key insight**: Best non-Google API at 67.40 MTEB — proves API market is competitive
- **What we learn**: API-based embeddings have cost implications at scale; self-hosted is more economical for high-volume
- **Plico gap**: Self-hosted Qwen3-0.6B eliminates per-query embedding cost; accuracy gap is acceptable tradeoff

#### Jina Embeddings v4
- **Score**: MTEB 66.81 (MTEB)
- **Architecture**: Jina's multimodal embedding, 1024 dims, native text+image, 32K context
- **Key insight**: Multimodal embedding at 66.81 MTEB — image+text in single vector enables cross-modal retrieval
- **What we learn**: Multimodal embeddings are mature enough for production; 32K context is significant for long documents
- **Plico gap**: We're text-only; multimodal could be a future capability for image/document understanding

#### BGE-M3
- **Score**: MTEB 63.0 (MTEB)
- **Architecture**: BAAI's multi-vector model: Dense + Sparse + ColBERT, 1024 dims
- **Key insight**: Multi-representation (Dense+Sparse+ColBERT) enables hybrid search strategies
- **What we learn**: Sparse vectors complement dense for keyword-heavy queries; ColBERT enables late interaction
- **Plico gap**: Our BM25+HNSW hybrid is similar in spirit; ColBERT-style late interaction could improve re-ranking

#### text-embedding-3-large
- **Score**: MTEB 64.6 (MTEB)
- **Architecture**: OpenAI's large embedding, 3072 dims, API-only, $0.13/M tokens
- **Key insight**: 64.60 MTEB at $0.13/M tokens — expensive but below open-weight leaders
- **What we learn**: OpenAI embeddings are not the best value; open-weight models outperform at zero marginal cost
- **Plico gap**: Self-hosted Qwen3 eliminates per-query cost; accuracy gap is manageable with architecture improvements

#### text-embedding-3-small
- **Score**: MTEB 62.26 (MTEB)
- **Architecture**: OpenAI's small embedding, 1536 dims, API-only, $0.02/M tokens
- **Key insight**: Cheapest API option at 62.26 MTEB — cost/quality tradeoff for budget-conscious deployments
- **What we learn**: Budget API embeddings underperform open-weight models; self-hosted is better value
- **Plico gap**: Our Qwen3-0.6B likely matches or exceeds this at zero marginal cost

#### Microsoft Harrier
- **Score**: MTEB None (MTEB)
- **Architecture**: Microsoft's multilingual embedding, open-sourced, MTEB-v2 #1
- **Key insight**: New MTEB-v2 leaderboard favors multilingual models; Harrier displaced previous leaders
- **What we learn**: Multilingual capability is increasingly important; monolingual models may lose leaderboard position
- **Plico gap**: We use Qwen3-0.6B which is multilingual but much smaller; Harrier shows what's possible

#### KaLM-Embedding-Gemma3-12B-2511
- **Score**: MTEB None (MTEB)
- **Architecture**: Tencent's 12B embedding model, Gemma3-based, MTEB multilingual #1
- **Key insight**: 12B parameters shows scale still matters for embedding quality
- **What we learn**: Large embedding models (12B) can achieve top scores but require significant compute
- **Plico gap**: Our 0.6B model is 20x smaller; architecture quality must compensate for scale

### Token Efficiency

#### Full-Context
- **Score**: 26031 tokens/query (Token Efficiency)
- **Architecture**: Brute-force: entire conversation in context window
- **Key insight**: 26K tokens/query at $0.02/query — unsustainable beyond prototype scale
- **What we learn**: Token efficiency is a production requirement, not an optimization
- **Plico gap**: Our L0/L1/L2 layering targets <1K tokens for common queries

#### Zep
- **Score**: 3911 tokens/query (Token Efficiency)
- **Architecture**: Graph-based retrieval, 3,911 tokens/query
- **Key insight**: 3.9K tokens is 85% reduction from full-context but 3x Memori's cost
- **What we learn**: Graph retrieval adds token overhead; need to balance graph richness with token budget
- **Plico gap**: Our KG-based retrieval must stay under 2K tokens to beat Memori's efficiency

#### Mem0
- **Score**: 1764 tokens/query (Token Efficiency)
- **Architecture**: Simple extraction, 1,764 tokens/query
- **Key insight**: 1.7K tokens is efficient but accuracy (62.47%) shows extraction quality matters more than token count
- **What we learn**: Low token count without quality extraction is false economy
- **Plico gap**: Our token efficiency should match Mem0's count but with much higher accuracy

#### Memori
- **Score**: 1294 tokens/query (Token Efficiency)
- **Architecture**: Structured memory extraction, 1,294 tokens/query
- **Key insight**: Best accuracy/token: 81.95% at 1.3K tokens — the benchmark to beat
- **What we learn**: Structured memory extraction achieves 5% context footprint with minimal accuracy loss
- **Plico gap**: Our L0+L1 combined should target <1.3K tokens while maintaining search quality

### Agent Frameworks

#### LangChain/LangGraph
- **Stars**: 90000
- **Memory layers**: 1
- **KG native**: False
- **Scope isolation**: per-thread checkpoint
- **Strengths**: Ecosystem (100+ integrations), LangSmith observability
- **Weaknesses**: No native memory/KG, Python-only, no proactive optimization

#### CrewAI
- **Stars**: 25000
- **Memory layers**: 1
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: Multi-agent role-based collaboration
- **Weaknesses**: No memory persistence, no KG, no sessions

#### AutoGen
- **Stars**: 40000
- **Memory layers**: 1
- **KG native**: False
- **Scope isolation**: per-conversation
- **Strengths**: Microsoft backing, multi-agent conversation
- **Weaknesses**: No persistent memory, no KG, no proactive optimization

#### Letta/MemGPT
- **Stars**: 15000
- **Memory layers**: 3
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: Self-editing memory, multi-model portability, good UX
- **Weaknesses**: No KG, no WASM, Python-speed (~10ms recall)

#### AIOS
- **Stars**: 5680
- **Memory layers**: 1
- **KG native**: False
- **Scope isolation**: none
- **Strengths**: Agent SDK, scheduling abstraction
- **Weaknesses**: Monolithic Python, no memory layers, no KG, no sessions

#### Mastra
- **Stars**: None
- **Memory layers**: 2
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: 94.87% on LongMemEval, open-source, prompt-cacheable architecture
- **Weaknesses**: TypeScript-only, no native KG, no WASM skills

#### Cloudflare Agent Memory
- **Stars**: None
- **Memory layers**: 4
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: SHA-256 content addressing, 4 memory types (facts/events/instructions/tasks), production infrastructure
- **Weaknesses**: Private beta, no KG, no WASM, cloud-dependent

#### MemoryOS (BAI-LAB)
- **Stars**: None
- **Memory layers**: 4
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: EMNLP 2025 Oral, 4W taxonomy (What/When/Where/Why), hierarchical storage
- **Weaknesses**: Python-only, no KG, no WASM, research prototype

#### TencentDB Agent Memory
- **Stars**: None
- **Memory layers**: 4
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: 4-layer progressive architecture (L0 raw→L1 atomic→L2 scene→L3 profile), 61% token reduction, MIT license, OpenClaw/Hermes compatible
- **Weaknesses**: Python-only, no KG, no WASM, cloud-oriented

#### agentmemory (rohitg00)
- **Stars**: 6247
- **Memory layers**: 1
- **KG native**: True
- **Scope isolation**: per-agent
- **Strengths**: Fastest growing (400 stars/day), hybrid search (BM25+vector+KG), targets coding agents (Claude Code/Cursor/Gemini CLI)
- **Weaknesses**: No scope isolation, no WASM, no token efficiency, coding-agent focused

#### NevaMind memU
- **Stars**: 13100
- **Memory layers**: None
- **KG native**: False
- **Scope isolation**: per-agent
- **Strengths**: 13.1K stars, 24/7 proactive agents, reduces LLM overhead for memory operations
- **Weaknesses**: Python-only, no KG, no WASM, details sparse

#### EverMind/EverOS
- **Stars**: None
- **Memory layers**: None
- **KG native**: True
- **Scope isolation**: per-agent
- **Strengths**: ACL 2026, HyperMem (92.73% LoCoMo), Skills Evolution Engine, 234.8% improvement on complex tasks
- **Weaknesses**: Python-only, research prototype, no WASM


## 4. Agent Framework Comparison

| Feature | LangChain/LangGraph | CrewAI | AutoGen | Letta/MemGPT | AIOS | Mastra | Cloudflare Agent Memory | MemoryOS (BAI-LAB) | TencentDB Agent Memory | agentmemory (rohitg00) | NevaMind memU | EverMind/EverOS | Plico (v49) |
|---------|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|----- |
| Language | Python | Python | Python | Python | Python | TypeScript | TypeScript/Python | Python | Python | TypeScript + Rust | Python | Python | Rust |
| Memory Layers | 1 | 1 | 1 | 3 | 1 | 2 | 4 | 4 | 4 | 1 | None | None | 4 |
| Scope Isolation | per-thread checkpoint | per-agent | per-conversation | per-agent | none | per-agent | per-agent | per-agent | per-agent | per-agent | per-agent | per-agent | Private/Shared/Group (OS-enforced) |
| KG Native | No | No | No | No | No | No | No | No | No | Yes | No | Yes | Yes |
| Semantic Search | external (Pinecone/Chroma/etc) | none | none | none | none | external | RRF-based retrieval | hierarchical retrieval | context offloading + structured task graph | BM25 + vector + KG hybrid | external | hypergraph hierarchical retrieval | native BM25+HNSW hybrid |
| Session Mgmt | checkpoint per thread | none | conversation history | persistent agent state | none | observation log | persistent agent state | hierarchical storage | progressive L0-L3 architecture | auto-record from sessions | persistent agent memory | self-evolving agent memory | OS session + warm context + delta |
| Proactive Optim | No | No | No | memory archival triggers | No | Observer + Reflector background agents | No | No | Mermaid Task Canvas | zero manual maintenance | reduces LLM calls for memory management | Skills Evolution Engine — auto-distills skills from interactions | intent prefetch + skill forge + self-healing |
| WASM Skills | No | No | No | No | No | No | No | No | No | No | No | No | Yes |
| Token Efficiency | No | No | No | basic | No | stable context window (prompt-cacheable) | No | No | 61% token reduction | No | No | No | L0/L1/L2 layered context |

## 5. Soul Alignment Score

| # | Axiom | Suite(s) | Status | Score |
|---|-------|----------|--------|-------|
| A1 | token_scarcity | token-efficiency | 533 tok/query (under Memori) | 2/2 |
| A2 | intent_before_action | intent-routing, conversational-qa | accuracy 37.5% | 0/2 |
| A3 | memory_exoskeleton | memory-lifecycle | CRUD 1.0% | 0/2 |
| A4 | sharing_before_duplication | scope-isolation | zero leaks | 2/2 |
| A5 | mechanism_not_strategy | performance | measured | 1/2 |
| A6 | semantics_before_structure | retrieval | beir_scifact recall@5 0.627 | 1/2 |
| A7 | proactive_before_passive | proactive-optimization | L0 113 tok, 100% hit | 2/2 |
| A8 | causality_before_correlation | causal-reasoning | causal retrieval 0% | 0/2 |
| A9 | gets_better | memory-lifecycle | persistence 100% | 2/2 |
| A10 | session_first_class | session-lifecycle | session success 100% | 2/2 |
| | **Total** | | | **12/20** |

## 6. Key Learnings from Competitors

### From Memory Specialists

| Competitor | Key Insight | What We Learn | Plico Gap |
|-----------|------------|---------------|-----------|
| Full-context GPT-4o (ceiling) | Even with unlimited context, 60% is the ceiling — retrieval is necessary | Full-context is not the answer; smart retrieval beats brute force | Our retrieval-based approach is correct in principle; the gap is in retrieval quality |
| Hindsight (Gemini-3) | Per-capability scoring reveals where memory systems excel vs struggle | Track temporal/multi-hop/preference accuracy separately; preference (80%) and temporal (87%) are hardest | We don't track per-capability accuracy; F1 metric masks category-specific weaknesses |
| Hindsight (OSS-20B) | +44.6pp improvement over full-context baseline proves architecture > model size | Invest in memory architecture, not just bigger models | Our Qwen3-0.6B is even smaller; architecture quality is the lever |
| OMEGA | Multi-stage search dramatically outperforms single-pass retrieval | Add reranking stage to search; multi-session reasoning (83.5%) is the hardest category | Our single-pass BM25+HNSW lacks reranking and multi-stage refinement |
| Supermemory (Gemini-3) | 85% with Gemini-3 shows cloud models can compensate for architecture gaps | Cloud API quality can partially offset architecture limitations | We are self-hosted; need architecture quality to compensate for smaller models |
| Supermemory (GPT-4o) | 4pp drop from Gemini-3 to GPT-4o shows model quality matters within same architecture | Model choice affects accuracy even with identical pipeline | Our LLM (Gemma 4 26B) is smaller; need architecture to compensate |
| Mastra Observational Memory | 94.87% with gpt-5-mini — observation-based memory achieves near-OMEGA scores with simpler architecture | Background observation agents can replace complex retrieval pipelines; stable context enables prompt caching | Our skill forge is conceptually similar but not benchmarked for memory quality |
| Supermemory (experimental) | 99% suggests near-perfect memory is achievable with sufficient engineering | The ceiling for memory systems is higher than OMEGA's 95.4% suggested | Gap is enormous; need systematic improvement across all capability areas |
| Mem0 (new algorithm) | 93.4% — now competitive with OMEGA (95.4%) and Mastra (94.87%) | Mem0's dramatic improvement shows the memory algorithm space is still rapidly evolving | 93.4% vs our F1-based metrics — need to adopt accuracy_pct evaluation to compare |
| Full-Context (ceiling) | 87.52% ceiling with 26K tokens/query — retrieval systems can approach this at 5% token cost | The gap between full-context (87.52%) and best retrieval (81.95%) is only 5.57pp — retrieval is viable | Our accuracy_pct needs to reach ~82% to match Memori; current F1-based metrics aren't comparable |
| Memori | Best accuracy/token ratio: 81.95% at 4.97% of full context cost | Structured memory extraction beats raw retrieval; open_domain (63.54%) is weakest category | Our token efficiency (L0/L1/L2) is competitive; accuracy gap is in retrieval quality |
| Zep | Best open_domain (73.96%) among retrieval systems — graph structure helps cross-topic queries | Graph-based memory improves open-domain retrieval; 3x token cost vs Memori for 2.86pp less accuracy | Our KG (redb, 17 edge types) is more sophisticated; should leverage it for open-domain queries |
| LangMem | Best temporal reasoning (86.92%) among retrieval systems — temporal indexing works | Dedicated temporal indexing dramatically improves time-based queries; weakest at multi_hop (61.06%) | Our temporal_reasoning suite should target 87%+; KG causal edges could enable multi-hop improvement |
| Mem0 (legacy) | 62.47% overall — naive extraction without structure performs poorly across all categories | Memory architecture quality matters enormously: 19.48pp gap to Memori with same underlying models | Our 4-layer memory (Ephemeral→Working→Long-term→Procedural) should far exceed Mem0's flat approach |
| Mem0 (new algorithm) | 29pp improvement (62.47%→91.6%) proves algorithm quality matters more than incremental tuning | A single algorithm redesign can dramatically close the gap; don't assume competitors are static | Now 91.6% vs our F1-based metrics — the gap is enormous; need accuracy_pct comparable to LoCoMo |
| EverMind HyperMem | 92.73% with hypergraph structure — graph-based memory outperforms flat retrieval | Hierarchical graph memory is the current state-of-the-art; our KG could serve similar role | Our redb KG with 17 edge types is more sophisticated but not benchmarked for memory quality |
| delta-mem | 1.31x on MemoryAgentBench, 1.20x on LoCoMo — minimal parameters, maximal improvement | Architectural innovation matters more than scale; lightweight mechanisms can dramatically improve memory | Our memory architecture is already sophisticated; need to measure improvement delta |

### From Embedding Models

| Model | Key Insight | What We Learn | Plico Gap |
|-------|------------|---------------|-----------|
| Gemini Embedding 001 | MTEB #1 but retrieval-specific score (67.71) is lower than general MTEB avg — retrieval is a distinct skill | High MTEB avg doesn't guarantee high retrieval; optimize for retrieval-specific metrics | We use Qwen3-0.6B (1024 dims); Gemini's 3x dimension advantage shows in MTEB scores |
| Qwen3-Embedding-8B | Best self-hosted model at 70.58 MTEB — proves open-weight can compete with proprietary | Qwen3 family scales well: 0.6B→8B gives significant MTEB improvement; same family simplifies migration | Our Qwen3-0.6B is 13x smaller; upgrading to 8B would require GPU but significantly improve retrieval |
| NV-Embed-v2 | Highest MTEB avg (72.31) but retrieval score (62.65) is mediocre — general vs retrieval tradeoff | MTEB leaderboard position doesn't predict retrieval quality; always benchmark retrieval separately | Our retrieval@5 metric is the right evaluation; MTEB avg alone is misleading |
| BGE-en-ICL | 71.24 MTEB with in-context learning approach — shows prompt engineering works for embeddings | In-context learning embeddings can achieve high MTEB scores; consider prompt-based embedding strategies | Our embedding model doesn't use in-context learning; adding query-time context could improve scores |
| Voyage-3.1-large | Best non-Google API at 67.40 MTEB — proves API market is competitive | API-based embeddings have cost implications at scale; self-hosted is more economical for high-volume | Self-hosted Qwen3-0.6B eliminates per-query embedding cost; accuracy gap is acceptable tradeoff |
| Jina Embeddings v4 | Multimodal embedding at 66.81 MTEB — image+text in single vector enables cross-modal retrieval | Multimodal embeddings are mature enough for production; 32K context is significant for long documents | We're text-only; multimodal could be a future capability for image/document understanding |
| BGE-M3 | Multi-representation (Dense+Sparse+ColBERT) enables hybrid search strategies | Sparse vectors complement dense for keyword-heavy queries; ColBERT enables late interaction | Our BM25+HNSW hybrid is similar in spirit; ColBERT-style late interaction could improve re-ranking |
| text-embedding-3-large | 64.60 MTEB at $0.13/M tokens — expensive but below open-weight leaders | OpenAI embeddings are not the best value; open-weight models outperform at zero marginal cost | Self-hosted Qwen3 eliminates per-query cost; accuracy gap is manageable with architecture improvements |
| text-embedding-3-small | Cheapest API option at 62.26 MTEB — cost/quality tradeoff for budget-conscious deployments | Budget API embeddings underperform open-weight models; self-hosted is better value | Our Qwen3-0.6B likely matches or exceeds this at zero marginal cost |
| Microsoft Harrier | New MTEB-v2 leaderboard favors multilingual models; Harrier displaced previous leaders | Multilingual capability is increasingly important; monolingual models may lose leaderboard position | We use Qwen3-0.6B which is multilingual but much smaller; Harrier shows what's possible |
| KaLM-Embedding-Gemma3-12B-2511 | 12B parameters shows scale still matters for embedding quality | Large embedding models (12B) can achieve top scores but require significant compute | Our 0.6B model is 20x smaller; architecture quality must compensate for scale |

### From Token Efficiency

| Competitor | Key Insight | What We Learn | Plico Gap |
|-----------|------------|---------------|-----------|
| Full-Context | 26K tokens/query at $0.02/query — unsustainable beyond prototype scale | Token efficiency is a production requirement, not an optimization | Our L0/L1/L2 layering targets <1K tokens for common queries |
| Zep | 3.9K tokens is 85% reduction from full-context but 3x Memori's cost | Graph retrieval adds token overhead; need to balance graph richness with token budget | Our KG-based retrieval must stay under 2K tokens to beat Memori's efficiency |
| Mem0 | 1.7K tokens is efficient but accuracy (62.47%) shows extraction quality matters more than token count | Low token count without quality extraction is false economy | Our token efficiency should match Mem0's count but with much higher accuracy |
| Memori | Best accuracy/token: 81.95% at 1.3K tokens — the benchmark to beat | Structured memory extraction achieves 5% context footprint with minimal accuracy loss | Our L0+L1 combined should target <1.3K tokens while maintaining search quality |

### From Agent Frameworks

| Framework | Strengths | Weaknesses |
|-----------|-----------|------------|
| LangChain/LangGraph | Ecosystem (100+ integrations), LangSmith observability | No native memory/KG, Python-only, no proactive optimization |
| CrewAI | Multi-agent role-based collaboration | No memory persistence, no KG, no sessions |
| AutoGen | Microsoft backing, multi-agent conversation | No persistent memory, no KG, no proactive optimization |
| Letta/MemGPT | Self-editing memory, multi-model portability, good UX | No KG, no WASM, Python-speed (~10ms recall) |
| AIOS | Agent SDK, scheduling abstraction | Monolithic Python, no memory layers, no KG, no sessions |
| Mastra | 94.87% on LongMemEval, open-source, prompt-cacheable architecture | TypeScript-only, no native KG, no WASM skills |
| Cloudflare Agent Memory | SHA-256 content addressing, 4 memory types (facts/events/instructions/tasks), production infrastructure | Private beta, no KG, no WASM, cloud-dependent |
| MemoryOS (BAI-LAB) | EMNLP 2025 Oral, 4W taxonomy (What/When/Where/Why), hierarchical storage | Python-only, no KG, no WASM, research prototype |
| TencentDB Agent Memory | 4-layer progressive architecture (L0 raw→L1 atomic→L2 scene→L3 profile), 61% token reduction, MIT license, OpenClaw/Hermes compatible | Python-only, no KG, no WASM, cloud-oriented |
| agentmemory (rohitg00) | Fastest growing (400 stars/day), hybrid search (BM25+vector+KG), targets coding agents (Claude Code/Cursor/Gemini CLI) | No scope isolation, no WASM, no token efficiency, coding-agent focused |
| NevaMind memU | 13.1K stars, 24/7 proactive agents, reduces LLM overhead for memory operations | Python-only, no KG, no WASM, details sparse |
| EverMind/EverOS | ACL 2026, HyperMem (92.73% LoCoMo), Skills Evolution Engine, 234.8% improvement on complex tasks | Python-only, research prototype, no WASM |

### RAGAS Production Targets

| Metric | Description | Production Baseline | Good Threshold |
|--------|------------|-------------------|----------------|
| Faithfulness | Are claims in the answer grounded in the retrieved context? | 0.85 | 0.9 |
| Answer Relevancy | Does the answer address the question asked? | 0.8 | 0.85 |
| Context Precision | Are the relevant items ranked higher in the context? | 0.65 | 0.75 |
| Context Recall | Does the retrieved context cover all claims in the ground truth? | 0.75 | 0.85 |

_RAGAS is the de-facto standard for RAG pipeline evaluation. These are production-grade targets._

### Cross-Benchmark References

**HotpotQA — multi-hop question answering over Wikipedia**

| System | Score | Notes |
|--------|-------|-------|
| DPR (Dense Passage Retrieval) | 67.5 |  |
| IRRR | 72.4 |  |
| Youtu-GraphRAG (Tencent) | ~72 | Agentic graph schema, ICLR 2026 |
- _Plico relevance: Our KG multi-hop reasoning (A8) is conceptually similar; HotpotQA measures retrieval+reasoning quality_

**AgentBench — comprehensive agent capability evaluation**

| System | Score | Notes |
|--------|-------|-------|
| GPT-4o | ~65 | Across 8 agent environments |
| Claude 3.5 Sonnet | ~60 | Competitive on coding/web agent tasks |
- _Plico relevance: Measures agent tool-use and planning; Plico's WASM skills and intent routing are related capabilities_

**BigBench-Hard — challenging reasoning tasks**

| System | Score | Notes |
|--------|-------|-------|
| GPT-4 (CoT) | ~83 | Chain-of-thought prompting |
| Claude 3 Opus (CoT) | ~86 | Best overall on BBH |
- _Plico relevance: Multi-step reasoning tasks; our causal reasoning (A8) and KG path finding are related_

