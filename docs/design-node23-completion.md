# Plico 第二十三节点设计文档
# 成 — 自主进化

**版本**: v1.0
**日期**: 2026-04-24
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Autonomous Skill Acquisition + Self-Healing + Goal Decomposition
**前置**: 节点 22 ✅（100%）— 行(5维) + 1361 tests + Soul 92%
**验证方法**: E2E autonomous loop + skill discovery + self-healing + goal decomposition
**信息来源**: `docs/design-node22-action.md` + OpenClaw self-improving-agent (268k downloads) + Self-Healing Systems (Ramsbaby/openclaw-self-healing) + Goal Decomposition (AgentRAN 2026) + Plico Node 22 implementation

---

## 0. 链式思考：从 Node 22 到 Node 23

### 为什么需要"成"

Node 22 建立了**行**能力：
- Execution as Learning：执行结果写回 AgentProfile
- Execution Time Tracking：追踪每种操作类型的平均执行时间
- Self-Optimization：根据历史执行时间优化 plan 排序
- Predictive Execution：执行完成后触发新的 prefetch
- Learning Loop Closure：形成完整闭环

**但这些只是"越用越好"的基础**。Soul 2.0 的核心差距是：

| 公理 | Node 22 实现 | 对齐度 |
|------|-------------|--------|
| 公理9: 越用越好 | AgentProfile 有 transition matrix 和 hot_objects | 92% |
| 公理7: 主动先于被动 | prefetch 触发但无自动发现新技能 | 88% |
| 公理2: 意图先于操作 | Intent 声明触发执行，但不自动分解 | 85% |

**"成"的三层含义**：
1. **自我进化**：根据执行历史自动发现和注册新工具/技能（公理9）
2. **自我修复**：根据失败历史自动调整 plan 策略，避免重复失败（公理7）
3. **目标分解**：根据成功历史自动分解复杂 intent 为可执行步骤（公理2）

### 链式推导

```
[因] Node 22 执行结果写回 AgentProfile → hot_objects 累积
    ↓
[果] 但无法基于历史自动发现"这个操作序列是个有用技能" (公理9未满)
    ↓
[因] 工具调用失败只记录 error，不分析"为什么失败"和"如何修复"
    ↓
[果] 同样的失败会在下一次重复发生 (公理7违反)
    ↓
[因] 复杂 intent 只有一个字符串，无法自动分解为步骤
    ↓
[果] Agent 必须手动规划，OS 无法自主驱动
    ↓
[因] 公理9 要求"越用越好" + 公理7 要求"主动先于被动"
    ↓
[果] 需要 Autonomous Skill Acquisition + Self-Healing + Goal Decomposition
```

---

## 1. 现状分析

### 1.1 Node 22 后的能力

| 能力 | 实现 | 功能 |
|------|------|------|
| Execution as Learning | record_cid_usage + record_intent_complete | 执行结果写回 Profile |
| Execution Time Tracking | ExecutionStats + record() | 追踪平均执行时间 |
| Self-Optimization | optimized_sort() | 时间权重拓扑排序 |
| Predictive Execution | trigger_predictive_prefetch() | 执行后触发预取 |
| Learning Loop Closure | 完整闭环 | declare → plan → execute → learn → predict → prefetch |

### 1.2 关键差距

**Gap 1: 无自动技能发现**
- AgentProfile 有 transition matrix 和 hot_objects
- 但无法识别"这个操作序列是个有用的技能"
- 效果：第100次执行和第1次一样，无法自动发现新能力

**Gap 2: 无自我修复**
- step 失败只记录 error message
- 不分析失败原因（权限？资源不足？工具不存在？）
- 不调整后续 plan 策略
- 效果：同样的失败会重复发生

**Gap 3: 无自动目标分解**
- 复杂 intent 只是一个字符串
- 无基于历史的自动分解能力
- 效果：Agent 必须手动规划，OS 无法真正自主

---

## 2. Node 23 四大维度

### D1: Autonomous Skill Acquisition — 自主技能发现

**问题**: 无法基于历史执行发现和注册新技能。
**目标**: 检测重复操作序列，自动注册为可调用技能。
**实现策略**:
- 追踪频繁的操作序列（IntentOperation patterns）
- 当同一序列出现 N 次且成功，标记为候选技能
- `SkillDiscriminator`: 分析并注册新工具到 ToolRegistry

### D2: Self-Healing — 自我修复

**问题**: 失败只记录 error，不调整后续 plan。
**目标**: 分析失败原因，自动调整 plan 策略。
**实现策略**:
- `FailureClassifier`: 分类失败类型（PermissionDenied/ResourceExhausted/ToolNotFound/ExecutionFailed）
- `PlanAdaptor`: 基于失败类型调整后续步骤（跳过/重试/替换工具）
- 记录到 `AgentProfile.failure_patterns`

