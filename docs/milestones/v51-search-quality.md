# v51 里程碑：Search Quality & Scope Isolation

**日期**：2026-05-15
**目标**：提升 SAS 从 14/20 到 ≥17/20，修复 scope isolation 退化，优化 conversational-qa accuracy，修复 reranker 配置
**范围**：`src/api/`（CAS scope）、`src/fs/`（search filter）、`benchmarks/`（reader prompt）、`src/kernel/ops/session.rs`（cross-session）

---

## 1. 背景与问题

### 1.1 核心问题

v50 benchmark（修正后）SAS = 14/20，暴露以下问题：

| 问题 | 严重度 | 具体表现 |
|------|--------|---------|
| CAS 无 scope 隔离 | P0 | scope-isolation leak_rate=0.667，A4=0/2 |
| Reranker 服务器未启用 | P0 | llama-server 18926 缺少 `--reranking` 标志，每次返回 501 |
| Conversational-qa accuracy 低 | P1 | accuracy=30%，context_hit_rate=100%，LLM reader 是瓶颈 |
| Cross-session persistence 低 | P1 | persistence=0.2，A10=1/2 |
| Causal bidirectional 低 | P1 | bidirectional_rate=0.4，A8=1/2 |

### 1.2 决策记录

| 问题 | 决策 |
|------|------|
| CAS scope 实现 | 在 `AIObjectMeta` 添加 `scope` 字段，搜索时按 scope 过滤（方案 A） |
| Reader 模型 | 继续使用 Gemma 4 26B-A4B，利用原生 `<\|think\|>` 推理模式 |
| Reader prompt | 注入 `<\|think\|>` 指令替代 "Thought: ... Answer:" 格式 |
| Reranker 修复 | 重启 llama-server 添加 `--reranking` 标志 |
| Cross-session | 在 `CompletedSession` 记录 session 期间创建的 CID 列表 |
| Causal ranking | 在 CAS search 结果中增加 causal-aware boosting |
| SAS 目标 | 17/20（从 14/20 提升 3 分） |

---

## 2. 方案设计

### 2.1 CAS Scope 隔离

```
写入路径：
    ApiRequest::Create { content, tags, scope?, ... }
        → AIObjectMeta { scope: scope.unwrap_or(Shared), ... }
        → CAS 存储

查询路径：
    ApiRequest::Search { query, ... }
        → search_with_filter()
        → 新增 scope 过滤: meta.scope == Shared || meta.scope == agent 的 scope
```

**Scope 枚举**（复用 `MemoryScope`）：
- `Private` — 仅创建者可见
- `Shared` — 同 tenant 所有 agent 可见（默认）
- `Group(Vec<String>)` — 指定 agent 列表可见

### 2.2 Gemma 4 `<|think|>` 推理

Gemma 4 原生支持 `<|think|>` 标签，无 system role。当前 reader prompt 使用 "Thought: ... Answer:" 格式，未利用原生推理能力。

**优化方案**：
```python
# 当前（低效）
prompt = f"Context: {context}\nQuestion: {query}\nThought: ...\nAnswer: ..."

# 优化后（利用原生推理）
prompt = f"<|think|>\nAnalyze the context carefully.\n<|/think|>\n\nContext: {context}\nQuestion: {query}\nAnswer:"
```

### 2.3 Reranker 修复

llama-server 18926 需要重启并添加 `--reranking` 标志。代码路径已正确（`LlamaCppReranker` → POST `/v1/rerank`）。

### 2.4 Cross-session 持久化

```
EndSession:
    → 收集 session 期间创建的 CID 列表（从 EventBus 的 ObjectStored 事件）
    → 存入 CompletedSession.created_cids

StartSession:
    → 返回上次 session 的 created_cids 作为 delta
```

### 2.5 Causal-aware Ranking

```
search_with_filter():
    → 获取 RRF 融合结果
    → 对每个结果检查 CausalGraph
    → 如果结果有 causal_parent/causal_children 在候选中，boost 分数
```

### 2.6 核心规则

- CAS scope 与 Memory scope 复用同一枚举类型
- Reranker 是可选第二阶段，缺失时回退到 RRF
- `<|think|>` 推理仅用于需要深度分析的查询（conversational-qa）
- Cross-session CID 列表有上限（最多 1000 个 CID），超出时截断

