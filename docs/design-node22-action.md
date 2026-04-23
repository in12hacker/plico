# Plico 第二十二节点设计文档
# 行 — 执行即学习

**版本**: v1.0
**日期**: 2026-04-24
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Execution as Learning + Self-Optimization + Predictive Execution
**前置**: 节点 21 ✅（100%）— 意(4维) + 1361 tests + Soul 92%
**验证方法**: E2E learning loop + profile growth + prediction accuracy
**信息来源**: `docs/design-node21-will.md` + Agentic Memory (Alex Spyropoulous 2025) + Self-Optimizing Systems

---

## 0. 链式思考：从 Node 21 到 Node 22

### 为什么需要"行"

Node 21 建立了**意**能力：
- 结构化意图声明（IntentDeclaration）
- Intent Plan 分解（DAG 步骤图）
- 自主执行循环（AutonomousExecutor）
- 多 Agent 协作（IntentTree）

**但这些只是"知道要做什么"**。Soul 2.0 的核心差距是：

| 公理 | Node 21 实现 | 对齐度 |
|------|-------------|--------|
| 公理9: 越用越好 | AgentProfile 有 transition matrix 但从未更新 | 35% |
| 公理7: 主动先于被动 | 预取基于历史，但不基于执行时间优化 | 50% |
| 公理2: 意图先于操作 | Intent 声明触发执行，但不预测 | 60% |

**"行"的三层含义**：
1. **执行即学习**：执行结果写回 AgentProfile，更新 transition matrix 和 hot_objects
2. **自我优化**：根据历史执行时间优化 plan 排序和资源分配
3. **预测执行**：在 intent 声明前，基于 profile 预测需求并预取

### 链式推导

```
[因] AutonomousExecutor 执行结果不写回 AgentProfile
    ↓
[果] AgentProfile 永远是静态的，越用不越好 (公理9违反)
    ↓
[因] 无执行时间追踪 → [果] plan 排序不考虑执行效率
    ↓
[因] 预测只基于 intent similarity，不基于执行历史
    ↓
[因] 公理9 要求"越用越好" + 公理7 要求"主动先于被动"
    ↓
[果] 需要 Execution as Learning + Self-Optimization + Predictive Execution
```

---

## 1. 现状分析

### 1.1 Node 21 后的能力

| 能力 | 实现 | 功能 |
|------|------|------|
| IntentDeclaration | struct | 结构化意图声明 |
| IntentPlan | DAG | 意图分解为步骤 |
| AutonomousExecutor | async loop | 自主执行步骤 |
| IntentTree | coordination | 多 Agent 协作 |

### 1.2 关键差距

**Gap 1: 执行结果不写回 Profile**
- AutonomousExecutor 执行完成，结果就丢了
- AgentProfile.intent_transitions 从不更新
- hot_objects 只读，从不写
- 效果：第100次执行和第1次一样

**Gap 2: 无执行时间优化**
- plan 排序只看依赖，不看执行时间
- 长时间步骤应该更早启动
- 效果：执行效率未优化

**Gap 3: 预测与执行脱节**
- prefetch 有 prediction 但与 execution loop 无关
- execution 完成后不触发新的 prefetch
- 效果：无法形成"预测→执行→学习"闭环

---

## 2. Node 22 五大维度

### D1: Execution as Learning — 执行即学习

**问题**: 执行结果不写回 AgentProfile，越用不越好。
**目标**: AutonomousExecutor 执行完成后，更新 AgentProfile 的 transition matrix 和 hot_objects。
**实现策略**:
- 在 `complete_step` 后调用 `profile_store.record_intent()`
- 记录哪些 CIDs 被实际使用（从 step result output_cids）
- 更新 hot_objects 计数

### D2: Execution Time Tracking — 执行时间追踪

**问题**: plan 排序不考虑执行效率。
**目标**: 追踪每种操作类型的平均执行时间，优化 plan 排序。
**实现策略**:
- `ExecutionStats`: 记录每种 IntentOperation 的平均执行时间
- 在 plan 排序时，长时间操作优先（尽早开始）
- 持久化到 profile store

