# Plico Multi-Hop Reasoning & Cross-Context Inference Design

**Version**: 0.1 (draft)
**Date**: 2026-04-15
**Status**: Design — not yet implemented

---

## 1. 核心场景

### 场景 1：会议 → 决议（已在 plico-kg-entity-design.md §9）

用户："前几天开的Q2产品规划会议，帮我生成PPT"

```
输入 → TemporalResolver → EventContainer
    → explore(HAS_DECISION) → 决议列表
    → explore(HAS_DOCUMENT) → 幻灯片CID
    → explore(HAS_MEDIA) → 照片 → Vision LLM → 白板内容
```

单跳：事件 → 关联内容。固定路径，无需跨域推断。

### 场景 2：醉酒 → 白粥（plico-kg-entity-design.md §2.4）

用户：（无主动提示，AI 自主观察）

```
点餐记录 × N次
    → BehavioralObservation (醉后点餐内容)
    → Pattern Extraction: 同一 trigger_context 重复
    → UserFact: "醉酒 → 偏好白粥" (confidence = f(重复次数))
    → ProactiveAction: "闹钟前15分钟 → 点白粥到家"
```

零跳 → 主动行动。AI 从行为序列自主推断，不需要用户显式输入。

### 场景 3：和王总吃饭 → 提醒带酒（本章重点）

用户："明天和王总吃饭"

```
输入 → EventContainer(吃饭)
    → HAS_ATTENDEE → Person(王总)
    → Person(王总) → Preference(喜欢喝酒)
    → ContextMatch(吃饭场景) × Preference(喝酒)
    → Action: "提醒带酒"
```

**多跳**：事件 → 人员 → 偏好 → 行动建议。路径非固定，需要跨域关联。

---

## 2. 多跳推理的图结构基础

### 2.1 推理网络节点

```rust
// 推理锚点节点
struct EventContainer { id, label, event_type, attendees, ... }
struct Person { id, name, aliases, importance, ... }

// 偏好节点（Person 专属）
struct Preference {
    id: String,
    person_id: String,           // 关联的 Person
    predicate: String,            // "prefers" | "dislikes" | "allergic_to"
    object: String,               // "wine" | "white_congee" | "spicy_food"
    context: String,              // "at_dinner" | "at_home" | "when_drunk"
    confidence: f32,              // 基于观察次数
    evidence_cids: Vec<String>,   // 支持的行为记录 CID
}

// 行动建议节点
struct ActionSuggestion {
    id: String,
    trigger_event_id: String,    // 触发的事件
    target_person_id: String,     // 针对的人
    action: String,              // "带酒" | "点白粥"
    reasoning_chain: Vec<String>, // 推理路径文本（可解释性）
    confidence: f32,
    status: SuggestionStatus,    // Pending / Confirmed / Dismissed
}
```

### 2.2 推理边类型

```rust
enum ReasoningEdge {
    // Person → Preference（核心）
    HAS_PREFERENCE,              // Person → Preference
    PREFERENCE_FOR,             // Preference → Context

    // Preference → Action（核心）
    SUGGESTS_ACTION,            // Preference → ActionSuggestion
    MOTIVATED_BY,              // ActionSuggestion → EventContainer

    // 上下文约束
    APPLICABLE_IN_CONTEXT,      // Preference → Context
}
```

### 2.3 偏好获取路径（单跳 vs 多跳）

```
单跳（直接观察）：
    用户输入："王总喜欢喝酒"
        → Preference(person=王总, predicate=prefers, object=wine)
        → SUGGESTS_ACTION → ActionSuggestion(带酒)

多跳（跨事件推断）：
    历史记录：王总 + 多次晚餐 + 多次喝酒
        → HAS_ATTENDEE 边聚合 → Person(王总) 的事件序列
        → 事件序列 → 提取共同模式 → Preference(person=王总, object=wine)
        → SUGGESTS_ACTION → ActionSuggestion(带酒)

跨人推断（更复杂）：
    王总 + 红酒偏好 ← 历史记录
    李律师 + 威士忌偏好 ← 历史记录
        → 跨 Person 聚合偏好类型
        → SUGGESTS_ACTION(商务宴请 → 带红酒)
```

---

## 3. 参考 Research（2025-2026）

### 3.1 Multi-Hop KG Reasoning（MHGRN, EMNLP 2020）

**核心思想**：在 KG 子图上进行多跳关系推理，结合路径推理和 GNN。

**Plico 借鉴**：
- 推理时先提取子图（从 Event 或 Person 出发）
- 在子图上进行多跳遍历
- 路径可解释性：每条推理链都是一条 KG 路径

### 3.2 Chain-of-Knowledge（CoK, NeurIPS）

**核心思想**：将推理过程链接到知识源，防止幻觉。

**Plico 借鉴**：
- 每条 ActionSuggestion 必须包含 `reasoning_chain`（KG 路径文本）
- 用户可以看到推理依据
- 低置信度时显示推理过程，让用户判断

### 3.3 WideMem-AI（v1.4.0, 2026）

**核心思想**：重要性评分 + 时间衰减 + YMYL 优先级 + 不确定性感知。