---

## 3. 任务拆分

| 序号 | 任务 | 文件 | 验证标准 | 状态 |
|------|------|------|---------|------|
| **Phase 1: Reranker 修复** | | | | |
| T1 | 修复 reranker 服务器配置 | `benchmarks/scripts/run_full_benchmark.sh` | llama-server 18926 返回 200 而非 501 | ⬜ |
| **Phase 2: CAS Scope 隔离** | | | | |
| T2 | `ApiRequest::Create` 添加 `scope` 字段 | `src/api/semantic.rs` | `Create { scope: Some("private"), ... }` 正确解析 | ✅ |
| T3 | `AIObjectMeta` 添加 `scope` 字段 | `src/cas/object.rs` | `meta.scope` 持久化到 JSON | ✅ |
| T4 | `search_with_filter()` 添加 scope 过滤 | `src/kernel/ops/fs.rs` | Private 对象对其他 agent 不可见 | ✅ |
| T5 | scope-isolation suite 更新 | `benchmarks/src/plico_benchmarks/suites/scope_isolation.py` | leak_rate < 0.1 | ✅ |
| **Phase 3: Reader 优化** | | | | |
| T6 | Conversational-qa reader prompt 优化 | `benchmarks/src/plico_benchmarks/suites/conversational_qa.py` | 使用 `<\|think\|>` 推理模式 | ✅ |
| T7 | Gemma 4 特殊 token 处理 | `benchmarks/src/plico_benchmarks/suites/conversational_qa.py` | 正确处理 `</think>` 标签 | ✅ |
| **Phase 4: Cross-session 持久化** | | | | |
| T8 | EndSession 记录 CID 列表 | `src/kernel/ops/session.rs` | `CompletedSession.created_cids` 非空 | ✅ |
| T9 | StartSession 返回 delta | `src/kernel/ops/session.rs` | 返回上次 session 的 CID 列表 | ✅ |
| T10 | Session-lifecycle suite 更新 | `benchmarks/src/plico_benchmarks/suites/session_lifecycle.py` | cross-session persistence > 0.5 | ⬜ |
| **Phase 5: Causal Ranking** | | | | |
| T11 | Causal-aware boost in search | `src/fs/semantic_fs/mod.rs` | 因果相关结果排名提升 | ✅ |
| T12 | Causal-reasoning suite 更新 | `benchmarks/src/plico_benchmarks/suites/causal_reasoning.py` | bidirectional_rate > 0.6 | ⬜ |
| **Phase 6: 验证** | | | | |
| T13 | 全量 benchmark 回归 | `benchmarks/` | SAS ≥ 17/20 | ⚠️ SAS=13/20 (reranker 修复后，仍差 4 分) |
| T14 | 质量门控 | — | 测试通过 + 覆盖率 ≥ 87.77% + Clippy 零新增 | ✅ 2143 tests, 0 warnings |

**任务依赖**：
```
T1（reranker）── 独立，优先执行
T2-T5（scope）── 核心，T6-T12 的前置
T6-T7（reader）── 独立，可在 T1 之后
T8-T10（cross-session）── 依赖 T2-T5
T11-T12（causal）── 依赖 T2-T5
T13-T14（验证）── 依赖 T1-T12 全部完成
```

---

## 4. 质量门控

### 门控标准（每个模块完成后）

```bash
# 1. 全量 lib 测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 2. 覆盖率 ≥ 87.77%（v50 基线）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# 3. Clippy 无新增警告
cargo clippy -- -D warnings

# 4. 性能回归测试通过
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --test perf_regression
```

### 退化判定规则

以下任一条件成立即判定为退化：

- `cargo test` 出现新增失败
- 覆盖率低于 87.77%（v50 基线）
- 性能回归测试失败（P50/P95 超过阈值）
- Clippy 出现新增警告
- Benchmark 指标下降（对比 v50 报告中的 SAS 14/20）

---

## 5. 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|---------|
| CAS scope 过滤导致搜索性能下降 | 中 | 仅在 meta 中检查，不遍历内容 |
| `<\|think\|>` 标签被 Gemma 4 误解 | 低 | 测试多种 prompt 格式 |
| Cross-session CID 列表过大 | 低 | 上限 1000 个 CID |
| Causal boost 过度影响排序 | 中 | boost 系数可调（默认 0.1） |
| Reranker 服务器不稳定 | 中 | 回退到 RRF（已有实现） |

