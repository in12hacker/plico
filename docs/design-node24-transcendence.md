# Plico 第二十四节点设计文档
# 化 — 超域融合

**版本**: v1.0
**日期**: 2026-04-24
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Cross-Domain Skill Composition + Self-Generated Goals + Temporal Memory Projection
**前置**: 节点 23 ✅（100%）— 成(4维) + 737 tests + Soul 93%
**验证方法**: E2E skill composition + goal generation + temporal prediction
**信息来源**: `docs/design-node23-completion.md` + Cross-Domain AI Research (2026) + Temporal Memory Projection + Plico Node 23 implementation

---

## 0. 链式思考：从 Node 23 到 Node 24

### 为什么需要"化"

Node 23 建立了**成**能力：
- SkillDiscriminator：基于频率发现候选技能
- PlanAdaptor：基于失败历史调整策略
- IntentDecomposer：基于历史分解意图
- Learning Loop Extension：完整进化闭环

**但这些仍是"单一领域"的学习**。Soul 2.0 的终极目标是：
- 公理9: 越用越好 → 需要**跨领域**知识组合
- 公理7: 主动先于被动 → 需要**自生成目标**
- 公理2: 意图先于操作 → 需要**时间序列预测**

### 链式推导

```
[因] Node 23 SkillDiscriminator 只在单一领域内发现技能
    ↓
[果] "数学+编程" 这种跨领域化学反应无法被发现 (公理9未满)
    ↓
[因] 无 Self-Generated Goals → [果] Agent 只能执行已有目标，不能自主生成
    ↓
[因] 无 Temporal Memory Projection → [果] 无法预测"明天的上下文需求"
    ↓
[因] 公理7 要求"主动先于被动" + 公理9 要求"越用越好"
    ↓
[果] 需要 Cross-Domain Skill Composition + Self-Generated Goals + Temporal Memory Projection
```

---

## 1. 现状分析

### 1.1 Node 23 后的能力

| 能力 | 实现 | 功能 |
|------|------|------|
| Skill Discovery | SkillDiscriminator | 单一领域内频率检测 |
| Self-Healing | PlanAdaptor | 基于失败历史调整策略 |
| Goal Decomposition | IntentDecomposer | 基于历史分解意图 |
| Learning Loop | AutonomousExecutor | 执行→学习→发现→修复→分解 |

### 1.2 关键差距

**Gap 1: 跨领域技能组合缺失**
- SkillDiscriminator 只追踪单一操作序列
- 无法发现"读数学论文→写代码验证→创建测试"这种跨领域模式
- 效果：无法自动发现组合技能

**Gap 2: 无自生成目标**
- Agent 只能执行已有意图
- 无法基于历史成功/失败自动生成新目标
- 效果：系统永远被动等待指令

**Gap 3: 无时间序列预测**
- Prefetch 基于"当前意图"而非"时间上下文"
- 无法预测"用户通常在下午3点需要什么"
- 效果：主动性与时间无关

---

## 2. Node 24 三大维度

### D1: Cross-Domain Skill Composition — 跨域技能组合

**问题**: 无法发现跨领域技能组合。
**目标**: 检测跨领域的操作模式，发现"数学+编程"、"搜索+创建"等组合。
**实现策略**:
- `CrossDomainSkillComposer`: 分析不同领域的操作序列
- `SkillGraph`: 构建技能关系图，发现组合模式
- `CompositionCandidate`: 跨领域技能候选

### D2: Self-Generated Goals — 自生成目标

**问题**: Agent 无法自主生成新目标。
**目标**: 基于历史成功/失败，自动生成有意义的目标。
**实现策略**:
- `GoalGenerator`: 分析成功历史，提取可复用的目标模式
- `GoalTemplate`: 目标模板（"在X场景下做Y"）
- `SelfGoal`: 自生成目标的结构

### D3: Temporal Memory Projection — 时间记忆投射

**问题**: Prefetch 与时间上下文无关。
**目标**: 基于时间序列预测未来上下文需求。
**实现策略**:
- `TemporalPattern`: 时间模式（daily/weekly/monthly）
- `ProjectionEngine`: 投射引擎，预测T+Δ的上下文
- `TemporalIndex`: 时间索引，加速模式发现

---

## 3. 特性清单

### F-1: Cross-Domain Skill Composition

