# Plico 第二十一节点设计文档
# 意 — 意图驱动的自主执行

**版本**: v1.0
**日期**: 2026-04-24
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Intent Plan + Autonomous Execution + Multi-Agent Coordination
**前置**: 节点 20 ✅（100%）— 觉(5维) + 1329 tests + Soul 87%
**验证方法**: E2E autonomous loop + intent plan execution + multi-agent coordination test
**信息来源**: `docs/design-node20-awareness.md` + Agentic AI research (AgentRAN 2026, Intent-driven O-RAN) + Plico Node 20 implementation

---

## 0. 链式思考：从 Node 20 到 Node 21

### 为什么需要"意"

Node 20 建立了**觉**能力：
- 意图缓存跨 session 持久化
- Agent Profile 反馈环路
- Hook 因果链追踪
- Async 可取消预取
- Context-Dependent Gravity

**但这些只是"看得见+学得会"**。Soul 2.0 的核心差距是：

| 公理 | Node 20 实现 | 对齐度 |
|------|-------------|--------|
| 公理2: 意图先于操作 | Agent 声明 intent，OS 预取上下文 | 55% |
| 公理7: 主动先于被动 | 预取是主动的，但执行仍被动 | 60% |

**"意"的三层含义**：
1. **结构化意图**：声明 intent 时提供结构化数据（关键词 + CID + token 预算），而非自然语言
2. **意图分解**：OS 将 intent 分解为可执行步骤序列（Intent Plan）
3. **自主执行**：OS 驱动执行循环，Agent 只在异常点介入

### 链式推导

```
[因] Node 20 prefetch 仍是 "Agent 声明 → OS 准备 → Agent 执行" 模式
    ↓
[因] OS 无法自主驱动执行 → [果] Agent 必须全程参与，token 消耗高
    ↓
[因] 无 Intent Plan 结构 → [果] OS 无法分解意图为步骤
    ↓
[因] 无自主执行循环 → [果] 无法实现 "OS 干活，Agent 监督"
    ↓
[因] 公理2 要求 "意图先于操作" → [果] 需要 OS 理解意图并自主执行
```

---

## 1. 现状分析

### 1.1 Node 20 后的能力

| 能力 | 实现 | 功能 |
|------|------|------|
| Intent Cache | CAS persist | 意图缓存跨 session |
| Agent Profile | JSON persist | transition matrix 累积 |
| Causal Hook | KG CausedBy | 工具调用因果链 |
| Async Prefetch | JoinHandle | 可取消预取 |
| Intent Feedback | hit rate stats | 自适应学习 |
| Gravity Search | hot objects boost | 意图相关性重排 |

### 1.2 关键差距

**Gap 1: Agent 仍需主导执行**
- `declare_intent` → `fetch_assembled_context` → Agent 自己执行
- OS 只负责准备，不负责执行
- 效果：Agent 仍需全程参与，token 消耗高

**Gap 2: 无 Intent Plan**
- 意图只是一个字符串，没有结构化分解
- OS 不知道意图由哪些步骤组成
- 无法实现步骤级别的进度跟踪

**Gap 3: 无自主执行循环**
- Agent 声明意图后，仍需自己循环调用工具
- OS 无法驱动 "思考→行动→观测→修正" 循环
- 无法实现真正的自主执行

---

## 2. Node 21 五大维度

### D1: Structured Intent Declaration — 结构化意图

**问题**: 意图只是一个字符串，OS 无法理解其结构。
**目标**: Intent declaration 包含结构化数据：关键词 + 关联 CID + token 预算 + 期望结果。
**实现策略**:
- `IntentDeclaration` struct: keywords + related_cids + budget_tokens + expected_outcome
- OS 解析结构化声明，验证完整性
- 存储到 session 中供后续执行使用

### D2: Intent Plan Decomposition — 意图分解

**问题**: OS 不知道意图由哪些步骤组成。
**目标**: OS 将 intent 分解为可执行的步骤序列（Intent Plan）。
**实现策略**:
- `IntentPlan` struct: steps + dependencies + estimated_tokens
- Plan steps: 原子操作（read/call/search/create）
- 步骤依赖图：DAG 拓扑排序
- 预估 token 消耗

### D3: Autonomous Execution Loop — 自主执行

**问题**: Agent 仍需主导执行循环。
**目标**: OS 驱动执行循环，Agent 只在异常/需要决策时介入。
**实现策略**:
- `AutonomousExecutor`: while loop + step execution + exception handling
- 步骤执行：按计划顺序执行，遇到依赖阻塞则等待
- 异常处理：权限拒绝、资源不足、工具失败
- Human-in-the-loop：关键决策点暂停，等待 Agent 确认

