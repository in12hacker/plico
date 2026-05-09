# Plico 基线性能评测报告 (v38)

> 生成时间: 2026-05-08 12:21:34
> 评测系统: Plico (usearch HNSW cos f16 + all-MiniLM-L6-v2)
> 评测模型: Gemma-4-26B-A4B-it-Q4_K_M (Reader/Judge), Qwen2.5-7B-Instruct (A/B Judge)

## 维度 1: 会话记忆 — LoCoMo

### Gemma 单模型主基线
- 样本数: 1542
- F1 Score: **0.145**
- BLEU-1: **0.101**
- LLM Score: **3.46**
- Context Hit Rate: **0.99**
- 平均在线延迟: 4.78s
- 平均搜索延迟: 2.3208s

### Gemma Reader + Qwen Judge A/B 对照
- 样本数: 1542
- F1 Score: **0.143**
- BLEU-1: **0.099**
- LLM Score: **3.44**
- Context Hit Rate: **0.98**
- 平均在线延迟: 4.62s
- 平均搜索延迟: 2.1966s

## 维度 2: 长期记忆 — LongMemEval

### Gemma 单模型主基线
- 样本数: 500
- Exact Match (EM): **0.274**
- Accuracy@4+: **0.374**
- LLM Score: **2.56**
- Context Hit Rate: **0.73**
- 平均在线延迟: 3.69s
- 平均搜索延迟: 1.9263s

### Gemma Reader + Qwen Judge A/B 对照
- 样本数: 500
- Exact Match (EM): **0.232**
- Accuracy@4+: **0.248**
- LLM Score: **2.10**
- Context Hit Rate: **0.63**
- 平均在线延迟: 3.33s
- 平均搜索延迟: 1.9685s

## 维度 3: 多跳推理 — HotPotQA

- 样本数: 200
- Exact Match (EM): **0.380**
- F1 Score: **0.525**
- Supporting Fact F1: **0.447**
- 平均在线延迟: 1.40s

## 维度 4: 信息检索 — BEIR (SciFact)

- 样本数: 300
- nDCG@10: **0.678**
- Recall@5: **0.744**
- MAP: **0.631**

## 维度 5: 增量记忆 — MemoryAgentBench AR

- 文档数: 22
- 问题数: 2000
- Plico API 命中率: **0.0%**
- 离线向量 命中率: **0.0%**

*注：当前版本该数据集命中率为0，可能由于评测口径或数据格式不匹配导致，需进一步排查。*

## 维度 6: 知识图谱 — KG 多跳

- 问题数: 50
- 实体存储: 951
- 关系存储: 620
- 路径命中率: **20.3%** (61/300)
- LLM 抽取: True

## 维度 7: 性能 — 微基准

| 操作 | QPS | P50 (ms) | P95 (ms) | P99 (ms) |
|------|-----|----------|----------|----------|
| cas_write | 19 | 6.53 | 11.11 | 1594.02 |
| cas_read | 2043 | 0.45 | 0.7 | 2.59 |
| search | 13 | 75.92 | 88.13 | 93.09 |
| memory_store_recall | 4898 | 0.16 | 0.4 | — |
| kg_operations | — | — | — | — |

## 综合评估

### 与竞品对比

| 系统 | LongMemEval R@5 | 类型 | 特点 |
|------|----------------|------|------|
| **Plico** | **N/A** | AI-OS Kernel (Rust) | 本地优先, CAS+KG+4层记忆 |
| agentmemory | 95.2% | Memory Layer (TS) | 仅作离线纯检索参考 |
| MemPalace | 96.6% | Vector Only | 仅作离线纯检索参考 |
| OMEGA | 95.4% (QA) | Memory Server (Python) | Local-first, SQLite |
| Zep/Graphiti | 63.8% (QA) | Temporal KG (Python) | 时间推理 |
| Mem0 | 49.0% (QA) | Cloud Memory (Python) | 即插即用 |

### 瓶颈识别

1. **最弱题型**: single-session-user (待确认)
2. **检索质量**: 待确认

### 改进路线

1. **短期**: 调优 usearch 参数提升 recall
2. **中期**: 添加 BM25 混合检索（仿 agentmemory）
3. **长期**: 时间感知检索（仿 Zep/Graphiti temporal windows）
