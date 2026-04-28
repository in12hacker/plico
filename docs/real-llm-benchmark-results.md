# Plico 真实 LLM Benchmark 报告

**测试日期**: 2026-04-28
**硬件**: NVIDIA GB10 Grace Blackwell Superchip (128GB LPDDR5X)
**模型配置**:
- LLM: Gemma 4 26B-A4B-it Q4_K_M (`--reasoning off`, port 18920)
- Embedding: v5-small-retrieval Q4_K_M (1024维, port 18921)
- Qwen2.5-7B-Instruct Q4_K_M (备用 LLM, port 18923)

## 测试总览

| # | Benchmark | 通过 | 准确率 | 平均延迟 |
|---|-----------|------|--------|----------|
| B1 | Intent Classification (LLM) | ✅ | **100%** (10/10) | 123ms |
| B1 | Intent Classification (Rules) | ✅ | 90% (9/10) | <1ms |
| B2 | Embedding Semantic Similarity | ✅ | **100%** (6/6) | 17ms |
| B3 | Memory Distillation (LLM) | ✅ | 5→3 entries | 1731ms |
| B4 | Contradiction Detection (LLM) | ✅ | 75% (6/8) | 134ms |
| B5 | CAS Store + Semantic Search | ✅ | **100%** (5/5) | 25ms |
| B6 | Recall Routed (Intent+Semantic) | ✅ | 80% intent | 185ms |
| B7 | Causal Graph | ✅ | 全通过 | 31μs |
| B8 | Full Pipeline (Store→Distill→Recall) | ✅ | 全通过 | 3548ms |

**全部 8/8 Benchmark 通过 + 1010 单元测试通过**

## 详细分析

### B1: 意图分类 — LLM vs 规则引擎

Gemma 4 在所有 10 个测试 query 上 100% 准确分类：

| Query | Expected | LLM | Rules | LLM延迟 |
|-------|----------|-----|-------|---------|
| What is the capital of France? | factual | ✅ | ✅ | 129ms |
| When did Alice join the team? | temporal | ✅ | ✅ | 126ms |
| What happened before the database migration? | temporal | ✅ | ✅ | 126ms |
| Why did the auth service fail after the config? | multi_hop | ✅ | ❌temporal | 158ms |
| How did refactoring affect performance? | multi_hop | ✅ | ✅ | 153ms |
| What does Bob prefer for deployment? | preference | ✅ | ✅ | 113ms |
| Which testing framework does the team like? | preference | ✅ | ✅ | 106ms |
| List all bugs fixed in last sprint | aggregation | ✅ | ✅ | 107ms |
| Summarize key decisions from arch review | aggregation | ✅ | ✅ | 107ms |
| What is the current database schema version? | factual | ✅ | ✅ | 107ms |

**结论**: LLM-first 策略完全验证，10/10 准确率。规则引擎 9/10（multi_hop query 误分类为 temporal）。平均 LLM 延迟仅 123ms，完全可接受。

### B2: Embedding 语义相似度

v5-small-retrieval 模型的余弦相似度分布：

| 语义相似对 | CosSim | 语义不相似对 | CosSim |
|-----------|--------|------------|--------|
| cat/mat ↔ feline/rug | 0.6627 | sunny weather ↔ quantum physics | 0.0968 |
| DB migration ↔ schema update | 0.4902 | pizza ↔ stock market | -0.0154 |
| deployed API ↔ pushed update | 0.3634 | | |
| memory pressure ↔ RAM threshold | 0.2166 | | |

**结论**: 阈值 0.15 完美适配 v5-small-retrieval（检索优化模型与匹配优化模型不同，余弦相似度偏低但区分度好）。

### B3: 记忆蒸馏

5 条工作记忆 → 3 条长期记忆（按认知类型分组）：

- **Episodic** (3→1): "Alice resolved login bug... session token validation... timezone mismatch... staging deploy"
- **Procedural** (1→1): "debug auth issues: check token expiry, verify timezone, inspect session store"
- **Semantic** (1→1): "best practice: always use UTC timestamps for session management"

LLM 生成了高质量语义摘要，虽然字符数增加了（328→409），但信息密度和可检索性显著提升。

### B4: 矛盾检测

