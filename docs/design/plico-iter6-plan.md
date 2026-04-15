# Plico Iteration 6 开发计划

**Date**: 2026-04-15
**Status**: Phase A ✅ complete — Audit loop done, all 8 issues verified resolved
**Next**: Phase C (BehavioralObservation Pipeline) / Phase D foundations (HAS_PREFERENCE edge types + ActionSuggestion struct)

---

## 0. 项目灵魂（system.md 核心萃取）

> system.md 的第一性原理：
> - **Agent 是第一公民**，进程/文件不是
> - **内容即地址**（CAS），语义即索引，向量+知识图谱
> - **意图驱动**而非路径驱动：用户说"上周的会议"，系统解析意图→找到内容
> - **分层上下文**：L0(~100tok) / L1(~2k) / L2(full)，按需加载
> - **一切皆工具**：文件、网络、数据库都是 API，不是 CLI
> - **概率+确定混合**：LLM 推理是概率的，但存储和调度是确定的

**关键约束**：EventContainer 不是"功能增强"，而是**存储组织核心原则的根本转变**——从「文件为主」到「事件为主」。这是 plico-kg-entity-design.md §0.5 提出的核心洞察，必须以此为锚。

---

## 1. 网络研究验证：设计决策去幻觉

### 1.1 Event vs Document 作为 KG 组织根节点

**Research**: Google Personal Knowledge Graphs (2019) 指出 PKG 与普通 KG 的核心区别：
- PKG 包含**用户个人感兴趣的实体**（private entities）
- PKG 的实体具有**所有权**（ownership）
- PKG 的关系是**时序化**的（事件驱动）

**验证结论**: ✅ EventContainer 作为独立 KG 节点类型是合理的，**不是过度设计**
- 理由：Event 是时间锚点 + 内容聚合根，符合 PKG 的事件化特征
- 避免幻觉：不需要在 KGNodeType 中新增 Event，用 metadata + 专用边即可实现

**关键修正**：设计文档中计划新增 `KGNodeType::Event`，这是**不必要的复杂度**。
现有 KGNodeType 已经够用，EventContainer 应该作为**带特殊 metadata 字段的 KGNode** 存储，
通过 `KGEdgeType::HasAttendee / HasDocument / HasDecision` 建立关系。

### 1.2 Preference 作为独立节点 vs 属性字段

**Research**: MHGRN (EMNLP 2020) + DRN (Dynamic Reasoning Network) 的多跳推理框架表明：
- 中间推理节点（hop results）可以**携带推理置信度**
- 推理路径的每一步都可以被解释
- 不需要为每个中间概念创建持久节点

**验证结论**: ⚠️ Preference 作为独立 KG 节点是**可选的**，取决于推理频率
- 如果 Preference 推断只服务单次 ActionSuggestion，存为 ActionSuggestion.reasoning_chain 即可
- 如果 Preference 需要跨事件复用（如王总在任何商务场合都偏好红酒），则需独立节点
- **Plico 应该采用混合策略**：高频偏好持久化为 KG 节点；低频推断内联在 ActionSuggestion

### 1.3 BehavioralObservation 数据来源

**Research**: WideMem-AI v1.4.0 的 memory 机制：
- 记忆分为 **Facts** / **Summaries** / **Themes** 三层
- Facts = 原始观察记录（verbatim），Themes = 高层推断

**验证结论**: ⚠️ BehavioralObservation 的前提是**系统能观察到行为**
- 当前 Plico 的数据来源：用户显式创建的内容（CAS 对象）
- 点餐记录等行为数据**不在当前系统范围内**
- BehavioralObservation pipeline 应该在 **Phase C 作为独立模块** 实现，不依赖 EventContainer

**设计修正**：Phase C 需要先定义"数据注入接口"，而不是假设数据已存在

### 1.4 时间衰减函数的合理性

**Research**: WideMem v1.4 的 4 种时间衰减函数（exponential/linear/step/none）在实践中：
- exponential 适用于"日常行为模式"（如每周三点外卖）
- linear 适用于"缓慢变化的知识"（如偏好）
- step 适用于"突然变化"（如搬家后地点偏好）

**验证结论**: ✅ plico-multi-hop-reasoning.md 提到 exponential decay 是合理的
- 建议显式说明：Preference 使用 linear decay，Habit 使用 exponential decay

---

## 2. 修正后的 Phase 依赖图