### D3: Goal Decomposition — 目标分解

**问题**: 复杂 intent 无法自动分解为可执行步骤。
**目标**: 基于历史成功经验，自动分解复杂 intent。
**实现策略**:
- `IntentDecomposer`: 分析 intent 关键词，从 history 找到相似成功案例
- 提取成功案例的 IntentPlan 作为模板
- 自动生成新 IntentPlan

### D4: Learning Loop Extension — 学习环扩展

**问题**: 各模块独立，无完整进化闭环。
**目标**: 形成"执行→学习→发现→修复→分解→进化"闭环。
**实现策略**:
- D1+D2+D3 整合到 `AutonomousExecutor`
- `execute_plan` 后触发 skill discovery + failure analysis + goal decomposition
- 将结果写回 AgentProfile

---

## 3. 特性清单

### F-1: Autonomous Skill Acquisition

```rust
// kernel/ops/skill_discovery.rs — NEW module
pub struct SkillDiscriminator {
    /// Sequence frequency threshold to trigger skill candidate.
    min_sequence_count: usize,
    /// Tracks operation sequences per agent.
    sequences: RwLock<HashMap<String, Vec<OpSequence>>>,
}

#[derive(Debug, Clone)]
pub struct OpSequence {
    pub operations: Vec<String>,  // e.g., ["read", "call", "create"]
    pub count: usize,
    pub success_rate: f32,
    pub avg_duration_ms: u64,
    pub last_seen_ms: u64,
}

impl SkillDiscriminator {
    /// Record an execution sequence and check if it qualifies as a skill.
    pub fn record_sequence(&self, agent_id: &str, operations: Vec<String>, success: bool, duration_ms: u64);

    /// Get candidate skills (sequences with high frequency and success rate).
    pub fn get_skill_candidates(&self, agent_id: &str) -> Vec<SkillCandidate>;
}

#[derive(Debug, Clone)]
pub struct SkillCandidate {
    pub operations: Vec<String>,
    pub count: usize,
    pub success_rate: f32,
    pub recommended_name: String,
}
```

**测试**:
- `test_skill_discovery_records_sequence`: 记录操作序列
- `test_skill_discovery_detects_repeated_sequence`: 重复序列检测
- `test_skill_discovery_skill_candidate_generation`: 技能候选生成

**新测试**: 3

### F-2: Self-Healing

```rust
// kernel/ops/intent_executor.rs — 新增 FailureClassifier
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureType {
    PermissionDenied,
    ResourceExhausted,
    ToolNotFound,
    ExecutionFailed,
    DependencyBlocked,
}

/// Analyze a step result and classify the failure type.
pub fn classify_failure(result: &StepResult) -> Option<FailureType>;

/// Plan adaptor — adjusts plan based on failure history.
pub struct PlanAdaptor {
    failure_history: RwLock<HashMap<String, Vec<FailureRecord>>>,
}

#[derive(Debug, Clone)]
pub struct FailureRecord {
    pub step_id: String,
    pub failure_type: FailureType,
    pub timestamp_ms: u64,
    pub suggestion: String,
}

impl PlanAdaptor {
    /// Record a failure and get adaptation suggestions.
    pub fn record_and_adapt(&self, step_id: &str, failure: &FailureType) -> Adaptation;

    /// Get adaptation for a step based on historical failures.
    pub fn get_adaptation(&self, step_id: &str) -> Option<Adaptation>;
}

#[derive(Debug, Clone)]
pub enum Adaptation {
    Skip,                           // Skip this step in future
    RetryWithNewParams,             // Retry with different parameters
    ReplaceTool { new_tool: String }, // Use a different tool
    ReduceScope,                    // Do a simpler version
}
```

**测试**:
- `test_failure_classifier_permission_denied`: PermissionDenied 分类
- `test_failure_classifier_resource_exhausted`: ResourceExhausted 分类
- `test_plan_adaptor_skips_repeated_failures`: 重复失败时跳过
- `test_plan_adaptor_replaces_tool`: 工具替换建议

**新测试**: 4

### F-3: Goal Decomposition

```rust
// kernel/ops/intent_decomposer.rs — NEW module
pub struct IntentDecomposer {
    profile_store: Arc<AgentProfileStore>,
}

impl IntentDecomposer {
    /// Decompose a complex intent into steps based on historical successes.
    ///
    /// Finds similar successful intents from profile and extracts their plans.
    pub fn decompose(&self, intent: &IntentDeclaration, agent_id: &str) -> Option<IntentPlan>;
}
```