| 旧陈述 | 新陈述 | 实际矛盾? | LLM检测 |
|--------|--------|----------|---------|
| PostgreSQL | MySQL | YES | ✅ |
| Alice is tech lead | Bob is tech lead | YES | ✅ |
| API 200ms | API 500ms | YES | ✅ |
| Deploy Fridays | Deploy Tuesdays | YES | ❌ |
| Linux server | 16GB RAM | NO | ✅ |
| Alice reviewed PR | Alice wrote tests | NO | ✅ |
| Meeting 3pm | Meeting 3pm UTC | NO | ✅ |
| Python 3.9 required | Python 3.11 recommended | YES | ❌ |

**漏检分析**:
- "Deploy Fridays/Tuesdays": LLM 可能认为这是"更新"而非矛盾
- "Python 3.9 required vs 3.11 recommended": "required" vs "recommended" 语义差异使 LLM 不确定

### B5: CAS 端到端语义搜索

存储 8 条事实到 CAS → 语义搜索 5/5 全部命中 Top-1：

| 搜索词 | Top-1 结果 | 延迟 |
|--------|-----------|------|
| project deadline | The project deadline is March 15th | 28ms |
| auth module developer | Alice is the lead developer for the auth module | 31ms |
| primary database | We use PostgreSQL 15 as the primary database | 9ms |
| services communication protocol | The microservices communicate via gRPC | 29ms |
| production deploy schedule | We deploy to production every Wednesday | 27ms |

**结论**: 语义搜索管道 100% 准确，平均延迟 25ms（含 embedding 计算）。

### B6: 意图路由召回

存储 8 条 LongTerm 记忆（含 embedding 向量），通过 `recall_routed` 测试意图分类+语义检索：

- **意图准确率**: 4/5 (80%) — "How many API requests" 被误分类为 aggregation
- **总命中数**: 37 (平均每 query 7.4 条)
- **关键发现**: Preference query ("Alice prefer deployment?") 正确返回了 Alice 的偏好作为 Top-1

### B7: 因果图

3 条因果链（root → effect1 → effect2）测试：
- 构建时间: **31μs**
- 祖先追溯: 正确找到 ["root", "effect1"]
- 根因分析: 正确定位 "root"
- 后代追踪: 正确找到 ["effect1", "effect2"]

### B8: 完整管道

Store(6 entries) → Distill(6→3, LLM summarization) → Recall(9 results):
- 存储: **0ms** (Ephemeral 内存存储)
- 蒸馏: **3548ms** (3 次 LLM 调用)
- 召回: **0ms** (内存中检索)

## 性能总结

| 操作 | 延迟 | 吞吐量 |
|------|------|--------|
| LLM 意图分类 | 123ms/query | ~8 QPS |
| Embedding 生成 | 17ms/text | ~59 QPS |
| CAS 存储+索引 | 16ms/object | ~62 ops/s |
| 语义搜索 (Top-3) | 25ms/query | ~40 QPS |
| LLM 矛盾检测 | 134ms/pair | ~7 QPS |
| LLM 摘要蒸馏 | 577ms/group | ~1.7 QPS |
| 因果图构建 | 31μs | ~32,000 ops/s |

## 已知限制与改进方向

1. **Embedding 排序**: v5-small-retrieval 的余弦相似度偏低（0.15-0.66），需要模型特定的阈值校准
2. **矛盾检测**: 边界情况（语义微妙差异如 "required" vs "recommended"）准确率 75%，可通过 prompt 优化提升
3. **蒸馏压缩率**: 当前 LLM 倾向于扩展摘要（字符数增加 25%），需要更严格的压缩 prompt
4. **B6 排序质量**: recall_routed 返回结果偏多（Top-K = 8），排序准确度有提升空间
5. **LLM 延迟**: 单次 LLM 调用 ~130ms，批量场景需考虑并发或 batching

## 与 Stub 测试对比

| 指标 | Stub 模式 | 真实 LLM 模式 | 提升 |
|------|----------|-------------|------|
| 意图分类 | 规则 90% | LLM **100%** | +10% |
| 语义搜索 | 无（标签匹配） | **100%** Top-1 | 全新能力 |
| 蒸馏质量 | 规则拼接 | **LLM 语义摘要** | 质的飞跃 |
| 矛盾检测 | 关键词重叠 | LLM **75%** | 语义理解 |