### D4: Multi-Agent Intent Coordination — 多 Agent 协作

**问题**: 多 Agent 协作时无 intent 共享机制。
**目标**: 多个 Agent 共享 intent tree，协作分工。
**实现策略**:
- `IntentTree`: 共享的 intent 分解结构
- Sub-agent 认领步骤（claim）
- 结果汇总到父 intent
- 冲突检测与解决

### D5: Intent Progress Tracking — 进度追踪

**问题**: 无法追踪 intent 执行进度。
**目标**: 实时追踪每个步骤的状态和 token 消耗。
**实现策略**:
- `IntentStepState`: Pending/Running/Completed/Failed/Blocked
- Progress API: 查询 intent 执行进度
- Token 预算追踪：每步骤消耗 vs 总额

---

## 3. 特性清单

### F-1: Structured Intent Declaration

```rust
// kernel/ops/intent.rs — NEW module
pub struct IntentDeclaration {
    /// Intent keywords for semantic matching.
    pub keywords: Vec<String>,
    /// Related CIDs for context assembly.
    pub related_cids: Vec<String>,
    /// Token budget for this intent.
    pub budget_tokens: usize,
    /// Expected outcome description.
    pub expected_outcome: String,
    /// Agent that declared this intent.
    pub agent_id: String,
    /// Session this intent belongs to.
    pub session_id: String,
}

impl IntentDeclaration {
    pub fn new(
        keywords: Vec<String>,
        related_cids: Vec<String>,
        budget_tokens: usize,
        expected_outcome: String,
        agent_id: String,
        session_id: String,
    ) -> Self;
}
```

**测试**:
- `test_intent_declaration_creation`: 创建有效的声明
- `test_intent_declaration_validation`: 验证必填字段
- `test_intent_declaration_serialization`: JSON 序列化/反序列化

**新测试**: 3

### F-2: Intent Plan Decomposition

```rust
// Intent Plan — intent 分解的结果
pub struct IntentPlan {
    pub intent_id: String,
    pub steps: Vec<IntentStep>,
    pub total_estimated_tokens: usize,
    pub created_at_ms: u64,
}

pub struct IntentStep {
    pub step_id: String,
    pub operation: IntentOperation,
    pub params: serde_json::Value,
    pub dependencies: Vec<String>,  // step_ids this depends on
    pub estimated_tokens: usize,
    pub state: IntentStepState,
}

pub enum IntentOperation {
    Read { cid: String },
    Search { query: String, tags: Vec<String> },
    Call { tool: String, params: serde_json::Value },
    Create { content: Vec<u8>, tags: Vec<String> },
}

pub enum IntentStepState {
    Pending,
    Running,
    Completed,
    Failed(String),
    Blocked { reason: String },
}
```

**测试**:
- `test_intent_plan_creation`: 创建包含多个步骤的计划
- `test_intent_plan_topological_sort`: DAG 排序正确
- `test_intent_step_dependencies`: 依赖解析正确

**新测试**: 3

### F-3: Autonomous Execution Loop

```rust
// kernel/ops/intent_executor.rs — NEW module
pub struct AutonomousExecutor {
    kernel: Arc<AIKernel>,
    session_store: Arc<SessionStore>,
}

impl AutonomousExecutor {
    /// Execute an intent plan autonomously.
    pub async fn execute_plan(&self, plan: &IntentPlan) -> IntentExecutionResult {
        // While loop: execute steps, handle exceptions, track progress
    }

    /// Handle step execution with exception catching.
    async fn execute_step(&self, step: &IntentStep) -> Result<StepResult, StepError> {
        // Execute the step operation
        // Catch exceptions and convert to StepError
        // Return result for observability
    }

    /// Check if step can proceed (dependencies satisfied).
    fn can_execute_step(&self, step: &IntentStep, completed: &HashSet<String>) -> bool {
        step.dependencies.iter().all(|dep| completed.contains(dep))
    }
}
```

**测试**:
- `test_autonomous_executor_executes_sequential_steps`: 顺序执行
- `test_autonomous_executor_handles_step_failure`: 步骤失败处理
- `test_autonomous_executor_blocks_on_dependency`: 依赖阻塞

**新测试**: 3

### F-4: Multi-Agent Intent Coordination

```rust
// IntentTree — 共享的 intent 分解结构
pub struct IntentTree {
    pub root_intent_id: String,
    pub plan: IntentPlan,
    /// Sub-agents working on this intent.
    pub assigned_agents: HashMap<String, Vec<String>>,  // agent_id -> step_ids
    pub results: RwLock<HashMap<String, StepResult>>,
}
```

