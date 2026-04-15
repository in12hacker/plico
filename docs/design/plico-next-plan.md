# Plico 开发计划 — Iteration 6

**Date**: 2026-04-15
**Based on**: `plico-kg-entity-design.md` (v0.2) + `plico-multi-hop-reasoning.md` (v0.1)

---

## 0. 现状盘点

| 组件 | 状态 |
|------|------|
| Phase 1: TemporalResolver + SearchFilter 时间过滤 | ✅ 已完成 |
| KG 基础 (KGNode/KGEdge + PetgraphBackend) | ✅ 已完成 |
| `upsert_document` (无实体提取) | ✅ 已完成 |
| SemanticSearch (InMemoryBackend, cosine) | ✅ 已完成 |
| Embedding (Ollama + LocalPython) | ✅ 已完成 |
| L0/L1/L2 层式上下文加载 | ✅ 已完成 |
| EventContainer + 事件关系边 | ✅ 已实现（iter6, 2026-04-15）|
| BehavioralObservation → UserFact pipeline | ❌ 未实现 |
| Preference 节点 + Preference Inference | ❌ 未实现 |
| ActionSuggestion + 通知集成 | ❌ 未实现 |

---

## 1. 下一轮开发范围（按依赖顺序）

```
Phase A  EventContainer（事件容器）
  A1   KGNodeType 新增 Event 变体
  A2   KGEdgeType 新增事件关系边类型
  A3   SemanticFS 新增 create_event / get_event / list_events
  A4   AIKernel 暴露事件 API
  A5   CLI 支持事件操作

Phase B  TemporalReasoningQuery（时序推理查询）
  B1   将 TemporalRange 整合进 SemanticFS 查询路径
  B2   支持 "上周和王总的会议" → event.start_time ∈ [上周]

Phase C  BehavioralObservation Pipeline（行为观察推断）
  C1   BehavioralObservation 数据结构
  C2   EventContainer 自动关联点餐/消费记录
  C3   UserFact 节点类型 + PatternExtraction
  C4   ProactiveAction 触发条件

Phase D  Preference 推理 + Multi-Hop（跨跳推理）
  D1   Preference 节点类型 + HAS_PREFERENCE 边
  D2   Preference Inference Pipeline（频率推断）
  D3   ActionSuggestion 节点 + 置信度
  D4   Multi-hop 查询：Event→Person→Preference→Action
```

---

## 2. Phase A 详细设计

### A1: KGNodeType 新增 Event 变体

```rust
// src/fs/graph.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGNodeType {
    // 现有
    Entity,
    Fact,
    Document,
    Agent,
    Memory,
    // 新增
    Event,         // 事件容器（内容聚合根）
    Person,        // 人物（现有视为 Entity 特殊化，待定）
}

// kg-entity-design.md §2.1 定义了 EventContainer，
// 但 KG 存储用 KGNode + metadata JSON 兼容
```

### A2: KGEdgeType 新增事件关系边

```rust
// src/fs/graph.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGEdgeType {
    AssociatesWith,
    Mentions,
    Follows,
    SimilarTo,
    // 新增（事件专用）
    HasAttendee,      // Event → Person
    HasDocument,       // Event → Document
    HasMedia,         // Event → Media
    HasDecision,      // Event → ActionItem
    FollowedBy,       // Event → Event（时间序）
    RelatedTo,        // 通用关系
}
```

### A3: SemanticFS 事件操作

```rust
// src/fs/semantic_fs.rs

impl SemanticFS {
    /// 创建事件容器（不创建 CAS 对象，只创建 KG 节点）
    pub fn create_event(
        &self,
        label: &str,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<&str>,
        attendee_ids: Vec<String>,
        tags: Vec<String>,
        agent_id: &str,
    ) -> Result<String, FSError> { ... }

    /// 按时间范围 + 标签查询事件
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> Result<Vec<EventSummary>, FSError> { ... }

    /// 事件添加关联对象
    pub fn event_attach(
        &self,
        event_id: &str,
        cid: &str,
        edge_type: EventEdgeType,
        agent_id: &str,
    ) -> Result<(), FSError> { ... }

    /// 事件添加参会人
    pub fn event_add_attendee(
        &self,
        event_id: &str,
        person_id: &str,
        agent_id: &str,
    ) -> Result<(), FSError> { ... }
}
```

### A4: AIKernel 事件 API

```rust
// src/kernel/mod.rs

impl AIKernel {
    pub fn create_event(&self, req: CreateEventRequest) -> Result<EventResponse, PlicoError>
    pub fn list_events(&self, query: EventQuery) -> Result<Vec<EventSummary>, PlicoError>
    pub fn event_attach(&self, event_id: &str, cid: &str, relation: &str) -> Result<(), PlicoError>
    pub fn event_add_attendee(&self, event_id: &str, person_id: &str) -> Result<(), PlicoError>
}
```

### A5: CLI 支持

