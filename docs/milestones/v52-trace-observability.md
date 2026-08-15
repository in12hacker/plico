# v52 里程碑：Trace Observability + Quality Fixes

**日期**：2026-05-17
**目标**：建立 tool call trace 基础设施，修复 benchmark 测试准确性，增强 recall 语义搜索
**范围**：Trace 存储 + CLI/API 查询 + judge 校准 + recall 语义搜索

---

## 1. 背景与问题

### 1.1 核心问题

| 问题 | 严重度 | 具体表现 |
|------|--------|---------|
| 无 tool call trace | P1 | Agent 执行失败时无法回溯，无法从历史学习，无法提炼技能 |
| causal-reasoning accuracy_pct=0 | P1 | 检索 100% 命中，但 judge 将 snippet 与 full text 比较，永远低分 |
| memory-lifecycle accuracy_pct=6.7% | P1 | snippet 截断 200 字符 vs expected 全文，recall 无 embedding |
| recall_isolation=0 | P2 | recall 端点使用 substring 匹配，无语义搜索能力 |

### 1.2 根因分析

**causal-reasoning accuracy_pct=0**：
- 检索已修复（bidirectional_rate=1.0），但 judge 评估的是 raw snippet
- snippet 截断到 200 字符，expected 是完整因果句
- judge 给 3 分（"部分正确，不完整"），threshold=4 不通过
- conversational-qa 成功因为它用 LLM 合成完整答案

**memory-lifecycle accuracy_pct=6.7%**：
- `remember()` 存入 Ephemeral tier，`embedding: None`
- `recall()` 做 substring 匹配，不是语义搜索
- `recall_semantic` 存在但只搜索 Long-term tier（有 embedding 的条目）
- snippet 截断到 200 字符 vs expected 全文

**recall_isolation=0**：
- benchmark 用 `recall()`（substring 匹配）+ `remember()`（无 embedding）
- `recall_semantic` 和 `recall_routed` 已存在且可用
- gap：benchmark 测试用错了端点 + `remember()` 不生成 embedding

### 1.3 决策记录

| 决策 | 结论 |
|------|------|
| Trace 存储 | JSONL 文件（`~/.plico/tool_trace/<date>/<agent>.jsonl`） |
| Trace 写入 | mpsc channel + 单线程 writer worker（非阻塞） |
| Trace 保留期 | 7 天自动清理，可配置 |
| Trace-Session 关系 | 松散关联（trace 有 session_id 字段） |
| Phase 1 范围 | 完整基础设施（存储 + CLI + API） |
| Trace→Knowledge | Phase 2 再做（v53） |
| judge 校准方案 | 改 snippet 比较逻辑，不改 judge prompt |
| recall 语义搜索 | benchmark 改用 `recall_semantic` + `remember_long_term` |

---

## 2. 方案设计

### 2.1 模块 A：Trace 基础设施

**文件结构**：
```
src/kernel/trace/
├── mod.rs          // TraceStore + Span 结构体 + 生命周期管理
├── writer.rs       // mpsc channel + JSONL writer worker
└── query.rs        // CLI/API 查询逻辑
```

**Span 结构体**：
```rust
pub struct Span {
    pub trace_id: String,
    pub parent_id: Option<String>,
    pub span_id: String,
    pub agent_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub output: serde_json::Value,
    pub status: SpanStatus,    // Success | Error | Timeout
    pub latency_ms: u64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub session_id: Option<String>,
    pub intent_id: Option<String>,
}
```

**集成点**：
- `src/kernel/api_dispatch.rs`：在 `handle_api_request` 入口/出口写入 span
- `src/kernel/trace/writer.rs`：`std::sync::mpsc` channel + 后台 writer 线程
- `src/kernel/trace/mod.rs`：7 天自动清理

**CLI 命令**（`src/bin/aicli/commands/handlers/trace.rs`）：
- `aicli trace list [--agent X] [--last N] [--since 7d]`
- `aicli trace show <trace_id>`
- `aicli trace failures [--agent X] [--since 7d]`

**API 变体**（`src/api/semantic.rs`）：
- `TraceList { agent_id, since, until, tool_name, status, limit }`
- `TraceShow { trace_id }`

### 2.2 模块 B：Benchmark Judge 校准

**问题**：judge 把 raw snippet（200 字符）与 full expected text 比较，永远低分。

**方案**：修改 `evaluate_scored()` 的调用方式——不是改 judge prompt，而是改传入的 `actual` 参数。