**Plico 借鉴（已在 plico-kg-entity-design.md §2.4）**：
- Preference.confidence 基于重复次数（频率）
- 时间衰减：长时间无新证据 → confidence 下降
- Uncertainty mode：低置信度 → 显示"根据历史记录，王总偏好红酒" + 询问确认

### 3.4 Uncertainty Modes（WideMem v1.4）

| 模式 | 行为 | 适用场景 |
|------|------|---------|
| **Strict** | 置信度不足时拒绝回答 | 医疗、法律、金融 |
| **Helpful**（默认）| 承认不确定性，给出部分信息 | 日常助手 |
| **Creative** | 主动猜测并明确声明不确定性 | 探索性对话 |

**Plico 应用**：
```
Preference(confidence=0.3) + Strict → "我不确定王总的酒类偏好"
Preference(confidence=0.3) + Helpful → "根据有限信息，王总可能偏好..."
Preference(confidence=0.3) + Creative → "建议带红酒（高概率），也可准备威士忌"
```

---

## 4. 推理引擎架构

### 4.1 推理触发时机

```
┌─────────────────────────────────────────────────────┐
│ 1. Event 触发（最常见）                               │
│    用户输入 → EventContainer 创建/更新                  │
│    → 自动触发 Preference Inference                     │
│                                                      │
│ 2. Person 触发（历史记录积累）                         │
│    同一 Person 的事件数 ≥ N（如3次）                   │
│    → 批量 Preference Inference                        │
│                                                      │
│ 3. 用户显式查询触发                                   │
│    用户："王总有什么偏好？"                            │
│    → 实时 KG Query → Preference 节点                   │
│                                                      │
│ 4. Proactive（主动提议）                              │
│    Alarm Scheduler 触发                               │
│    → 检查相关 Person 的 Preference                     │
│    → 生成 ActionSuggestion                            │
└─────────────────────────────────────────────────────┘
```

### 4.2 Preference Inference Pipeline

```rust
/// 从行为记录推断 Preference
async fn infer_preference(
    person_id: &str,
    behavioral_observations: Vec<BehavioralObservation>,
) -> Vec<Preference> {
    // Step 1: 按 context（场景）分组
    // 晚餐事件 → context="at_dinner"
    // 醉酒事件 → context="when_drunk"

    // Step 2: 在每组内统计 object 频率
    // context=at_dinner: wine=5次, beer=2次, tea=1次
    // → Preference(person, prefers, wine, at_dinner, confidence=0.7)

    // Step 3: 置信度 = min(1.0, frequency / MIN_OBSERVATIONS)
    // MIN_OBSERVATIONS = 3（至少观察3次才建立偏好）

    // Step 4: 时间衰减
    // 最后一次观察到现在的天数 → decay(confidence)
}
```

### 4.3 Action Suggestion Generation

```rust
async fn generate_suggestions(
    event: &EventContainer,
    persons: &[Person],
) -> Vec<ActionSuggestion> {
    let mut suggestions = Vec::new();

    for person in persons {
        // 查询该 Person 的所有 Preference
        let prefs = kg.get_preferences(person.id);

        for pref in prefs {
            // 检查偏好是否适用于当前事件类型
            if pref.context.matches_event_type(event.event_type) {
                // 生成行动建议
                suggestions.push(ActionSuggestion {
                    id: uuid!(),
                    trigger_event_id: event.id.clone(),
                    target_person_id: person.id.clone(),
                    action: action_for_preference(&pref),
                    reasoning_chain: format!(
                        "因为 {} 在 {} 场合偏好 {}，\
                         所以 {} 时建议 {}",
                        person.label, pref.context, pref.object,
                        event.label, action_text(&pref)
                    ),
                    confidence: pref.confidence,
                    status: SuggestionStatus::Pending,
                });
            }
        }
    }

    suggestions
}

fn action_for_preference(pref: &Preference) -> String {
    match (pref.predicate.as_str(), pref.object.as_str()) {
        ("prefers", "wine") => "提醒带红酒".to_string(),
        ("prefers", "white_congee") => "准备白粥".to_string(),
        ("dislikes", food) => format!("避免准备{}", food),
        _ => format!("考虑{}", pref.object),
    }
}
```

---

## 5. 具体推理链分析

### 5.1 "和王总吃饭 → 提醒带酒" 完整推理链

```
用户输入："明天和王总吃饭"
    ↓
Step 1: Event Registration
    创建 EventContainer(
        label = "与王总晚餐",
        event_type = Meal,
        start_time = 明天 19:00,
        attendees = [Person(王总)],
    )
    ↓ KG upsert

Step 2: Preference Lookup（多跳起点）
    HAS_ATTENDEE(王总) → Person(王总)
        ↓ Query
    Preference(person=王总, predicate=prefers, object=wine)
        (从历史记录：王总出席的8次商务晚餐中，6次选择了红酒)

Step 3: Context Matching
    Preference.context = "at_business_dinner"
    EventContainer.event_type = Meal (subtype: BusinessDinner)
    → 匹配 ✓

Step 4: Action Suggestion Generation
    ActionSuggestion(
        action = "提醒带红酒",
        reasoning_chain = [
            "前提1：王总偏好红酒（6/8次商务晚餐选择红酒）",
            "前提2：明天是与王总的商务晚餐",
            "结论：建议携带红酒赴约",
        ],
        confidence = 0.75,
    )

Step 5: User Notification（通过 Scheduler）
    闹钟触发 → Agent 读取待确认建议
        → 推送："明天和王总晚餐，是否提醒带红酒？"
```

