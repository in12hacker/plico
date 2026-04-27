# Plico Benchmark 全谱评测报告

> 评测日期: 2026-04-25/26
> 评测系统: Plico v26.0.0 (Rust, usearch HNSW cos f16)
> Embedding 模型: all-MiniLM-L6-v2 (384 维, 与 agentmemory 基线一致)
> LLM Judge: qwen2.5-coder-7b-instruct (本地 llama-server, 18920 端口)
> 硬件: Linux 6.17, NVIDIA GPU server

---

## 总览

| 维度 | 指标 | 分数 | 对标 |
|------|------|------|------|
| 1. 检索质量 | R@5 / R@10 / NDCG@10 | **92.4% / 96.8% / 82.5%** | agentmemory 95.2%/98.6%/87.9% |
| 2. 端到端 QA | Accuracy (50Q) | **40.0%** | Zep 63.8%, Mem0 49% |
| 3. 增量记忆 | AR Hit Rate | **10.4%** (ruler_qa 52%/39%) | — |
| 4. 知识图谱 | Path Hit Rate | **20.8%** (339 entities, 105 edges) | — |
| 5. 性能 | CAS/Search/Memory QPS | **1 / 23 / 24** | usearch 官方: 134K QPS |

---

## 维度 1: 检索质量 — LongMemEval-S Retrieval

**方法**: 500 题, 每题 ~40-53 个 session 作为 haystack, 使用 usearch HNSW (cos, f16) + all-MiniLM-L6-v2 embedding。纯检索评估，不依赖 LLM。

### 综合结果

| 指标 | Plico | agentmemory (BM25+Vec) | MemPalace (Vec) | 差距 |
|------|-------|------------------------|-----------------|------|
| R@5 | **92.4%** | 95.2% | 96.6% | -2.8pp |
| R@10 | **96.8%** | 98.6% | ~97.6% | -1.8pp |
| R@20 | **98.4%** | 99.4% | — | -1.0pp |
| NDCG@10 | **82.5%** | 87.9% | — | -5.4pp |
| MRR | **82.6%** | 88.2% | — | -5.6pp |

### 按题型分析

| 题型 | R@5 | R@10 | 数量 | 评估 |
|------|-----|------|------|------|
| knowledge-update | **97.4%** | 100.0% | 78 | 优秀 |
| single-session-assistant | **98.2%** | 98.2% | 56 | 优秀 |
| multi-session | **94.7%** | 99.2% | 133 | 良好 |
| temporal-reasoning | **92.5%** | 96.2% | 133 | 良好 |
| single-session-preference | **83.3%** | 96.7% | 30 | 弱项 |
| single-session-user | **81.4%** | 88.6% | 70 | **最弱** |

### 分析

- **强项**: `knowledge-update` 和 `single-session-assistant` 接近满分, embedding 模型对知识更新型问题和助手回复型问题的表征能力强
- **弱项**: `single-session-user` (81.4%) 和 `single-session-preference` (83.3%) 偏低, 用户偏好类问题通常包含隐含信息, 纯向量检索难以捕捉
- **与 agentmemory 差距**: 约 2.8pp (R@5), 主要因为 agentmemory 使用 BM25+Vector 混合检索, BM25 对关键词匹配场景有补充作用
- **搜索延迟**: 平均 0.04ms/query (usearch 本身极快, 受限于 Python 端开销)

---

## 维度 2: 端到端记忆 — LongMemEval-S E2E QA

**方法**: Plico 向量检索 top-5 → llama-server (qwen2.5-coder-7b) reader → 同模型 judge。
仅测试 50 题子集 (`single-session-user` 类型), 7B 模型作为 judge 质量有限。

| 模式 | 准确率 | 对标 |
|------|--------|------|
| Retrieval (Plico 检索) | **40.0%** (20/50) | — |
| Zep/Graphiti (GPT-4o judge) | 63.8% | 参照 |
| Mem0 (GPT-4o judge) | 49.0% | 参照 |
| Oracle GPT-4o | 82.4% | 上限 |

### 分析

- **40% 准确率**需谨慎解读: 使用 7B coding 模型作为 reader+judge, 与文献中 GPT-4o judge 不可直接对比
- 主要瓶颈在 **reader 能力** (7B 模型理解长文本不如 GPT-4o) 而非检索质量
- 检索本身对这 50 题的 R@5=81.4%, 即约 19% 的题目检索就已失败
- **建议**: 用 GPT-4o 级别模型重跑此评测可获得更有意义的对比