**causal-reasoning 修复**：
- 当前：`evaluate_scored(cause_text, effect_text, effect_snippet)`
- 修复：`evaluate_scored(cause_text, effect_text, effect_snippet)` 不变，但改善 snippet 质量
- 具体：用 `recall_routed` 或更长的 snippet（从 CAS 获取完整内容）

**memory-lifecycle 修复**：
- 当前：judge 比较 snippet vs full content
- 修复：judge 比较 snippet vs snippet（expected 也用 snippet）
- 或者：用 LLM 从 snippet 合成答案（与 conversational-qa 一致）

**通用方案**：引入 `Reader` 模式——从检索结果合成答案，而非直接传 snippet。
```python
# 当前（raw snippet）：
score = judge.evaluate_scored(question, expected, snippet)

# 修复（LLM 合成答案）：
context = "\n".join(snippets)
answer = llm.generate(f"Based on context, answer: {question}\nContext: {context}")
score = judge.evaluate_scored(question, expected, answer)
```

### 2.3 模块 C：Recall 语义搜索增强

**问题**：`recall()` 做 substring 匹配，`remember()` 不生成 embedding。

**方案分两层**：

**层 1：Benchmark 测试修复**（不改 Rust 代码）
- `scope_isolation.py` 的 `_test_recall_isolation` 改用 `recall_semantic()` + `remember_long_term()`
- `memory_lifecycle.py` 的 `_test_layer_migration` 已修复（用 `recall()` + 内容匹配）

**层 2：Rust 代码增强**（可选，提升 recall 端点能力）
- `remember()` 异步生成 embedding（通过 Cognitive Pipeline）
- `recall()` 查询时，如果有 query 且条目有 embedding，用语义排序替代 substring 匹配

**建议**：层 1 在 v52 做，层 2 在 v53 做（需要异步 embedding pipeline）。

---

## 3. 任务拆分

| 序号 | 任务 | 验证标准 | 状态 |
|------|------|---------|------|
| **模块 A：Trace 基础设施** | | | |
| A1 | `src/kernel/trace/mod.rs`：Span 结构体 + TraceStore | 单元测试：Span 序列化/反序列化 | ✅ |
| A2 | `src/kernel/trace/writer.rs`：mpsc channel + JSONL writer | 单元测试：写入 + 读取 + 7 天清理 | ✅ |
| A3 | `src/kernel/api_dispatch.rs`：集成 trace 写入 | 集成测试：API 调用产生 trace | ✅ |
| A4 | `src/api/semantic.rs`：TraceList/TraceShow API 变体 | handler 测试：查询返回正确结果 | ✅ |
| A5 | `src/bin/aicli/commands/handlers/trace.rs`：CLI 命令 | 手动测试：`aicli trace list/show/failures` | ✅ |
| A6 | 性能回归：trace 写入不影响 API 延迟 | perf_regression 测试通过 | ✅ |
| **模块 B：Judge 校准** | | | |
| B1 | `causal_reasoning.py`：引入 Reader 模式（LLM 合成答案） | accuracy_pct > 0 | ✅ |
| B2 | `memory_lifecycle.py`：引入 Reader 模式 | accuracy_pct > 13.3 | ✅ |
| B3 | 验证 conversational-qa 无退化 | 无代码变更，无退化 | ✅ |
| **模块 C：Recall 语义搜索** | | | |
| C1 | `scope_isolation.py`：改用 `recall_semantic` + `remember_long_term` | own_recall_hits > 0 | ✅ |
| C2 | 验证 memory-lifecycle layer_migration 无退化 | cross_layer_hit_rate=1.0, recall_hit_rate=1.0 | ✅ |

---

## 4. 质量门控

### 门控标准（每个模块完成后）

```bash
# 1. 全量测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# 2. 覆盖率 ≥ 87%（不低于基线）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# 3. Clippy 无新增警告
cargo clippy -- -D warnings

# 4. 性能回归测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression
```

### 退化判定规则

以下任一条件成立即判定为退化：

- `cargo test` 出现新增失败
- 覆盖率低于 87%
- 性能回归测试失败
- Clippy 新增警告
- Benchmark 指标下降（对比 v51-embedfix 报告）

---

## 5. 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| Trace 写入增加 API 延迟 | 性能退化 | mpsc channel 非阻塞，writer 异步 |
| Reader 模式增加 benchmark 时间 | Benchmark 超时 | 限制 Reader token 数，超时回退 snippet |
| `recall_semantic` 在 stub 后端不可用 | 测试失败 | benchmark 用真实 embedding server |
| 7 天清理误删重要 trace | 数据丢失 | 清理前可配置保留期 |