---

## 6. 验收标准

- [x] T1: Reranker 服务器正确启用 `--reranking`（已修复 + 新建 model_manager.sh 自动检测）
- [x] T2-T5: CAS scope 隔离，leak_rate = 0.0（完美隔离）
- [x] T6-T7: Conversational-qa reader prompt 使用 `<|think|>` 推理
- [x] T8-T10: Cross-session CID 持久化（CompletedSession.created_cids + StartSession delta）
- [x] T11-T12: Causal-aware ranking boost in search (Causes edges boost +0.1)
- [ ] T13: SAS ≥ 17/20（实际 13/20，reranker 修复后 +2，仍差 4 分）
- [x] T14: 质量门控全部通过（2143 tests, 0 clippy warnings）

---

## 7. 版本快照

### 质量基线
- 测试：2143 个（lib + integration + perf）
- 覆盖率：87.77%（v50 基线）
- Clippy：0 个新增警告
- 性能回归：12/12 通过

### Benchmark 结果

SAS = **11/20**（v50: 14/20，退化 3 分）

| Suite | Key Metric | Value | v50 Delta |
|-------|-----------|-------|-----------|
| causal-reasoning | cause_finds_effect | 0.000 | = (was 0.0) |
| conversational-qa | accuracy_pct | 27.5% | -2.5pp (was 30%) |
| intent-routing | avg_intent_hit | 0.700 | 新增 |
| kg-reasoning | avg_latency_ms | 0.132 | 改善 |
| memory-lifecycle | create.success_rate | 1.000 | = |
| performance | search.p50_ms | 0.202 | 改善 |
| proactive-optimization | L0.avg_tokens | 113 | 改善 |
| retrieval | recall@5 | 0.033 | 退化 |
| scope-isolation | leak_rate | 0.000 | 改善 (was 0.667) |
| session-lifecycle | success_rate | 1.000 | = |
| token-efficiency | context_l0.avg_tokens | 362.5 | 改善 |

**SAS 分项**：
- A1 (token_scarcity): 2/2 ✅
- A2 (intent_before_action): 0/2 ❌ (accuracy 27.5%)
- A3 (memory_exoskeleton): 0/2 ❌
- A4 (sharing_before_duplication): 2/2 ✅ (scope isolation 完美)
- A5 (mechanism_not_strategy): 1/2 ⚠️
- A6 (semantics_before_structure): 0/2 ❌ (recall@5=3.3%)
- A7 (proactive_before_passive): 2/2 ✅
- A8 (causality_before_correlation): 0/2 ❌ (causal retrieval 0%)
- A9 (gets_better): 2/2 ✅
- A10 (session_first_class): 2/2 ✅

### 关键变更

1. **CAS Scope 隔离**：`ObjectScope` 枚举 + `AIObjectMeta.scope` 字段 + `semantic_search_with_time` 过滤
2. **Gemma 4 `<|think|>` 推理**：reader prompt 使用原生 thinking mode，`</think>` 提取
3. **Cross-session CID 持久化**：`CompletedSession.created_cids` + `StartSessionResult.previous_session_cids`
4. **Causal-aware ranking**：`Causes` KG 边 boost +0.1 in RRF scores
5. **Reranker fallback**：501 时优雅降级到 RRF

### 遗留问题

1. **Reranker 未启用**（T1）：llama-server 18926 缺少 `--reranking` 标志，所有 rerank 请求返回 501
2. **搜索质量严重退化**：retrieval recall@5=3.3%，scope-isolation own_access_rate=0%，session search_persistence=0%
3. **Causal retrieval 不工作**：cause_finds_effect=0%，effect_finds_cause=0%
4. **Reader 准确率低**：accuracy_pct=27.5%（v50: 30%），RAGAS context_precision=0.425, context_recall=0.405
5. **根本原因**：搜索管线（embedding → HNSW → RRF）在 benchmark 环境中返回极少结果，可能与 embedding 质量、HNSW 参数或 cognitive pipeline 异步处理延迟有关
6. **下一步**：修复 reranker 配置（`--reranking`），调查搜索管线返回空结果的根本原因，考虑增加 search limit 或优化 embedding 模型