```
Phase A: EventContainer as Content Aggregation Root
  └─ 核心变化：从文件为主 → 事件为主（system.md §0.5 核心洞察）
  ├─ A1: KGEdgeType 新增事件关系边（不改 KGNodeType）
  ├─ A2: SemanticFS::create_event / list_events（不新建节点类型）
  ├─ A3: AIKernel API 包装
  ├─ A4: TemporalResolver → 事件时间过滤
  └─ A5: CLI 支持

Phase B: BehavioralObservation Pipeline（与 A 并行，可独立）
  ├─ B1: BehavioralObservation 数据结构
  ├─ B2: 数据注入接口（外部系统接入点）
  ├─ B3: PatternExtraction（非 KG 节点，内联推断）
  └─ B4: UserFact 节点（持久化推断结果）

Phase C: Preference-Informed ActionSuggestion
  ├─ C1: Preference 作为 ActionSuggestion 内联字段（不独立节点）
  ├─ C2: 多跳推理：Event→Person→Preference→Action
  ├─ C3: reasoning_chain 序列化
  └─ C4: Uncertainty mode：Strict/Helpful/Creative

Phase D: Proactive Scheduler Integration（最后）
  └─ D1: ActionSuggestion → Scheduler 触发通知
```

---

## 3. Phase A 详细设计（修正版）

### A1: KGEdgeType 新增事件关系边

```rust
// src/fs/graph.rs — 新增边类型，不改 KGNodeType

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KGEdgeType {
    // 现有
    AssociatesWith,
    Mentions,
    Follows,
    SimilarTo,
    // 新增：事件专用关系（标注 from_entity / to_entity 类型）
    HasAttendee,       // Event → Person (attendee_ids 字段编码)
    HasDocument,        // Event → Document (related_cids 字段编码)
    HasMedia,          // Event → Media
    HasDecision,       // Event → ActionItem
    RelatedTo,         // Event ↔ Event
}
```

**设计决策**：EventContainer **不**作为新的 `KGNodeType`，而是：
- 存储为带 `event_type` + `start_time` metadata 的现有 KGNode
- 事件专用边（HasAttendee 等）提供类型安全的关系查询
- 这样无需修改 KG 的持久化格式，兼容现有 PetgraphBackend

### A2: EventContainer 数据结构

```rust
// src/fs/semantic_fs.rs — 新增，不修改现有 KGNode

/// Event metadata stored in KG node's metadata JSON field.
/// This avoids adding a new KGNodeType — we reuse existing Document/Entity nodes
/// with special metadata to represent events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventMeta {
    pub label: String,
    pub event_type: EventType,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub location: Option<String>,
    pub attendee_ids: Vec<String>,    // Person KG node IDs
    pub related_cids: Vec<String>,     // All related CAS object CIDs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    Meeting,
    Presentation,
    Travel,
    Entertainment,
    Social,
    Work,
    Personal,
    Other,
}

impl EventMeta {
    /// Check if this event matches a time range.
    pub fn in_range(&self, since: Option<u64>, until: Option<u64>) -> bool {
        let start = self.start_time.unwrap_or(0);
        if let Some(s) = since {
            if start < s { return false; }
        }
        if let Some(u) = until {
            if start > u { return false; }
        }
        true
    }
}
```

### A3: SemanticFS 事件操作

