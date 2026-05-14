# Plico Benchmark Report

> Generated: 2026-05-14 00:41:01

## conversational-qa

- **count**: 40
- **f1**: 0.057
- **bleu1**: 0.046
- **llm_score**: 0.250
- **context_hit_rate**: 1.000
- **mean**: 0.057
- **std**: 0.188
- **ci95_low**: -0.001
- **ci95_high**: 0.116

## kg-reasoning

- **n_nodes**: 50
- **avg_paths_unweighted**: 2.500
- **avg_paths_weighted**: 0.000
- **avg_latency_ms**: 0.216

## kg-reasoning

- **n_nodes**: 10
- **avg_paths**: 0.000
- **avg_latency_ms**: 0.153

## memory-crud

- **create**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.9}
- **read**: {'count': 100, 'success_rate': 0.0, 'hit_rate': 0, 'avg_latency_ms': 0.24}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 0.0, 'avg_latency_ms': 140.52}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 7.08}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 170.46}

## memory-crud

- **create**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 3.27}
- **read**: {'count': 100, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 0.25}
- **search**: {'count': 20, 'success_rate': 0, 'hit_rate': 95.0, 'avg_latency_ms': 11.48}
- **update**: {'count': 20, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 3.05}
- **batch_create**: {'count': 50, 'success_rate': 100.0, 'hit_rate': 0, 'avg_latency_ms': 440.56}

## performance

- **cas_write**: {'qps': 200.1, 'p50_ms': 4.141532990615815, 'p95_ms': 9.222074603894725, 'p99_ms': 31.076837851433048}
- **search**: {'qps': 3121.6, 'p50_ms': 0.21958391880616546, 'p95_ms': 0.4654160526115447, 'p99_ms': 0.5193157203029838}
- **memory_recall**: {'qps': 3836.0, 'p50_ms': 0.23509602760896087, 'p95_ms': 0.43544067302718736, 'p99_ms': 1.9804287329316113}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## performance

- **cas_write**: {'qps': 231.6, 'p50_ms': 3.7892370019108057, 'p95_ms': 6.040371453855187, 'p99_ms': 11.123093944042825}
- **search**: {'qps': 51.7, 'p50_ms': 19.104388018604368, 'p95_ms': 22.48983219033107, 'p99_ms': 24.290487023536112}
- **memory_recall**: {'qps': 584.4, 'p50_ms': 1.7751060076989233, 'p95_ms': 2.137173316441475, 'p99_ms': 2.7894404786638907}
- **kg_path**: {'qps': None, 'p50_ms': None, 'p95_ms': None, 'p99_ms': None}

## retrieval


## temporal-reasoning

- **count**: 30
- **f1**: 0.069
- **bleu1**: 0.026
- **llm_score**: 0.833
- **context_hit_rate**: 0.000
- **mean**: 0.069
- **std**: 0.064
- **ci95_low**: 0.047
- **ci95_high**: 0.092

## temporal-reasoning

- **count**: 30
- **f1**: 0.069
- **bleu1**: 0.032
- **llm_score**: 0.733
- **context_hit_rate**: 1.000
- **mean**: 0.069
- **std**: 0.058
- **ci95_low**: 0.048
- **ci95_high**: 0.090
