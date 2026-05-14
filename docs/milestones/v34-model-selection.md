# Plico v34 里程碑报告

## 概述

v34 聚焦于 **模型选型 + 架构增强 + Ingest Pipeline**，目标是通过更好的模型和结构化 ingest 提升 LongMemEval 和 LoCoMo 表现。

## 完成的工作

### 1. Embedding 模型选型 — Qwen3-Embedding-0.6B

**A/B 测试结果：**

| 维度 | v5-small-retrieval | Qwen3-Embedding-0.6B |
|------|-------------------|---------------------|
| 隐式偏好检测 | 0.479 | **0.618** (+29%) |
| 相似对均分 | 0.621 | **0.727** |
| 不相似对均分 | **0.052** | 0.307 |
| 区分度 (instruction prefix) | 0.569 | **0.553** |
| 延迟 | **5.7ms/text** | 8.9ms/text |
| 维度 | 1024 | 1024 |

**结论**：切换到 Qwen3-Embedding-0.6B。隐式偏好理解力提升 29%，使用 instruction prefix 后区分度接近 v5-small。

**代码变更**：
- `OpenAIEmbeddingBackend` 新增 `query_prefix` 字段 + `with_query_prefix()` builder
- `embed_query()` 自动添加 instruction prefix（如 `"Instruct: Retrieve relevant memory entries.\nQuery: "`）
- `recall_routed`, `recall_semantic_query`, `recall_relevant_semantic` 改用 `embed_query` 而非 `embed`
- 通过 `EMBEDDING_QUERY_PREFIX` 环境变量配置

### 2. Cross-Encoder Reranker 集成

将 `bge-reranker-v2-m3` cross-encoder reranker 集成到 `recall_routed` 的最终排序阶段。

**Pipeline**：
```
Query → 3通道并发(Intent + Embed + BM25) → RFE 7信号融合 → Cross-Encoder Reranker → 返回结果
```

**代码变更**：
- `AIKernel` 新增 `reranker: Option<Arc<dyn RerankerProvider>>` 字段
- `recall_routed` 在 RFE rank 后调用 reranker 精排 top-3K candidates
- Reranker 失败时 graceful degradation 回退到 RFE 排序

### 3. LLM A/B Test — Gemma 4 vs GPT-OSS-20B

| 维度 | Gemma 4 26B-A4B | GPT-OSS-20B (MXFP4) |
|------|-----------------|---------------------|
| 意图分类准确率 | **100%** (3/3) | 0% (0/3) |
| Fact 提取 | **3/3 facts** | 0/3 (无法遵循格式) |
| 上下文问答 | **100%** | 100% |
| 平均延迟 | **516ms** | 3423ms (6.6x 慢) |
| 总评 | **100%** (5/5) | 20% (1/5) |

**结论**：GPT-OSS-20B 指令遵循能力极差（即使 `--reasoning off` 仍输出思维过程），在结构化任务上完全不可用。**保持 Gemma 4 为主力 LLM**。

### 4. Ingest Pipeline — LLM Fact Extraction + Regex Preference

新增 `src/kernel/ops/ingest.rs` 模块：

**双通道架构**：
1. **Regex Preference Extraction（默认开启，零 LLM 开销）**：
   - 16 种模式匹配（"I prefer", "I find", "I don't like" 等）
   - 生成 synthetic preference documents（如 "User prefers: PostgreSQL"）
   - 每条记忆 ingest 时自动运行

2. **LLM Fact Extraction（可选，需 `PLICO_INGEST_EXTRACT=1`）**：
   - 调用 LLM 从原始文本提取结构化 facts
   - 输出格式：`TYPE|ENTITIES|FACT_TEXT`
   - 四种类型：FACT, PREFERENCE, EVENT, PROCEDURE
   - 每个 fact 生成独立的 MemoryEntry，带 typed tags + entity: tags

**工具函数**：
- `extract_entities_regex()`: 基于大写开头的简单命名实体识别
- `extract_temporal_hint()`: 时间表达式检测（EN + ZH）
- `extract_preference_signals()`: 16 种偏好模式 regex 匹配

### 5. 预置修复

- 修复 `plico_sse.rs` 测试中缺少 `import_results` 字段的编译错误

## Benchmark 结果

### LongMemEval Real (B25) — 60 questions

| Category | v33 | v34 | Change |
|----------|-----|-----|--------|
| single-session-user | — | 90% | |
| single-session-assistant | — | 90% | |
| single-session-preference | — | 20% | |
| temporal-reasoning | — | 60% | |
| knowledge-update | — | 90% | |
| multi-session | — | 50% | |
| **Overall** | **68.3%** | **66.7%** | -1.6% |

### LoCoMo Real (B26) — 100 questions

| Category | v33 | v34 | Change |
|----------|-----|-----|--------|
| single-hop | 23% | 23% | 0% |
| temporal | 57% | **61%** | +4% |
| common-sense | 40% | 40% | 0% |
| multi-hop | 69% | 69% | 0% |
| adversarial | 100% | 100% | 0% |
| **Overall** | **61.0%** | **62.0%** | +1.0% |

### 性能指标

| 指标 | v33 | v34 | Change |
|------|-----|-----|--------|
| B25 查询延迟 | ~30ms | **28ms** | -7% |
| B26 查询延迟 | ~30ms | **29ms** | -3% |
| B25 Ingest 时间 | ~38s/q | ~38s/q | 0% |

## 关键分析

### 为什么 Overall Score 没有提升？

1. **Embedding 升级的效果被 reranker 的判断覆盖**：Cross-encoder reranker 使用自己的交叉注意力打分，embedding 质量的改进被 reranker 的能力瓶颈限制
2. **偏好类别（30% → 20%）下降**：Regex preference extraction 的 patterns 过于简单，无法匹配 LongMemEval 中复杂的隐式偏好表达
3. **核心瓶颈未变**：多会话推理（50%）和偏好检测（20%）需要 **ingest-time LLM fact extraction** 才能真正突破

### v35 方向

1. **异步 Ingest Pipeline**：将 LLM fact extraction 从同步路径移到后台异步处理
2. **选择性提取**：只对长度 > 100 chars 的内容做 LLM 提取
3. **KG Entity Linking**：利用已有 KG 基础设施做 entity co-reference
4. **Temporal Index**：结构化时间索引加速时间范围查询

## 模型配置

| 组件 | 模型 | 端口 |
|------|------|------|
| LLM | Gemma 4 26B-A4B-it Q4_K_M | 18920 |
| Embedding | **Qwen3-Embedding-0.6B Q8_0** ← 新 | 18921 |
| Reranker | **bge-reranker-v2-m3 Q4_K_M** ← 新 | 18926 |

## 新增环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `EMBEDDING_QUERY_PREFIX` | 查询 embedding 前缀 | 无 |
| `PLICO_INGEST_EXTRACT` | 启用 LLM fact extraction | 0 |
| `PLICO_RERANKER_API_BASE` | Reranker 服务 URL | 无（禁用） |
| `PLICO_RERANKER_MODEL` | Reranker 模型名 | bge-reranker-v2-m3 |
