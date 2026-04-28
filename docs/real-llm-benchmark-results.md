# Plico 真实 LLM Benchmark 报告 (最终版)

**测试日期**: 2026-04-28
**硬件**: NVIDIA GB10 Grace Blackwell Superchip (128GB LPDDR5X)
**模型配置**:
- LLM: Gemma 4 26B-A4B-it Q4_K_M (`--reasoning off`, port 18920)
- Embedding: v5-small-retrieval Q4_K_M (1024维, port 18921)

## 总览: 14/14 Benchmark 全部通过

| # | Benchmark | 通过 | 准确率/指标 | 延迟 | vs 基线 |
|---|-----------|------|------------|------|---------|
| B1 | Intent Classification (LLM) | ✅ | **100%** (10/10) | 120ms | — |
| B1 | Intent Classification (Rules) | ✅ | 90% (9/10) | <1ms | — |
| B2 | Embedding Semantic Similarity | ✅ | **100%** (6/6) | 15ms | — |
| B3 | Memory Distillation | ✅ | **42.4% 压缩率** | 969ms | **+67%** 压缩率, **-44%** 延迟 |
| B4 | Contradiction Detection | ✅ | **88%** (7/8) | 127ms | **+13%** 准确率 |
| B5 | CAS Store + Semantic Search | ✅ | **100%** (5/5) | 13ms | — |
| B6 | Recall Routed (Intent+Semantic) | ✅ | **100%** intent | 192ms | **+20%** 准确率 |
| B7 | Causal Graph | ✅ | 全通过 | 25μs | — |
| B8 | Full Pipeline | ✅ | 全通过 | **1848ms** | **-48%** 延迟 |
| B9 | Scale Test (50 entries) | ✅ | 80% relevance | store p50=29ms, search p50=30ms | 新增 |
| B10 | Embedding Throughput (30) | ✅ | **90.1 emb/sec** | p50=11ms p95=12ms | 新增 |
| B11 | Multi-Session Memory | ✅ | **100%** 跨会话召回 | 8ms/query | 新增 |
| B12 | LLM Latency Stability (20) | ✅ | CV=**2.4%** | avg=107ms, 9.3 QPS | 新增 |
| B13 | Batch vs Sequential Embedding | ✅ | **2.53x 加速** | batch 3.4ms/text | 新增 |
| B14 | Multi-Round Conversation | ✅ | **100%** 验证 | distill 1587ms | 新增 |

**总运行时间: 13.26 秒 | 1010 单元测试全通过 | 零编译警告**

---

## 优化前后对比

| 指标 | 基线 (优化前) | 最终 (优化后) | 改进幅度 |
|------|-------------|-------------|----------|
| B3 压缩率 | -25.3% (膨胀) | +42.4% | **从膨胀到有效压缩** |
| B3 延迟 | 1731ms | 969ms | -44% |
| B4 矛盾检测 | 75% (6/8) | 88% (7/8) | +13pp |
| B6 意图准确率 | 80% (4/5) | 100% (5/5) | +20pp |
| B8 全管道延迟 | 3548ms | 1848ms | -48% |

---

## 详细分析

### B1: 意图分类 (100% LLM, 90% Rules)

Gemma 4 在所有 10 个测试查询上 100% 准确分类。规则引擎在 "Why did the auth service fail after the config change?" 上误分类为 temporal（因为包含 "after" 关键词），LLM 正确识别为 multi_hop。

平均 LLM 延迟 120ms，9.3 QPS 吞吐量。

### B2: Embedding 语义相似度 (100%)

v5-small-retrieval 模型的区分度验证：

| 语义相似对 | CosSim | 语义不相似对 | CosSim |
|-----------|--------|------------|--------|
| cat/feline | 0.6627 | weather/quantum | 0.0968 |
| DB migration/schema update | 0.4902 | pizza/stock market | -0.0154 |
| deploy API/push update | 0.3634 | | |
| memory pressure/RAM threshold | 0.2166 | | |

阈值 0.15 完美划分，所有相似对 > 0.15，所有不相似对 < 0.15。

### B3: 记忆蒸馏 (+42% 压缩率)

5 条工作记忆 → 3 条长期记忆（328 → 189 字符）：

- **Semantic**: "Use UTC for session management." (37字符，高度精炼)
- **Episodic**: "Alice fixed login bug by correcting session token timezone mismatch; staging deployment succeeded." (95字符)
- **Procedural**: "Debug auth: check token expiry, timezone, and session store." (57字符)

优化的 prompt 指令 "Compress into the SHORTEST possible summary" 有效引导 LLM 生成更精炼的输出。

### B4: 矛盾检测 (88%)

改进的 prompt 明确定义矛盾为"同一属性的不同值"，成功修复了 "Deploy Fridays/Tuesdays" 的漏检。