### 5.2 对比：醉酒白粥 vs 吃饭带酒

| 维度 | 醉酒白粥 | 吃饭带酒 |
|------|---------|---------|
| 触发方式 | AI 主动观察（无显式输入） | 用户显式输入事件 |
| Preference 来源 | BehavioralObservation 序列推断 | 历史事件中被观察 |
| 推断方向 | 行为 → 偏好 → 行动 | 事件 → 人员 → 偏好 → 行动 |
| 推理跳数 | 3跳（观察→推断→行动） | 4跳（事件→人员→偏好→行动） |
| 置信度来源 | 行为重复次数 | 事件中偏好出现的频率 |
| 行动类型 | Proactive（AI 发起） | Reactive（用户事件触发） |

### 5.3 更复杂的跨人推理链

> "王总、李律师明天都来开会，准备什么酒？"

```
Step 1: Event("三方商务会议") → attendees = [王总, 李律师]
    ↓
Step 2: Person(王总) → Preference(prefers=red_wine, context=business)
    Person(李律师) → Preference(prefers=whisky, context=business)
    ↓
Step 3: 冲突检测
    王总 + 李律师 偏好不同 → 触发冲突
    ↓
Step 4: 冲突解决
    Option A: 准备红酒+威士忌两种
    Option B: 折中选择香槟
    ↓
Step 5: ActionSuggestion × 2
    → "准备红酒（王总）" + "准备威士忌（李律师）"
    → "折中：准备香槟（需双方确认）"
```

---

## 6. 关键设计决策

### 6.1 Preference 内联 vs 独立 KG 节点

> ⚠️ **设计决策（2026-04-15 验证）**：
> Preference **不**作为独立 KG 节点存储。
> 理由：
> 1. DRN (Dynamic Reasoning Network) 证明逐步推理中，中间结果**内联传递**更简洁
> 2. Preference 通常是单次推理产物，不跨事件复用
> 3. 高频偏好（如王总→红酒）通过**多次 ActionSuggestion 累积置信度**来强化
> 4. 只有被多个 ActionSuggestion 引用的 Preference 才提取为 KG 节点

| 场景 | 存储位置 | 原因 |
|------|---------|------|
| 单次事件推断 | `ActionSuggestion.reasoning_chain` 内联 | 不需要持久化 |
| 跨事件高频出现 | UserFact KG 节点 | 多次印证，需要持久化 |
| Person 长期偏好 | Person 节点的 metadata 字段 | 直接查询，不需要图遍历 |

### 6.2 Preference vs Habit

```
Preference:  静态的长期偏好（王总喜欢红酒）
Habit:      动态的短期模式（用户每周三喜欢点外卖）
```

在 KG 中两者表示相同（KG 节点 + valid_at），但 importance 不同：
- Preference importance = 固定高（如 7.0），不随时间衰减
- Habit importance = 初始高，随无新观察逐渐衰减

### 6.3 置信度阈值设计

```rust
const PREFERENCE_MIN_CONFIDENCE: f32 = 0.4; // 最低置信度阈值
const PREFERENCE_HIGH_CONFIDENCE: f32 = 0.8; // 高置信度（自动行动）

impl Preference {
    fn is_actionable(&self) -> bool {
        self.confidence >= PREFERENCE_HIGH_CONFIDENCE
    }

    fn needs_confirmation(&self) -> bool {
        self.confidence >= PREFERENCE_MIN_CONFIDENCE
            && self.confidence < PREFERENCE_HIGH_CONFIDENCE
    }
}
```

---

## 7. 实现路线图

```
Phase M13: Preference 节点 + 边类型（依附 Phase 5 M14）
Phase M14: Preference Inference Pipeline
  - 从 EventContainer attendee 历史推断 Preference
  - 置信度 = frequency / total observations
  - 时间衰减函数

Phase M15: ActionSuggestion + 通知集成
  - 生成 → Scheduler 触发 → 用户确认
  - reasoning_chain 序列化

Phase M16: 跨人冲突检测 + 折中建议
  - 多 Person 不同 Preference → 冲突图节点
  - 折中方案生成
```

---

## 8. 与现有系统的关系

```
BehavioralObservation ──→ 推断 ──→ Preference
        ↑                                     ↓
   EventContainer                    ActionSuggestion
        ↑                                     ↓
  (现有 KG Event 节点)          (Scheduler 触发通知)

 Preference Inference Pipeline
        ↓
 KG 节点写入（Preference 节点）
        ↓
  Query API: KGQuery(preferences_for=person_id)
        ↓
  Action Suggestion Generation
```

所有新组件都基于现有 KG 接口扩展，不破坏现有架构。