---

## 维度 3: 增量记忆 — MemoryAgentBench AR

**方法**: 20 篇文档, 每篇 ~100 个问题。文档分块后 embedding + usearch 索引, 检索 top-5 检查答案是否包含。

| 子集 | Hit Rate | 数量 | 评估 |
|------|----------|------|------|
| ruler_qa1_197K | **52.0%** | 100 | 中等 — 长文档知识检索 |
| ruler_qa2_421K | **39.0%** | 100 | 偏低 — 超长文档稀释 |
| eventqa_full | **0-1%** | 500 | 极低 — 事件链理解 |
| eventqa_65536 | **0%** | 500 | 极低 — 时间序列事件 |
| eventqa_131072 | **0%** | 500 | 极低 |
| longmemeval_s* | **51-65%** | 180 | 中等 — 对话记忆 |
| **总体** | **10.4%** | 1880 | — |

### 分析

- **ruler_qa 系列** (52%/39%): 这是 RULER 格式的 QA, 答案通常是短 entity, 向量检索 + 精确匹配评估合理
- **eventqa 系列** (0-1%): 几乎完全失败。EventQA 需要事件链理解和时间推理, 纯向量检索无法处理。很多文档被分成 1 个 chunk (分块策略不适配)
- **longmemeval_s** (51-65%): 对话记忆检索表现中等
- **整体 10.4%** 被 eventqa 严重拉低, 实际上 ruler_qa 和 longmemeval 子集表现合理
- **核心问题**: 缺乏 BM25 关键词检索和时间感知能力

---

## 维度 4: 知识图谱 — 多跳推理

**方法**: 从 LongMemEval `multi-session` 和 `temporal-reasoning` 题目中抽取 20 题, 用 LLM (qwen2.5-coder-7b) 从 evidence sessions 抽取实体和关系, 存入 Plico KG (redb), 测试路径发现。

| 指标 | 值 |
|------|-----|
| 问题数 | 20 |
| 实体存储 | **339** |
| 关系存储 | **105** |
| 路径查询 | 120 次 |
| 路径命中 | **25** (20.8%) |
| LLM 抽取 | qwen2.5-coder-7b |

### 分析

- **Plico KG 基础设施工作正常**: add_node/add_edge/find_paths API 功能完备
- **20.8% 路径命中率**: 低于预期, 原因分析:
  1. **LLM 实体抽取质量**: 7B coding 模型对实体关系抽取不够精确, 抽取的 "实体" 多为普通名词而非命名实体
  2. **关系映射损失**: LLM 返回的自由文本 predicate 需映射到固定的 `KGEdgeType` 枚举, 损失了语义精度
  3. **稀疏图**: 339 节点 + 105 边, 平均度数仅 0.62, 图过于稀疏导致多跳路径稀少
- **改进方向**: 使用更强的 NER 模型, 或在 Plico 内核集成自动 KG 构建管道

---

## 维度 5: 性能微基准

**配置**: plicod + EMBEDDING_BACKEND=stub, 单线程 TCP 同步调用

| 操作 | QPS | P50 延迟 | 评估 |
|------|-----|----------|------|
| CAS write | **1** | 910.6ms | **严重瓶颈** |
| Semantic search | **23** | 43.1ms | 合格 |
| Memory store | **24** | 41.2ms | 合格 |
| Memory recall | **24** | 41.5ms | 合格 |
| KG add_node | — | — | 测试中断 (内存压力) |

### 与 usearch 官方基准对比

| 指标 | Plico (via plicod) | usearch 官方 |
|------|-------------------|--------------|
| Vector search QPS | 23 (TCP 端到端) | 134,000 (in-process) |
| Search R@1 | ~82% (384d, cos, f16) | 99.2% (1M vectors) |

### 瓶颈分析

1. **CAS write 1 QPS**: 这是最关键的瓶颈。每次 `create` 操作触发:
   - SHA-256 哈希计算
   - 磁盘文件写入 (同步)
   - Tag 索引更新
   - Embedding 生成 (即使 stub 也有开销)
   - 事件记录
   - **建议**: 引入批量写入 API, 异步持久化, 或 write-ahead log
2. **Search/Memory 23-24 QPS**: TCP 往返 + 内核处理 ~40ms, 对单用户场景充足
3. **内存使用**: 100 次 CAS write 导致 2.2GB RSS, 需要调查内存泄漏
4. **KG 未完成**: 因 plicod 内存压力导致连接断开