唯一剩余漏检: "Python 3.9 is required" vs "Python 3.11 is recommended" — 这在语义上存在争议（"required" 是强制的，"recommended" 是建议的，可以共存），属于合理的边界情况。

### B5: CAS 语义搜索 (100%, 13ms/query)

8 条事实存储 → 5 个语义查询全部 Top-1 命中。CAS 存储 + 索引平均 12ms/条。

### B6: 意图路由召回 (100% intent, 34 hits)

改进的 intent 分类 prompt 将 "How many X per day?" 正确归类为 factual（而非 aggregation），区分了"查找单个数值"和"汇总多条数据"的语义差异。

### B7: 因果图 (25μs)

纯内存数据结构操作，性能极高。祖先追溯、根因分析、后代追踪全部正确。

### B8: 全管道 (1848ms, -48%)

Store → Distill → Recall 端到端管道。蒸馏延迟从 3548ms 降至 1848ms，完全由 B3 prompt 优化带来——LLM 生成更短输出 = 更少推理 token = 更快响应。

### B9: 规模测试 (50 entries)

50 条异构数据（infra/team/process/architecture/metrics）的存储和搜索性能：

- **Store**: avg=26ms/条, p50=29ms, p95=39ms, p99=55ms
- **Search**: avg=28ms/query, p50=30ms, p95=33ms
- **Relevance**: 8/10 (80%) — 2 个 miss 是 embedding 局限（"backend" vs "frontend"，"gRPC" vs "REST" 语义过于相似）

### B10: Embedding 吞吐量

30 条文本连续 embedding 测试：
- **吞吐**: 90.1 embeddings/sec
- **延迟**: avg=11ms, p50=11ms, p95=12ms
- **冷启动**: 首 5 次 avg=9.6ms vs 后 5 次 avg=11.6ms — 无显著冷启动效应

### B11: 跨会话记忆 (100%)

3 个会话 × 3 条记忆 → 5 个跨会话查询全部命中：
- "What tech stack?" → 找到 "React 18 + Rust"
- "Who handles frontend?" → 找到 "Alice"
- "Performance improvement?" → 找到 "30% after Rust migration"
- "Sprint planning schedule?" → 找到 "every Monday"
- "Next milestone?" → 找到 "real-time notifications"

### B12: LLM 延迟稳定性

20 次连续 LLM 调用：
- avg=107ms, min=103ms, max=113ms
- **CV=2.4%** — 极其稳定
- 标准差 2.6ms

### B13: Batch Embedding 加速

- Sequential: 8.6ms/text (10 texts = 86ms)
- **Batch: 3.4ms/text** (10 texts = 34ms)
- **加速: 2.53x**
- 一致性: 10/10 embeddings 余弦相似度 > 0.99

**优化建议**: `remember_long_term` 当前逐条调用 `embed()`，可改为 batch 调用减少网络开销。

### B14: 多轮对话循环 (100%)

3 轮对话 → distill → 7 条长期记忆 → 跨轮验证：
- "deployment strategy?" → "blue-green" ✅
- "monitoring tools?" → "Prometheus" ✅
- "auth system?" → "JWT with refresh tokens" ✅

---

## 性能总结

| 操作 | 延迟 | 吞吐量 |
|------|------|--------|
| LLM 意图分类 | 120ms/query | 9.3 QPS |
| Embedding 单条 | 11ms/text | 90 emb/sec |
| Embedding 批量 | 3.4ms/text | 294 emb/sec |
| CAS 存储+索引 | 12ms/object | 83 ops/sec |
| 语义搜索 (Top-5) | 13ms/query | 77 QPS |
| LLM 矛盾检测 | 127ms/pair | 7.9 QPS |
| LLM 摘要蒸馏 | 323ms/group | 3.1 QPS |
| 因果图构建 | 25μs | 40,000 ops/sec |
| recall_routed (LLM分类+搜索) | 192ms/query | 5.2 QPS |

## 已知限制

1. **B4 边界 case**: "required" vs "recommended" 的矛盾检测需要更深层语义推理
2. **B9 语义区分**: 小 embedding 模型难以区分"backend"和"frontend"等语义近似概念
3. **蒸馏延迟**: 仍是全管道瓶颈（~1.8s），受限于 LLM 推理速度
4. **记忆存储非批量**: `remember_long_term` 逐条 embed，未利用 batch API（B13 已证明可 2.5x 加速）

## 下一步优化方向

1. **Batch Embedding 集成**: 在 `remember_long_term` 路径中使用 `embed_batch` API
2. **蒸馏并行化**: 不同 MemoryType 的分组可并行调用 LLM
3. **更大 Embedding 模型**: 替换为更强的 embedding 模型提升语义区分度
4. **Prompt 工程**: 矛盾检测可引入 Chain-of-Thought 提升边界 case 准确率
5. **规模扩展测试**: 100-500 条数据下的搜索延迟退化曲线