### D3: Self-Optimization — 自我优化

**问题**: plan 排序只基于依赖，不基于效率。
**目标**: 根据历史执行时间，优化 plan 排序。
**实现策略**:
- 在 topological_sort 时考虑执行时间权重
- 长时间步骤优先（尽早启动并行化）
- 根据执行成功率调整下一步预测

### D4: Predictive Execution — 预测执行

**问题**: 预测与执行脱节。
**目标**: 执行完成后，基于结果触发新的 prefetch。
**实现策略**:
- execution loop 完成后，检查 output CIDs
- 如果有高置信度的下一 intent，触发 prefetch
- 将 prediction 与实际执行结果对比，更新置信度

### D5: Learning Loop Closure — 学习环闭合

**问题**: 各模块独立，无闭环。
**目标**: 形成"声明→计划→执行→学习→预测→声明"闭环。
**实现策略**:
- `declare_intent` 时检查是否有历史 profile
- `execute_step` 时记录实际使用的 CIDs
- `complete_plan` 时更新 profile 并检查预测

---

## 3. 特性清单

### F-1: Execution as Learning

```rust
// kernel/ops/intent_executor.rs — 修改 AutonomousExecutor
impl AutonomousExecutor {
    /// Execute an intent plan and record learning feedback.
    pub async fn execute_plan(&self, plan: &IntentPlan) -> IntentExecutionResult {
        // ... existing execution ...
        
        // F-1: Write execution results back to AgentProfile
        for (step_id, result) in &execution_result.results {
            if result.success {
                // Record CID usage for hot_objects
                self.update_profile_on_success(step_id, &result.output_cids);
            }
        }
        
        // F-1: Update transition matrix
        if let Some(last_step) = execution_result.results.keys().last() {
            self.profile_store.record_intent(
                agent_id,
                &self.extract_intent_tag(last_step),
                None,
            );
        }
    }
}
```

**测试**:
- `test_execution_writes_to_profile`: 执行后 profile 被更新
- `test_hot_objects_updated_after_execution`: hot_objects 计数增加
- `test_transition_matrix_updated`: transition matrix 有新条目

**新测试**: 3

### F-2: Execution Time Tracking

```rust
// kernel/ops/intent_executor.rs — 新增
pub struct ExecutionStats {
    /// Average execution time per operation type (ms).
    avg_times: HashMap<String, u64>,
    /// Success count per operation type.
    success_counts: HashMap<String, u32>,
}

impl ExecutionStats {
    pub fn record(&mut self, operation_type: &str, duration_ms: u64, success: bool);
    pub fn get_avg_time(&self, operation_type: &str) -> Option<u64>;
}
```

**测试**:
- `test_execution_stats_tracking`: 记录执行时间
- `test_avg_time_calculation`: 平均时间计算正确
- `test_stats_persistence`: 统计持久化到 profile

**新测试**: 3

### F-3: Self-Optimization

```rust
// IntentPlan — 修改 topological_sort 支持时间权重
impl IntentPlan {
    /// Topological sort with execution time optimization.
    /// Long-running steps are prioritized to start earlier.
    pub fn optimized_sort(&self, stats: &ExecutionStats) -> Result<Vec<usize>, PlanError> {
        // Weight steps by average execution time
        // Prioritize steps that take longer
    }
}
```

**测试**:
- `test_optimized_sort_prioritizes_long_steps`: 长时间步骤优先
- `test_optimized_sort_respects_dependencies`: 依赖仍然满足
- `test_optimized_sort_empty_plan`: 空 plan 处理

**新测试**: 3

### F-4: Predictive Execution

```rust
// AutonomousExecutor — 执行后预测
impl AutonomousExecutor {
    /// After execution, check for predicted next intent and prefetch.
    async fn trigger_predictive_prefetch(&self, result: &IntentExecutionResult) {
        // Get predicted next intent from profile
        if let Some(next_intent) = self.profile_store.predict_next(&current_intent_tag) {
            // Check confidence threshold
            if confidence >= 0.5 {
                // Trigger prefetch for predicted intent
                self.prefetcher.prefetch_async(&next_intent, ...);
            }
        }
    }
}
```