---

## 6. 验收标准

- [x] 所有任务完成（A1-A6, B1-B3, C1-C2）
- [x] 质量门控全部通过
- [x] 无退化
- [x] `aicli trace list/show/failures` 可用

---

## 7. 版本快照

### 质量基线
- 测试：2157 个（lib）
- 覆盖率：87.61%（≥87% 阈值）
- Clippy：0 个新增警告
- 性能回归：13/13 通过

### Benchmark 结果（v52 vs v50 完整对比）

报告：`benchmarks/docs/benchmark_report_v52_comparison.md`

#### 关键指标变化

| Suite | Metric | v50 | v52 | Delta |
|-------|--------|-----|-----|-------|
| conversational-qa | accuracy_pct | 30.0% | 42.5% | +12.5pp |
| conversational-qa | f1 | 0.204 | 0.258 | +0.054 |
| conversational-qa | RAGAS faithfulness | — | 0.950 | 超目标 0.85 |
| retrieval | recall@5 | 0.557 | 0.687 | +0.130 |
| retrieval | recall@10 | 0.770 | 0.727 | -0.043 |
| scope-isolation | own_access_rate | 0.000 | 1.000 | +1.000 |
| scope-isolation | leak_rate | 0.000 | 0.000 | = |
| session-lifecycle | success_rate | 0.000 | 1.000 | +1.000 |
| session-lifecycle | search_persistence | 0.000 | 1.000 | +1.000 |
| memory-lifecycle | search.hit_rate | 0.000 | 1.000 | +1.000 |
| memory-lifecycle | layer_migration.accuracy_pct | 13.3% | 100.0% | +86.7pp |
| memory-lifecycle | delete.success_rate | 0.000 | 1.000 | +1.000 |
| causal-reasoning | bidirectional_rate | 0.000 | 0.900 | +0.900 |
| causal-reasoning | cause_finds_effect | 0.000 | 1.000 | +1.000 |
| causal-reasoning | accuracy_pct | 0.0% | 50.0% | +50.0pp |
| performance | cas_write.qps | 21.3 | 204.0 | +182.7 |
| performance | memory_recall.qps | 2218.7 | 8030.7 | +5812.0 |
| **⚠️ performance** | **search.qps** | **1375.5** | **3.0** | **-1372.5** |
| **⚠️ performance** | **search.p50_ms** | **0.079** | **288.9** | **+288.8** |
| token-efficiency | L2 avg_tokens | 1689.8 | 1854.8 | +165.0 |
| proactive-optimization | L2 avg_tokens | 1058.8 | 568.9 | -489.9 |

#### SAS

| # | Axiom | v50 | v52 | Delta |
|---|-------|-----|-----|-------|
| A1 | token_scarcity | 2/2 | 2/2 | = |
| A2 | intent_before_action | 1/2 | 1/2 | = |
| A3 | memory_exoskeleton | 0/2 | 0/2 | = |
| A4 | sharing_before_duplication | 2/2 | 2/2 | = |
| A5 | mechanism_not_strategy | 1/2 | 1/2 | = |
| A6 | semantics_before_structure | 1/2 | 1/2 | = |
| A7 | proactive_before_passive | 2/2 | 2/2 | = |
| A8 | causality_before_correlation | 0/2 | 2/2 | +2 |
| A9 | gets_better | 2/2 | 2/2 | = |
| A10 | session_first_class | 0/2 | 2/2 | +2 |
| **Total** | | **11/20** | **15/20** | **+4** |

### 关键变更
- **3a5910c**: embedding 修复 — `remember_working_scoped` 同步生成 embedding（修复语义搜索不可用）
- **A1-A6**: Trace 基础设施（Span + TraceStore + mpsc Writer + CLI/API）
- **B1-B2**: benchmark 引入 Reader 模式（causal-reasoning + memory-lifecycle）
- **C1**: scope_isolation 改用 `recall_semantic` + `remember_long_term`
- **benchmark.md**: 添加前置条件规则（完整 benchmark 前必须通过阶段 3+4）

### 遗留问题
- **搜索性能严重退化**：同步 embedding 生成导致 search qps 从 1375 降至 3，延迟从 0.08ms 升至 289ms
- **conversational-qa accuracy_pct 42.5%**：远低于 OMEGA 95.4%、Mastra 94.87%
- **A3 memory_exoskeleton 0/2**：CRUD 指标 1.0 但 accuracy_pct 和跨层语义迁移待提升
- **A2 intent_before_action 1/2**：intent-routing accuracy_pct=50%，hit_rate 部分退化