---

## 综合竞品对比

| 系统 | 语言 | LongMemEval R@5 | 类型 | 特点 |
|------|------|-----------------|------|------|
| **Plico** | **Rust** | **92.4%** | **AI-OS Kernel** | **本地优先, CAS+KG+4层记忆** |
| agentmemory | TypeScript | 95.2% | Memory Layer | BM25+Vector hybrid |
| MemPalace | Python | 96.6% | Vector Only | Pure vector search |
| OMEGA | Python | 95.4% (QA) | Memory Server | Local-first, SQLite |
| Zep/Graphiti | Python | 63.8% (QA) | Temporal KG | 时间推理 |
| Mem0 | Python | 49.0% (QA) | Cloud Memory | 即插即用 |

### Plico 的独特优势

1. **Rust 性能底座**: 搜索延迟 0.04ms, 远低于 Python 框架 (usearch in-process)
2. **CAS + 知识图谱**: 唯一同时具备内容寻址存储和结构化知识图谱的 AI 记忆系统
3. **4 层记忆分级**: Ephemeral → Working → LongTerm → Procedural, 符合认知科学
4. **本地优先**: 零云端依赖, 隐私安全, 单节点部署

### Plico 当前劣势

1. **缺少 BM25 混合检索**: 相比 agentmemory 的 BM25+Vector 混合方案, 纯向量检索在关键词匹配场景劣 2-3pp
2. **CAS write 性能**: 1 QPS 是硬伤, 需要批量写入优化
3. **时间感知不足**: temporal-reasoning 和 eventqa 表现差, 缺乏时间窗口感知的检索策略
4. **内存使用偏高**: 100 对象 2.2GB RSS 异常, 需排查

---

## 改进路线

### 短期 (1-2 周)

1. **CAS write 性能优化**
   - 引入 batch_create API, 一次请求写入多个对象
   - 异步持久化: 写入内存后立即返回, 后台批量 flush
   - 调查 2.2GB RSS 问题
2. **usearch 参数调优**
   - 增加 `ef_construction` 和 `ef_search` 提升 recall (当前使用默认值)
   - 测试 f32 vs f16 精度影响

### 中期 (1-2 月)

3. **添加 BM25 混合检索**
   - 仿 agentmemory 的 BM25+Vector RRF (Reciprocal Rank Fusion) 策略
   - 预期提升 R@5 约 2-3pp, 达到 ~95%
4. **时间感知检索**
   - 仿 Zep/Graphiti 的 temporal window 策略
   - 对 temporal-reasoning 类问题自动添加时间区间过滤
5. **自动 KG 构建管道**
   - 在 CAS 写入时自动抽取实体/关系 (pipeline hook)
   - 使用更精确的 NER 模型替代通用 LLM

### 长期 (3-6 月)

6. **RAG-Graph 融合**
   - 结合 KG 图遍历和向量检索的 Hybrid RAG
   - Plico 已有 hybrid_retrieval API 框架, 需实现
7. **Embedding 模型升级**
   - 从 all-MiniLM-L6-v2 (384d) 升级到 bge-large-en-v1.5 (1024d)
   - 需要调整 usearch 参数和存储开销
8. **分布式 CAS**（不需要分布式，这里之作为记录，没用）
   - 多节点 CAS 一致性协议
   - 适应大规模 Agent 集群场景

---

## 评测方法论说明

1. **维度 1 (检索)**: 纯向量检索评估, 不受 LLM 影响, 最可靠的对比指标
2. **维度 2 (E2E)**: 受限于本地 7B judge, 分数不与 GPT-4o judge 文献直接对比, 仅作方向性参考
3. **维度 3 (MAB)**: 数据集格式特殊 (超长文档 + 精确答案匹配), eventqa 子集不适合纯向量检索评估
4. **维度 4 (KG)**: 自定义评测, 无直接竞品基线; 重点验证 KG 基础设施功能完备性
5. **维度 5 (性能)**: 通过 TCP 测量端到端延迟, 包含网络 + 序列化 + 内核处理开销

---

## 原始数据文件

- `bench/results/longmemeval_retrieval.json` — 维度 1 全量结果
- `bench/results/longmemeval_e2e_retrieval.json` — 维度 2 结果
- `bench/results/memoryagentbench_ar.json` — 维度 3 结果
- `bench/results/kg_multi_hop.json` — 维度 4 结果
- `bench/results/perf_micro.json` — 维度 5 结果
