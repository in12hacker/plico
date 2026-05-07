# Plico Benchmark 评测报告

> 生成时间: 2026-05-07 16:26:54
> 评测系统: Plico (usearch HNSW cos f16 + all-MiniLM-L6-v2)

## 维度 1: 检索质量 — LongMemEval-S Retrieval

| 指标 | Plico | agentmemory BM25+Vec | MemPalace Vec |
|------|-------|---------------------|---------------|
| R@5 | **92.4%** | 95.2% | 96.6% |
| R@10 | **96.8%** | 98.6% | ~97.6% |
| R@20 | **98.4%** | 99.4% | — |
| NDCG@10 | **82.5%** | 87.9% | — |
| MRR | **82.6%** | 88.2% | — |

按题目类型:

| 类型 | R@5 | R@10 | 数量 |
|------|-----|------|------|
| knowledge-update | 97.4% | 100.0% | 78 |
| multi-session | 94.7% | 99.2% | 133 |
| single-session-assistant | 98.2% | 98.2% | 56 |
| single-session-preference | 83.3% | 96.7% | 30 |
| single-session-user | 81.4% | 88.6% | 70 |
| temporal-reasoning | 92.5% | 96.2% | 133 |

- 嵌入模型: all-MiniLM-L6-v2
- 索引配置: usearch cos f16 (Plico-equivalent)
- 平均搜索延迟: 0.04ms

## 维度 2: 端到端记忆 — LongMemEval-S QA

### Retrieval 模式
- 准确率: **40.0%** (20/50)
- LLM: qwen2.5-coder-7b
- 在线延迟: ?s

| 类型 | 准确率 | 数量 |
|------|--------|------|
| single-session-user | 40.0% | 50 |


对标: Zep=63.8%, Mem0=49%, Oracle-GPT4o=82.4% (注：Plico 采用本地小模型，竞品采用 GPT-4o，仅作方向性参考)

## 维度 3: 增量记忆 — MemoryAgentBench AR

- 文档数: 20
- 问题数: 1880
- Plico API 命中率: **0.0%**
- 离线向量 命中率: **0.0%**

*注：当前版本该数据集命中率为0，可能由于评测口径或数据格式不匹配导致，需进一步排查。*

## 维度 4: 知识图谱 — KG 多跳

- 问题数: 50
- 实体存储: 0
- 关系存储: 0
- 路径命中率: **0.0%** (0/0)
- LLM 抽取: True

## 维度 5: 性能 — 微基准

| 操作 | QPS | P50 (ms) | P95 (ms) | P99 (ms) |
|------|-----|----------|----------|----------|
| cas_write | 15 | 7.99 | 17.14 | 2158.87 |
| cas_read | 2548 | 0.36 | 0.68 | 2.18 |
| search | 9 | 102.42 | 159.05 | 200.42 |
| memory_store_recall | 2123 | 0.43 | 0.91 | — |
| kg_operations | — | — | — | — |

## 综合评估

### 与竞品对比

| 系统 | LongMemEval R@5 | 类型 | 特点 |
|------|----------------|------|------|
| **Plico** | **92.4%** | AI-OS Kernel (Rust) | 本地优先, CAS+KG+4层记忆 |
| agentmemory | 95.2% | Memory Layer (TS) | 仅作离线纯检索参考 |
| MemPalace | 96.6% | Vector Only | 仅作离线纯检索参考 |
| OMEGA | 95.4% (QA) | Memory Server (Python) | Local-first, SQLite |
| Zep/Graphiti | 63.8% (QA) | Temporal KG (Python) | 时间推理 |
| Mem0 | 49.0% (QA) | Cloud Memory (Python) | 即插即用 |

### 瓶颈识别

1. **最弱题型**: single-session-user (R@5=81.4%)
2. **检索质量**: 低于 agentmemory 基线，建议:
   - 添加 BM25 混合检索
   - 调整 usearch 参数 (M, ef_construction, ef_search)
   - 尝试更高维度 embedding 模型

### 改进路线

1. **短期**: 调优 usearch 参数提升 recall
2. **中期**: 添加 BM25 混合检索（仿 agentmemory）
3. **长期**: 时间感知检索（仿 Zep/Graphiti temporal windows）