```rust
impl SemanticFS {
    /// Create an event container — stores as KG node with EventMeta.
    /// Returns the KG node ID (which may equal the CID or be separate).
    pub fn create_event(
        &self,
        label: &str,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<&str>,
        tags: Vec<String>,
        agent_id: &str,
    ) -> Result<String, FSError> {
        let meta = EventMeta {
            label: label.to_string(),
            event_type,
            start_time,
            end_time,
            location: location.map(String::from),
            attendee_ids: Vec::new(),
            related_cids: Vec::new(),
        };
        let node_id = format!("evt:{}", uuid::Uuid::new_v4());
        let node = KGNode {
            id: node_id.clone(),
            label: label.to_string(),
            node_type: KGNodeType::Entity, // Reuse Entity, metadata carries event info
            agent_id: agent_id.to_string(),
            metadata: serde_json::to_string(&meta).map_err(|e| FSError::Io(...))?,
            created_at: chrono::Utc::now().timestamp_millis() as u64,
        };
        self.kg.add_node(node)?;
        // Tag index for event queries
        let mut tag_index = self.tag_index.write().unwrap();
        for tag in &tags {
            tag_index.entry(tag.clone()).or_default().insert(node_id.clone());
        }
        Ok(node_id)
    }

    /// List events matching time range and optional filters.
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> Result<Vec<EventSummary>, FSError> {
        // Step 1: Candidate events from tag index (if tags provided)
        let candidate_ids: HashSet<String> = if tags.is_empty() {
            self.kg.all_node_ids()  // Full scan if no tags
        } else {
            let tag_index = self.tag_index.read().unwrap();
            tags.iter()
                .filter_map(|t| tag_index.get(t))
                .flatten()
                .cloned()
                .collect()
        };

        // Step 2: Filter by time range + event type using EventMeta
        let mut results = Vec::new();
        for node_id in candidate_ids {
            if let Some(node) = self.kg.get_node(&node_id) {
                if let Ok(event_meta) = serde_json::from_str::<EventMeta>(&node.metadata) {
                    if !event_meta.in_range(since, until) { continue; }
                    if let Some(et) = event_type {
                        if event_meta.event_type != et { continue; }
                    }
                    results.push(EventSummary {
                        id: node.id,
                        label: event_meta.label,
                        event_type: event_meta.event_type,
                        start_time: event_meta.start_time,
                        attendee_count: event_meta.attendee_ids.len(),
                        related_count: event_meta.related_cids.len(),
                    });
                }
            }
        }
        results.sort_by_key(|e| e.start_time);
        Ok(results)
    }

    /// Attach a CAS object or Person to an event.
    pub fn event_attach(
        &self,
        event_id: &str,
        target_id: &str,
        relation: EventRelation,
        agent_id: &str,
    ) -> Result<(), FSError> {
        let edge_type = match relation {
            EventRelation::Attendee => KGEdgeType::HasAttendee,
            EventRelation::Document => KGEdgeType::HasDocument,
            EventRelation::Media => KGEdgeType::HasMedia,
            EventRelation::Decision => KGEdgeType::HasDecision,
        };
        self.kg.add_edge(event_id, target_id, edge_type, 1.0)?;
        // Update EventMeta.attendee_ids / related_cids for consistency
        self.update_event_meta_field(event_id, relation, target_id)?;
        Ok(())
    }

    pub fn event_add_attendee(&self, event_id: &str, person_id: &str) -> Result<(), FSError> {
        self.event_attach(event_id, person_id, EventRelation::Attendee, "")
    }
}
```

### A4: TemporalResolver → 事件查询集成

```rust
// 在 list_events 中复用已有 TemporalResolver

impl SemanticFS {
    /// Resolve time expression and find matching events.
    /// Example: list_events_by_time("上周", tags=["商务"]) → Vec<EventSummary>
    pub fn list_events_by_time(
        &self,
        time_expression: &str,
        tags: &[String],
        resolver: &dyn TemporalResolver,
    ) -> Result<Vec<EventSummary>, FSError> {
        let range = resolver.resolve(time_expression, None)
            .ok_or_else(|| FSError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Cannot resolve time expression",
            )))?;
        self.list_events(Some(range.since as u64), Some(range.until as u64), tags, None)
    }
}
```

---

## 4. Phase B 详细设计（BehavioralObservation Pipeline）

### B1: 关键设计决策修正

**来源验证**：BehavioralObservation 的数据必须来自某个**观察者**。
在 Plico 架构中，唯一的观察者是 **AI Agent**。
因此 BehavioralObservation 的生成方式是：

```
AI Agent 观察到用户行为 → 通过 SemanticFS.create() 存储为带标签的 CAS 对象
                            tags: ["behavior", "drunk", "food_order"]
                            content: "用户在醉酒状态下点了白粥和清淡小菜"
                            ↓
AI Agent 或 Scheduler 定期扫描 behavior 标签的对象
                            ↓
PatternExtraction 分析行为序列
                            ↓
生成 UserFact 或直接触发 ProactiveAction
```

**修正**：BehavioralObservation **不是新数据类型**，而是 **SemanticFS 的带特殊标签的 CAS 对象集合**。
PatternExtraction 是**离线分析 pipeline**（由 Scheduler 或外部 Agent 触发）。

### B2: PatternExtraction Pipeline

```rust
// 新文件: src/fs/pattern.rs

use crate::fs::semantic_fs::SemanticFS;

/// A behavior pattern inferred from multiple observations.
#[derive(Debug, Clone)]
pub struct BehaviorPattern {
    pub subject_id: String,
    pub trigger_context: String,  // "drunk" | "working_late" | "stressed"
    pub predicate: String,       // "prefers" | "dislikes" | "needs"
    pub object: String,           // "white_congee" | "red_wine"
    pub confidence: f32,          // Based on observation count + recency
    pub evidence_cids: Vec<String>,
    pub last_observed: u64,
}

impl SemanticFS {
    /// Extract behavior patterns from a series of behavioral observations.
    /// Called by Scheduler or external Agent periodically.
    ///
    /// Algorithm (WideMem-inspired):
    /// 1. Collect all objects tagged "behavior" created by or about the subject
    /// 2. Group by trigger_context
    /// 3. Within each group, count object frequency
    /// 4. confidence = min(1.0, count / 3) × recency_decay(last_observed)
    /// 5. If confidence > threshold, emit BehaviorPattern
    pub fn extract_patterns(
        &self,
        subject_id: &str,
    ) -> Vec<BehaviorPattern> {
        // Scan CAS for objects with tag "behavior" and matching subject metadata
        // ...
    }
}
```