```
aicli event create --label "Q2产品规划" --type meeting \
    --start 2026-04-10T14:00 \
    --tags "产品,规划" \
    --attendee 王总 --attendee 李经理

aicli event list --since 2026-04-01 --until 2026-04-30 --type meeting
aicli event attach <event-id> --cid <doc-cid> --relation has_document
aicli event add-attendee <event-id> --person 王总
```

---

## 3. Phase B: TemporalReasoningQuery

将 `TemporalRange` 整合进 `SemanticFS::list_events` 查询路径：

```rust
// 基于已有的 TemporalResolver，将"上周" → [since, until]
// 复用现有代码，只需在事件查询路径上应用时间过滤

pub fn list_events(&self, query: &TemporalRange, tags: &[String]) {
    // 已有: TemporalResolver.resolve("上周") → Some(TemporalRange)
    // 已有: SearchFilter.since/until 字段
    // 新增: list_events 用 same 逻辑过滤 KG Event 节点
}
```

---

## 4. Phase C: BehavioralObservation Pipeline

```rust
// 新文件: src/fs/behavior.rs

/// 行为观察记录（从点餐/消费记录自动生成）
struct BehavioralObservation {
    id: String,
    timestamp: u64,
    subject_id: String,        // 谁的行为
    context: String,            // "drunk" | "working" | "leisure"
    action_type: String,       // "order_food" | "browse" | "search"
    outcome: String,            // 点餐内容描述
    explicit_preference: Option<String>,  // 用户明确说的话
}

/// 模式提取：从多次观察推断 UserFact
async fn extract_pattern(
    observations: Vec<BehavioralObservation>,
) -> Vec<UserFact> {
    // 按 context 分组
    // 在每组内统计 outcome 频率
    // confidence = min(1.0, count / 3) × 时间衰减
    // → UserFact(trigger_context, predicate, object, confidence)
}

/// UserFact KG 节点
struct UserFact {
    id: String,
    subject_id: String,
    predicate: String,      // "prefers" | "dislikes" | "needs"
    object: String,         // "white_congee" | "wine"
    trigger_context: String,
    confidence: f32,
    evidence_cids: Vec<String>,
    created_at: u64,
}
```

---

## 5. Phase D: Preference + Multi-Hop Reasoning

```rust
// 基于 plico-multi-hop-reasoning.md §4

/// Preference 节点（Person 专属）
struct Preference {
    id: String,
    person_id: String,
    predicate: String,    // "prefers" | "dislikes" | "allergic_to"
    object: String,       // "wine" | "white_congee" | "spicy_food"
    context: String,      // "at_dinner" | "when_drunk" | "at_home"
    confidence: f32,
    evidence_cids: Vec<String>,
}

/// ActionSuggestion 节点
struct ActionSuggestion {
    id: String,
    trigger_event_id: String,
    target_person_id: String,
    action: String,
    reasoning_chain: Vec<String>,
    confidence: f32,
    status: SuggestionStatus,  // Pending / Confirmed / Dismissed
}

/// 推理路径：Event → Person → Preference → ActionSuggestion
async fn infer_and_suggest(
    event: &EventContainer,
    persons: &[Person],
) -> Vec<ActionSuggestion> {
    // 对每个 Person：
    //   1. 查询 HAS_PREFERENCE 边 → Preference 节点
    //   2. 检查 Preference.context 是否匹配 event.event_type
    //   3. 生成 ActionSuggestion + reasoning_chain
}
```

---

## 6. 依赖关系图

```
Phase A (EventContainer)
    │
    ├─► 依赖: 无（全新类型）
    │
    ▼
Phase B (TemporalReasoningQuery)
    ├─► 依赖: Phase A（已有 TemporalResolver）
    │
    ▼
Phase C (BehavioralObservation → UserFact)
    ├─► 依赖: Phase A（事件关联观察记录）
    │
    ▼
Phase D (Preference + Multi-Hop)
        依赖: Phase A + Phase C
```

---

## 7. 建议优先级

**优先级 1: Phase A（EventContainer）**
理由：Event 是所有其他功能的核心锚点。Phase 5 行为推断需要事件作为观察容器；Phase D 跨跳推理需要事件→人员→偏好路径。没有 EventContainer，其他 phases 都无法正常实现。

**优先级 2: Phase B（TemporalReasoningQuery）**
理由：Phase 1 的 TemporalResolver 已经实现，只需连接到事件查询路径。工作量小，收益大。

**优先级 3: Phase C（BehavioralObservation）**
理由：这是 Plico 与传统文件系统的核心差异 — AI 自我迭代能力。

**优先级 4: Phase D（Preference + Multi-Hop）**
理由：最后一步，整合前三个 phases 的能力。

---

## 8. Phase A 实施步骤

```
Step A1: 修改 KGNodeType + KGEdgeType（graph.rs）
Step A2: 实现 SemanticFS::create_event / get_event / list_events
Step A3: 实现 SemanticFS::event_attach / event_add_attendee
Step A4: AIKernel 包装层
Step A5: CLI 命令（event create / list / attach）
Step A6: 集成测试 + cargo test
Step A7: 更新 fs/INDEX.md + AGENTS.md
```

每步单独测试，全部通过后进入下一步。