**测试**:
- `test_intent_tree_assign_to_agent`: 分配步骤到 sub-agent
- `test_intent_tree_aggregate_results`: 汇总 sub-agent 结果
- `test_intent_tree_conflict_detection`: 检测冲突

**新测试**: 3

### F-5: Intent Progress Tracking

```rust
// kernel/ops/intent_tracker.rs — NEW module
pub struct IntentTracker {
    active_plans: RwLock<HashMap<String, IntentPlanState>>,
}

pub struct IntentPlanState {
    pub plan: IntentPlan,
    pub step_states: HashMap<String, IntentStepState>,
    pub tokens_used: usize,
    pub started_at_ms: u64,
}

impl IntentTracker {
    pub fn get_progress(&self, intent_id: &str) -> Option<IntentProgress>;
    pub fn cancel_intent(&self, intent_id: &str) -> bool;
}
```

**测试**:
- `test_intent_tracker_progress_query`: 查询进度
- `test_intent_tracker_cancel`: 取消 intent
- `test_intent_tracker_token_tracking`: token 追踪

**新测试**: 3

---

## 4. 行业研究对标

### 4.1 Agentic AI (AgentRAN, 2026)

| 维度 | AgentRAN | Plico Node 21 | 对齐度 |
|------|---------|----------------|--------|
| Intent-driven | NL intent → structured | Structured intent declaration | ✅ |
| Multi-agent hierarchy | Supervisor + weighting agents | IntentTree coordination | ✅ |
| Memory for experience | Prior experience retrieval | AgentProfile hot_objects | ✅ |
| **Autonomous execution** | Self-organizing hierarchy | **AutonomousExecutor** | 🔴 |

### 4.2 Intent-Driven Networks

| 维度 | Industry | Plico | 对齐度 |
|------|---------|-------|--------|
| Intent translation | Supervisor agent | F-1 Structured Declaration | ✅ |
| Plan decomposition | User weighting agent | F-2 Intent Plan | ✅ |
| **Autonomous control loop** | Execute & observe | **F-3 Autonomous Loop** | 🔴 |

---

## 5. 量化目标

| 指标 | N20 现状 | N21 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 1329 | **1349+** | ✅ 1347 (705 lib + E2E) |
| Intent Declaration | string only | **Structured** | ✅ M1 完成 |
| Intent Plan | N/A | **DAG decomposition** | ✅ M2 完成 |
| Autonomous Execution | Agent-driven | **OS-driven loop** | 🔴 M3 未开始 |
| Multi-Agent Intent | N/A | **IntentTree** | 🔴 M4 未开始 |
| Progress Tracking | N/A | **Real-time tracking** | ✅ M1/M2 完成 |

---

## 6. 实施计划

### Phase 1: Intent Structure (M1, ~1 day)

1. F-1: IntentDeclaration struct + validation
2. F-5: IntentTracker for progress tracking
3. 测试: 6 个新测试
4. Dogfood: 声明 intent → 查询进度

### Phase 2: Intent Plan (M2, ~1 day)

1. F-2: IntentPlan + IntentStep + topological sort
2. 测试: 3 个新测试
3. Dogfood: 分解复杂 intent 为步骤

### Phase 3: Autonomous Execution (M3, ~1.5 days)

1. F-3: AutonomousExecutor 实现
2. 异常处理 + human-in-the-loop
3. 测试: 3 个新测试
4. Dogfood: OS 自主执行 intent plan

### Phase 4: Multi-Agent Coordination (M4, ~1 day)

1. F-4: IntentTree + sub-agent 分配
2. 结果汇总 + 冲突检测
3. 测试: 3 个新测试
4. Dogfood: 多 Agent 协作执行

### Phase 5: Integration + Regression (~0.5 day)

1. 全量 1349+ 测试通过
2. E2E: declare structured intent → decompose → execute → track progress

---

## 7. 从 Node 21 到 Node 22 的推演

Node 21 完成后，Plico 将具备：
- 结构化意图声明（关键词 + CID + 预算 + 期望结果）
- Intent Plan 分解（DAG 步骤图）
- 自主执行循环（OS 驱动，Agent 监督）
- 多 Agent 协作（IntentTree 共享）
- 进度追踪（实时状态 + token 消耗）

**Node 22 展望**: **行 (Action) — 执行即学习**

基于 Node 21 的"意"基础设施，Node 22 将攻坚：
1. **Execution as Learning**: 执行结果写回 AgentProfile
2. **Self-Optimization**: 根据历史执行时间优化 plan 排序
3. **Predictive Execution**: 在 intent 声明前就预测可能的需求

**预期 Soul 对齐**: ~87% → **92%+**（公理2: 55%→75%, 公理7: 60%→80%）