---

## 5. Phase C 详细设计（Preference-Informed Action）

### C1: 核心决策——Preference 内联 vs 独立节点

**研究支撑**：DRN (Dynamic Reasoning Network) 采用逐步推理，每跳结果**内联传递**，不持久化中间节点。
只有最终答案和推理路径需要存储。

**Plico 采用**：Preference 作为 **ActionSuggestion 的内联字段**，不独立存储为 KG 节点。
理由：
1. Preference 通常是**单次推理的产物**，不跨事件复用
2. 王总"偏好红酒"这类高频知识通过**多次 ActionSuggestion 推断**来强化
3. 如果同一 Preference 被多个 ActionSuggestion 引用，才提取为 KG 节点

### C2: ActionSuggestion 结构

```rust
// src/fs/reasoning.rs — 新文件

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSuggestion {
    pub id: String,
    /// The event that triggered this suggestion.
    pub trigger_event_id: String,
    /// Person(s) this suggestion targets.
    pub target_person_ids: Vec<String>,
    /// The suggested action text.
    pub action: String,
    /// Step-by-step reasoning chain (for explainability, Chain-of-Knowledge).
    pub reasoning_chain: Vec<ReasoningStep>,
    /// Aggregated confidence [0, 1].
    pub confidence: f32,
    /// Uncertainty mode applied when generating this suggestion.
    pub uncertainty_mode: UncertaintyMode,
    /// Status: pending user confirmation, confirmed, or dismissed.
    pub status: SuggestionStatus,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningStep {
    pub hop: usize,
    pub from: String,     // "王总偏好红酒"
    pub via: String,      // "HAS_PREFERENCE"
    pub to: String,       // "商务晚餐场景"
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UncertaintyMode {
    Strict,   // If confidence < threshold, do not suggest
    Helpful,  // Acknowledge uncertainty, show reasoning chain
    Creative, // Guess and label as "speculative"
}

impl ActionSuggestion {
    /// Should this suggestion be proactively shown to the user?
    pub fn is_actionable(&self) -> bool {
        match self.uncertainty_mode {
            UncertaintyMode::Strict => self.confidence >= 0.8,
            UncertaintyMode::Helpful => self.confidence >= 0.4,
            UncertaintyMode::Creative => true,
        }
    }

    /// Generate a human-readable explanation of the reasoning chain.
    pub fn explain(&self) -> String {
        let mut lines = vec![format!("置信度: {:.0}%", self.confidence * 100.0)];
        for step in &self.reasoning_chain {
            lines.push(format!("  {}. [{}] {} → {} (置信度 {:.0}%)",
                step.hop, step.via, step.from, step.to, step.confidence * 100.0));
        }
        lines.push(format!("结论: {}", self.action));
        lines.join("\n")
    }
}
```

### C3: 多跳推理引擎

