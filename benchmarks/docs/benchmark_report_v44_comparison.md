# Plico Benchmark Report

> Generated: 2026-05-14 17:39:54

## conversational-qa

- **count**: 40
- **f1**: 0.206
- **bleu1**: 0.167
- **llm_score**: 2.325
- **context_hit_rate**: 1.000
- **mean**: 0.206
- **std**: 0.329
- **ci95_low**: 0.104
- **ci95_high**: 0.308

## conversational-qa

- **count**: 40
- **f1**: 0.220
- **bleu1**: 0.183
- **llm_score**: 2.400
- **context_hit_rate**: 1.000
- **mean**: 0.220
- **std**: 0.338
- **ci95_low**: 0.116
- **ci95_high**: 0.325

## conversational-qa

- **count**: 200
- **f1**: 0.232
- **bleu1**: 0.186
- **llm_score**: 2.565
- **context_hit_rate**: 1.000
- **mean**: 0.232
- **std**: 0.323
- **ci95_low**: 0.188
- **ci95_high**: 0.277

## kg-reasoning

- **n_nodes**: 50
- **avg_paths_unweighted**: 2.500
- **avg_paths_weighted**: 0.000
- **avg_latency_ms**: 0.486

## kg-reasoning

- **n_nodes**: 50
- **avg_paths_unweighted**: 2.500
- **avg_paths_weighted**: 0.000
- **avg_latency_ms**: 0.609

## kg-reasoning

- **n_nodes**: 50
- **avg_paths_unweighted**: 2.500
- **avg_paths_weighted**: 0.000
- **avg_latency_ms**: 0.543

## memory-crud

- **create**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 6.9}
- **read**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.4}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 100.0, 'avg_latency_ms': 10.28}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 32.21}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 2015.7}

## memory-crud

- **create**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 4.16}
- **read**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.64}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 85.0, 'avg_latency_ms': 11.83}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 33.31}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 1969.86}

## memory-crud

- **create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 69.56}
- **read**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.15}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 0.0, 'avg_latency_ms': 107.25}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 33.62}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 2591.97}

## performance

- **cas_write**: {'qps': 24.9, 'p50_ms': 36.34408552898094, 'p95_ms': 76.2456793454475, 'p99_ms': 139.1884470230434}
- **search**: {'qps': 3134.2, 'p50_ms': 0.10438601020723581, 'p95_ms': 1.7123445693869144, 'p99_ms': 1.9420306815300135}
- **memory_recall**: {'qps': 2213.4, 'p50_ms': 0.34172553569078445, 'p95_ms': 1.801428000908345, 'p99_ms': 1.997221193742007}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## performance

- **cas_write**: {'qps': 42.5, 'p50_ms': 24.917657021433115, 'p95_ms': 50.591276754857965, 'p99_ms': 61.843663859181106}
- **search**: {'qps': 2865.2, 'p50_ms': 0.11105556041002274, 'p95_ms': 1.6365327814128248, 'p99_ms': 1.857563810190186}
- **memory_recall**: {'qps': 3587.6, 'p50_ms': 0.11635199189186096, 'p95_ms': 1.6922247712500393, 'p99_ms': 1.832878419663757}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## performance

- **cas_write**: {'qps': 12.0, 'p50_ms': 40.515424974728376, 'p95_ms': 69.0464874904137, 'p99_ms': 2094.149297397816}
- **search**: {'qps': 1067.2, 'p50_ms': 0.13277598191052675, 'p95_ms': 1.7078536562621593, 'p99_ms': 1.8895316740963608}
- **memory_recall**: {'qps': 3455.6, 'p50_ms': 0.12096751015633345, 'p95_ms': 1.5956736227963118, 'p99_ms': 1.8027381098363549}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## retrieval


## retrieval


## retrieval


## temporal-reasoning

- **count**: 30
- **f1**: 0.069
- **bleu1**: 0.030
- **llm_score**: 0.833
- **context_hit_rate**: 1.000
- **mean**: 0.069
- **std**: 0.061
- **ci95_low**: 0.047
- **ci95_high**: 0.091

## temporal-reasoning

- **count**: 30
- **f1**: 0.073
- **bleu1**: 0.031
- **llm_score**: 0.633
- **context_hit_rate**: 1.000
- **mean**: 0.073
- **std**: 0.067
- **ci95_low**: 0.049
- **ci95_high**: 0.097

## temporal-reasoning

- **count**: 30
- **f1**: 0.092
- **bleu1**: 0.041
- **llm_score**: 0.767
- **context_hit_rate**: 1.000
- **mean**: 0.092
- **std**: 0.131
- **ci95_low**: 0.046
- **ci95_high**: 0.139