**测试**:
- `test_predictive_prefetch_triggered`: 高置信度时触发预取
- `test_no_prefetch_low_confidence`: 低置信度不触发
- `test_feedback_improves_prediction`: 反馈改善预测

**新测试**: 3

### F-5: Learning Loop Closure

```rust
// kernel/ops/intent.rs — IntentTracker 整合学习
impl IntentTracker {
    /// Complete intent and trigger learning loop.
    pub fn complete_with_learning(&self, intent_id: &str, result: IntentExecutionResult) {
        // 1. Record execution results to profile
        // 2. Check for next intent prediction
        // 3. Trigger predictive prefetch if confidence high
    }
}
```

**测试**:
- `test_learning_loop_closure`: 完整闭环执行
- `test_intent_profile_growth`: profile 随执行增长
- `test_no_redundant_learning`: 幂等性

**新测试**: 3

---

## 4. 行业研究对标

### 4.1 Agentic Memory (Alex Spyropoulous, 2025)

| 维度 | Agentic Memory | Plico Node 22 | 对齐度 |
|------|----------------|---------------|--------|
| Behavior rule learning | Transition matrix | AgentProfile update | ✅ F-1 |
| Bidirectional memory links | KG因果边 | CausalHook写入 | ✅ Node 20 |
| Multi-layer optimization | L0/L1/L2 | 已有 | ✅ |
| Automatic maintenance | TTL/eviction | IntentCache TTL | ✅ |
| **Execution feedback loop** | 需落地 | **F-1 + F-4** | 🔴 |

---

## 5. 量化目标

| 指标 | N21 现状 | N22 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 1361 | **1376+** | 🔴 |
| Execution as Learning | 0 (结果丢弃) | **Profile 更新** | 🔴 |
| Execution Time Tracking | 0 | **Stats 记录** | 🔴 |
| Self-Optimization | 0 | **optimized_sort** | 🔴 |
| Predictive Execution | 0 | **prefetch 触发** | 🔴 |
| Learning Loop | 0 | **完整闭环** | 🔴 |

---

## 6. 实施计划

### Phase 1: Execution as Learning (M1, ~1 day)

1. F-1: AutonomousExecutor 写回 AgentProfile
2. hot_objects 更新逻辑
3. transition matrix 更新逻辑
4. 测试: 3 个新测试

### Phase 2: Execution Time Tracking (M2, ~0.5 day)

1. F-2: ExecutionStats 数据结构
2. 时间记录逻辑
3. 测试: 3 个新测试

### Phase 3: Self-Optimization (M3, ~1 day)

1. F-3: IntentPlan.optimized_sort()
2. 时间权重拓扑排序
3. 测试: 3 个新测试

### Phase 4: Predictive Execution + Learning Loop (M4, ~1 day)

1. F-4: 预测预取触发
2. F-5: 学习环闭合
3. 测试: 6 个新测试

### Phase 5: Integration + Regression (~0.5 day)

1. 全量 1376+ 测试通过
2. E2E: declare intent → plan → execute → learn → predict → prefetch

---

## 7. 从 Node 22 到 Node 23 的推演

Node 22 完成后，Plico 将具备：
- 完整的学习闭环（执行结果 → AgentProfile）
- 自我优化的 plan 排序（执行时间感知）
- 预测执行（高置信度时自动预取）
- 真正的"越用越好"（公理9 接近 100%）

**Node 23 展望**: **成 (Completion) — 自主进化**

基于 Node 22 的"行"学习基础设施，Node 23 将攻坚：
1. **Autonomous Skill Acquisition**: 根据执行历史自动发现和注册新工具
2. **Self-Healing**: 根据失败历史自动调整 plan 策略
3. **Goal Decomposition**: 根据成功历史自动分解复杂 intent

**预期 Soul 对齐**: ~92% → **95%+**（公理9: 75%→95%, 公理7: 80%→90%）