```rust
impl SemanticFS {
    /// Multi-hop reasoning: Event → Person → (implicit preference) → Action
    ///
    /// This is NOT a full KG reasoning engine (no MHGRN-style GNN).
    /// It uses pre-established edges and explicit inference rules.
    /// For novel inference paths, falls back to LLM.
    pub fn reason_about_event(
        &self,
        event_id: &str,
        uncertainty_mode: UncertaintyMode,
    ) -> Result<Vec<ActionSuggestion>, FSError> {
        let mut suggestions = Vec::new();

        // Step 1: Get event details
        let event_node = self.kg.get_node(event_id)
            .ok_or(FSError::NotFound)?;

        let event_meta: EventMeta = serde_json::from_str(&event_node.metadata)
            .map_err(|_| FSError::Io(...))?;

        // Step 2: Get attendees via HasAttendee edges
        let attendees = self.kg.get_neighbors(event_id, KGEdgeType::HasAttendee);

        for person_node in attendees {
            let person_label = &person_node.label;

            // Step 3: Infer preference from event context + person history
            // This uses the BehavioralObservation pattern (Phase B results).
            // Fallback: check KG for explicit preference facts.
            let inferred_preference = self.infer_preference_for_context(
                &person_node.id,
                &event_meta,
            );

            if let Some(pref) = inferred_preference {
                let step1 = ReasoningStep {
                    hop: 1,
                    from: format!("事件: {}", event_meta.label),
                    via: "HasAttendee".to_string(),
                    to: format!("人员: {}", person_label),
                    confidence: 1.0,
                };
                let step2 = ReasoningStep {
                    hop: 2,
                    from: format!("{} 在 {} 场合", person_label, pref.context),
                    via: "inferred_from_history".to_string(),
                    to: format!("偏好: {}", pref.object),
                    confidence: pref.confidence,
                };
                let step3 = ReasoningStep {
                    hop: 3,
                    from: format!("事件类型: {:?}", event_meta.event_type),
                    via: "context_match".to_string(),
                    to: format!("行动: {}", action_for_preference(&pref)),
                    confidence: pref.confidence,
                };

                let suggestion = ActionSuggestion {
                    id: uuid::Uuid::new_v4().to_string(),
                    trigger_event_id: event_id.to_string(),
                    target_person_ids: vec![person_node.id.clone()],
                    action: action_for_preference(&pref),
                    reasoning_chain: vec![step1, step2, step3],
                    confidence: pref.confidence,
                    uncertainty_mode,
                    status: SuggestionStatus::Pending,
                    created_at: chrono::Utc::now().timestamp_millis() as u64,
                };

                if suggestion.is_actionable() {
                    suggestions.push(suggestion);
                }
            }
        }

        Ok(suggestions)
    }

    fn infer_preference_for_context(
        &self,
        person_id: &str,
        event_meta: &EventMeta,
    ) -> Option<InferredPreference> {
        // Look for UserFact nodes about this person with matching context
        // Or use BehaviorPattern results from Phase B
        // Fallback: return None (let LLM handle novel inference)
        None
    }
}
```

---

## 6. Phase D: Proactive Scheduler Integration

```rust
// Scheduler triggers ActionSuggestion evaluation

impl AgentScheduler {
    /// Check all pending events and generate ActionSuggestions.
    /// Triggered by alarm or periodic scan.
    pub fn evaluate_upcoming_events(&self, kernel: &AIKernel) {
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let lookahead = 24 * 3600 * 1000; // 24 hours

        // Find events in the next 24 hours
        let events = kernel.list_events(Some(now), Some(now + lookahead), &[], None)
            .unwrap_or_default();

        for event in events {
            let suggestions = kernel.reason_about_event(&event.id, UncertaintyMode::Helpful)
                .unwrap_or_default();

            for suggestion in suggestions {
                // Store as pending suggestion (for user confirmation)
                kernel.store_action_suggestion(suggestion);
            }
        }
    }
}
```

---

## 7. 实施路线图（每步独立测试）

```
Iter 6 Step 1: KGEdgeType 事件关系边 + EventMeta 结构
  → cargo test, 验证 KG CRUD 正常

Iter 6 Step 2: SemanticFS::create_event / list_events / event_attach
  → cargo test, 集成测试: "创建事件 → 添加参会人 → 按时间查询"

Iter 6 Step 3: TemporalResolver → list_events_by_time
  → 测试: list_events_by_time("上周", resolver) 返回正确结果

Iter 6 Step 4: AIKernel API 包装 + CLI (event create / list)
  → cargo build --bin aicli, CLI 测试

Iter 6 Step 5: PatternExtraction pipeline (src/fs/pattern.rs)
  → 单元测试: 从行为记录推断模式

Iter 6 Step 6: ActionSuggestion + reasoning_chain
  → 单元测试: 多跳推理链生成 + explain()

Iter 6 Step 7: Scheduler 集成（可选，看时间）
```

---

## 8. 关键设计决策汇总（经网络研究验证）

| 决策 | 结论 | 依据 |
|------|------|------|
| EventContainer 不新增 KGNodeType | ✅ 用 EventMeta + 现有 Entity 节点 | 避免持久化格式变化，PKG 研究支持 |
| Preference 不独立存储为 KG 节点 | ✅ 作为 ActionSuggestion 内联字段 | DRN 内联推理原则，简化架构 |
| BehavioralObservation = 带特殊标签的 CAS 对象集合 | ✅ 不新增数据类型 | 与 Plico CAS-first 架构一致 |
| PatternExtraction = 离线分析 pipeline | ✅ 由 Scheduler/Agent 触发扫描 | 解耦观察与推断 |
| Uncertainty mode 在 ActionSuggestion 层实现 | ✅ Strict/Helpful/Creative 三档 | WideMem v1.4 验证 |
| reasoning_chain = Vec<ReasoningStep> | ✅ 每步内联，支持 CoK 可解释性 | Chain-of-Knowledge 原则 |