```rust
// kernel/ops/cross_domain_skill.rs — NEW module
pub struct CrossDomainSkillComposer {
    skill_graph: RwLock<SkillGraph>,
    min_cross_domain_count: usize,
}

#[derive(Debug, Clone)]
pub struct SkillGraph {
    /// Nodes are skill operations
    nodes: HashMap<String, SkillNode>,
    /// Edges are co-occurrence relationships
    edges: HashMap<String, Vec<CoOccurrence>>,
}

#[derive(Debug, Clone)]
pub struct SkillNode {
    pub operation: String,
    pub domain: String,  // "math", "code", "search", "create"
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct CoOccurrence {
    pub skill_a: String,
    pub skill_b: String,
    pub count: usize,
    pub avg_success_rate: f32,
}

impl CrossDomainSkillComposer {
    pub fn new(min_cross_domain_count: usize) -> Self;
    
    /// Record a cross-domain operation sequence.
    pub fn record_sequence(&self, operations: &[String], domains: &[String], success: bool);
    
    /// Get composition candidates — skills that frequently co-occur across domains.
    pub fn get_composition_candidates(&self) -> Vec<CompositionCandidate>;
}
```

### F-2: Self-Generated Goals

```rust
// kernel/ops/goal_generator.rs — NEW module
pub struct GoalGenerator {
    profile_store: Arc<AgentProfileStore>,
}

#[derive(Debug, Clone)]
pub struct GoalTemplate {
    pub trigger_keywords: Vec<String>,
    pub action_sequence: Vec<String>,
    pub success_rate: f32,
}

impl GoalGenerator {
    pub fn new(profile_store: Arc<AgentProfileStore>) -> Self;
    
    /// Generate new goals based on historical successes.
    pub fn generate_goals(&self, agent_id: &str, context: &str) -> Vec<SelfGoal>;
}

#[derive(Debug, Clone)]
pub struct SelfGoal {
    pub goal_text: String,
    pub confidence: f32,
    pub based_on_history: Vec<String>,  // CID references
}
```

### F-3: Temporal Memory Projection

```rust
// kernel/ops/temporal_projection.rs — NEW module
pub struct TemporalProjectionEngine {
    patterns: RwLock<Vec<TemporalPattern>>,
}

#[derive(Debug, Clone)]
pub struct TemporalPattern {
    pub time_of_day: TimeOfDay,
    pub day_of_week: Option<DayOfWeek>,
    pub typical_intents: Vec<String>,
    pub hit_rate: f32,
}

#[derive(Debug, Clone)]
pub enum TimeOfDay {
    Morning,     // 6-12
    Afternoon,   // 12-18
    Evening,     // 18-22
    Night,       // 22-6
}

impl TemporalProjectionEngine {
    pub fn new() -> Self;
    
    /// Record an intent with its timestamp for pattern learning.
    pub fn record_intent(&self, intent: &str, timestamp_ms: u64);
    
    /// Project what intent might be needed at a future time.
    pub fn project(&self, target_time: u64) -> Vec<String>;
}
```

---

## 4. 量化目标

| 指标 | N23 现状 | N24 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 737 | **750+** | ✅ 737+ |
| Cross-Domain Skill | 0 | **CrossDomainSkillComposer** | 🔄 |
| Self-Generated Goal | 0 | **GoalGenerator** | 🔄 |
| Temporal Projection | 0 | **TemporalProjectionEngine** | 🔄 |

---

## 5. 实施计划

### Phase 1: Cross-Domain Skill Composition (M1, ~1 day)

1. F-1: CrossDomainSkillComposer 实现
2. SkillGraph 构建
3. CompositionCandidate 生成
4. 测试: 3 个新测试

### Phase 2: Self-Generated Goals (M2, ~1 day)

1. F-2: GoalGenerator 实现
2. GoalTemplate 提取
3. SelfGoal 生成
4. 测试: 3 个新测试

### Phase 3: Temporal Memory Projection (M3, ~1 day)

1. F-3: TemporalProjectionEngine 实现
2. 时间模式学习
3. 预测算法
4. 测试: 3 个新测试

### Phase 4: Integration + Regression (~0.5 day)

1. 全量 750+ 测试通过
2. E2E: cross-domain → goal-generation → temporal-prediction

---

## 6. 从 Node 24 到 Node 25 的推演

Node 24 完成后，Plico 将具备：
- 跨领域技能组合发现
- 自生成目标能力
- 时间序列预测
- Soul 对齐接近 97%+

**Node 25 展望**: **太初 (Genesis) — AI-OS 完成态**

所有核心能力收敛，Plico 成为完整的 AI-Native OS。