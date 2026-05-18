# Plico Benchmark Report

> Generated: 2026-05-14 23:56:28

## conversational-qa

- **count**: 40
- **f1**: 0.212
- **bleu1**: 0.174
- **llm_score**: 2.300
- **context_hit_rate**: 1.000
- **mean**: 0.212
- **std**: 0.330
- **ci95_low**: 0.110
- **ci95_high**: 0.314

## kg-reasoning

- **n_nodes**: 50
- **avg_paths_unweighted**: 2.500
- **avg_paths_weighted**: 0.000
- **avg_latency_ms**: 0.212

## memory-crud

- **create**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 4.42}
- **read**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.34}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 90.0, 'avg_latency_ms': 10.97}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 42.81}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 1974.77}

## performance

- **cas_write**: {'qps': 24.1, 'p50_ms': 37.965564464684576, 'p95_ms': 51.37238079914823, 'p99_ms': 240.50341809983334}
- **search**: {'qps': 4989.9, 'p50_ms': 0.07245648885145783, 'p95_ms': 0.466373277595222, 'p99_ms': 1.9547642162069676}
- **memory_recall**: {'qps': 2291.5, 'p50_ms': 0.30787306604906917, 'p95_ms': 1.743259222712367, 'p99_ms': 2.0044933748431504}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## retrieval


## temporal-reasoning

- **count**: 30
- **f1**: 0.069
- **bleu1**: 0.029
- **llm_score**: 0.867
- **context_hit_rate**: 1.000
- **mean**: 0.069
- **std**: 0.061
- **ci95_low**: 0.047
- **ci95_high**: 0.091