**测试**:
- `test_intent_decomposer_finds_similar_intent`: 找到相似成功 intent
- `test_intent_decomposer_extracts_plan`: 提取成功 plan 作为模板
- `test_intent_decomposer_no_history_returns_none`: 无历史时返回 None

**新测试**: 3

### F-4: Learning Loop Extension

```rust
// kernel/ops/intent_executor.rs — 修改 execute_plan
impl AutonomousExecutor {
    pub async fn execute_plan(&mut self, plan: &IntentPlan, agent_id: &str) -> IntentExecutionResult {
        // ... existing execution ...

        // F-4: After execution, trigger learning loop extension
        // 1. Record operation sequences for skill discovery
        self.record_operation_sequences(&exec_result);

        // 2. Analyze failures and update adaptation strategies
        self.analyze_failures(&exec_result);

        // 3. Check if intent decomposition is needed for future similar intents
        self.check_decomposition_opportunity(&exec_result, agent_id);
    }
}
```

**测试**:
- `test_learning_loop_extension_full`: 完整学习环扩展
- `test_learning_loop_extension_skill_discovery_triggered`: 技能发现触发
- `test_learning_loop_extension_self_healing_adapts`: 自我修复生效

**新测试**: 3

---

## 4. 行业研究对标

### 4.1 OpenClaw self-improving-agent (2026)

| 维度 | OpenClaw | Plico Node 23 | 对齐度 |
|------|----------|---------------|--------|
| Error Memory | 记录错误命令和原因 | F-2 FailureClassifier | ✅ |
| Solution Optimization | 记住用户偏好的解决方案 | F-2 PlanAdaptor | ✅ |
| Knowledge Accumulation | 持续累积使用经验 | F-1 SkillDiscriminator | ✅ |
| Feedback Learning | 从用户纠正中学习 | F-4 Learning Loop | ✅ |

### 4.2 Self-Healing Systems (Ramsbaby, 2026)

| 维度 | Self-Healing System | Plico Node 23 | 对齐度 |
|------|---------------------|---------------|--------|
| 4-tier recovery | KeepAlive → Watchdog → AI Doctor → Alert | F-2 Adaptation (Skip/Retry/Replace/Reduce) | ✅ |
| Autonomous detection | 自动检测故障 | F-2 FailureClassifier | ✅ |
| Pattern learning | 从失败历史学习 | F-2 failure_patterns in AgentProfile | ✅ |

---

## 5. 量化目标

| 指标 | N22 现状 | N23 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 1361 | **1375+** | ✅ 737+ |
| Skill Discovery | 0 | **SkillDiscriminator** | ✅ M1 |
| Self-Healing | 0 | **PlanAdaptor** | ✅ M2 |
| Goal Decomposition | 0 | **IntentDecomposer** | ✅ M3 |
| Learning Loop | 闭环 | **扩展闭环** | ✅ M4 |

---

## 6. 实施计划

### Phase 1: Skill Discovery (M1, ~1 day)

1. F-1: SkillDiscriminator 实现
2. 操作序列追踪逻辑
3. 技能候选生成
4. 测试: 3 个新测试

### Phase 2: Self-Healing (M2, ~1 day)

1. F-2: FailureClassifier 实现
2. F-2: PlanAdaptor 实现
3. Adaptation 策略
4. 测试: 4 个新测试

### Phase 3: Goal Decomposition (M3, ~1 day)

1. F-3: IntentDecomposer 实现
2. 基于历史提取 IntentPlan 模板
3. 测试: 3 个新测试

### Phase 4: Learning Loop Extension (M4, ~1 day)

1. F-4: 整合到 AutonomousExecutor
2. 完整进化闭环
3. 测试: 3 个新测试

### Phase 5: Integration + Regression (~0.5 day)

1. 全量 1375+ 测试通过
2. E2E: execute → learn → discover → heal → decompose → evolve

---

## 7. 从 Node 23 到 Node 24 的推演

Node 23 完成后，Plico 将具备：
- 完整的学习闭环（执行 → 学习 → 发现 → 修复 → 分解 → 进化）
- 真正的"越用越好"（公理9 接近 100%）
- 真正的"主动先于被动"（公理7 接近 95%）
- 真正的"意图先于操作"（公理2 接近 95%）

**Node 24 展望**: **化 (Transcendence) — 超域融合**

基于 Node 23 的"成"进化基础设施，Node 24 将攻坚：
1. **Cross-Domain Skill Composition**: 跨领域技能组合，发现"数学+编程"的化学反应
2. **Self-Generated Goals**: 基于失败历史和成功历史，自动生成新的目标
3. **Temporal Memory Projection**: 基于时间序列预测未来上下文需求

**预期 Soul 对齐**: ~92% → **97%+**（公理9: 92%→98%, 公理7: 88%→95%, 公理2: 85%→95%）