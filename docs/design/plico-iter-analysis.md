# Plico 迭代方向分析

**Date**: 2026-04-16
**Version**: 0.1
**Status**: Analysis — for next iteration planning

---

## 1. 项目灵魂 (Soul) 回顾

### 1.1 四大核心原则

```
1. 内容即地址     — CAS + SHA-256，内容哈希即身份
2. 语义即索引     — 向量嵌入 + 知识图谱，而非路径/文件名
3. 事件为第一公民  — 人类记忆基于事件，AI存储也应如此
4. AI自我迭代     — BehavioralObservation → UserFact → ActionSuggestion
```

### 1.2 架构层次

```
应用层 (AI Agent Ecosystem)
        ↓
AI友好接口层 (Semantic API/CLI)
        ↓
AI内核层 (Agent Scheduler + Layered Memory + Permission Guardrails)
        ↓
AI原生文件系统 (CAS + Semantic Index + KG + Context Loading)
```

---

## 2. 当前进度 vs 项目灵魂 检验

| 灵魂需求 | 实现状态 | 差距 |
|---------|---------|------|
| 无人类CLI/GUI | 只有TCP/CLI供AI调用 | ✅ 满足 |
| CAS内容寻址 | SHA-256 CID | ✅ 满足 |
| 语义索引 | embedding + petgraph KG | ✅ 满足 |
| 事件为第一公民 | EventContainer + KG边 | ✅ 满足 |
| AI自我迭代 | Phase D 全套 | ✅ 满足 |
| **Agent执行层** | LocalExecutor stub | ❌ **关键缺口** |
| **外部工具执行** | 无 | ❌ **关键缺口** |
| 分层记忆 | Ephemeral/Working/LongTerm/Procedural | ✅ 满足 |

---

## 3. 三个例子 需求映射

### Example 1: 总结会议生成PPT

```
用户需求: "帮我总结前几天会议内容，生成PPT"

需求拆解:
┌─────────────────────────────────────────────────────────────┐
│ 1. 找到会议文件    → semantic_search (✅ 已实现)              │
│ 2. 理解会议内容    → LLM 总结能力 (⚠️ 需要 Ollama 配置)      │
│ 3. 提取决议/任务   → EntityExtractor (❌ 未实现自动提取)      │
│ 4. 生成PPT         → 外部工具/技能 (❌ 无执行机制)            │
│ 5. 调度执行        → Agent Scheduler (⚠️ LocalExecutor stub)│
└─────────────────────────────────────────────────────────────┘
```

### Example 2: 宿醉后主动点白粥

```
用户需求: "某天我宿醉后 AI 主动帮我点白粥"

需求拆解:
┌─────────────────────────────────────────────────────────────┐
│ 推理链 (✅ 已实现):                                           │
│   BehavioralObservation → PatternExtractor → UserFact        │
│   → infer_suggestions_for_event → ActionSuggestion           │
│                                                             │
│ 执行链 (❌ 未实现):                                           │
│   ActionSuggestion → Scheduler 触发 → 用户确认 → 点餐API执行   │
└─────────────────────────────────────────────────────────────┘
```

### Example 3: 王总吃饭提醒带酒

```
用户需求: "和王总吃饭 AI 提醒我带酒"

需求拆解:
┌─────────────────────────────────────────────────────────────┐
│ 推理链 (✅ 已实现):                                           │
│   Event(吃饭) → HasAttendee(王总) → UserFact → ActionSuggestion │
│                                                             │
│ 推送链 (❌ 未实现):                                           │
│   Scheduler 触发提醒 → 推送通知 → 用户确认/Dismiss           │
└─────────────────────────────────────────────────────────────┘
```

---

## 4. 链式推理：下一轮迭代方向

### 4.1 基于灵魂的优先级推导

```
项目灵魂第4条: "AI自我迭代"
    ↓
Phase D 已完成推理链 (BehavioralObservation → ActionSuggestion)
    ↓
推理链完成后，项目灵魂要求"AI自我迭代"必须落地执行
    ↓
**Agent 执行层完善** 是下一轮最高优先级
```

### 4.2 迭代方向链式图

```
Phase D 推理链完成 (当前状态)
        │
        ▼
缺失1: ActionSuggestion → Scheduler 触发机制
        │
        ▼
缺失2: 用户确认/Dismiss 状态跟踪
        │
        ▼
缺失3: 外部工具注册/调用 (点餐API、提醒推送)
        │
        ▼
缺失4: LocalExecutor stub → 真实执行调度
        │
        ▼
结论: 下一轮迭代 = "Agent 执行层 + 外部工具集成"
```

---

## 5. 具体迭代计划

### Phase E: Agent 执行层完善

#### E1: LocalExecutor → 真实调度实现

```rust
// src/scheduler/dispatch.rs

// 当前: stub LocalExecutor
struct LocalExecutor;

// 目标: 真实调度
impl AgentExecutor for LocalExecutor {
    async fn execute(&self, intent: Intent) -> Result<IntentResult, SchedulerError> {
        // 1. 解析 Intent.action
        // 2. 查找注册的 Tool
        // 3. 调用 Tool
        // 4. 返回结果
    }
}
```

#### E2: Tool 注册机制

```rust
// 新文件: src/scheduler/tool.rs

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, args: serde_json::Value) -> impl Future<Output = Result<serde_json::Value, ToolError>>;
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Arc<dyn Tool>) { ... }
    pub fn call(&self, name: &str, args: serde_json::Value) -> ... { ... }
}
```

#### E3: ActionSuggestion → Scheduler 触发

```rust
// 扩展 AIKernel
impl AIKernel {
    /// 当 ActionSuggestion 被用户确认时，调度执行
    pub fn confirm_suggestion(&self, suggestion_id: &str) -> Result<(), PlicoError> {
        let suggestion = self.fs.get_suggestion(suggestion_id)?;
        let intent = Intent {
            action: suggestion.action.clone(),
            args: suggestion.args.clone(),
            priority: IntentPriority::High, // 确认的建议是高优先级
        };
        self.scheduler.submit(intent)?;
        Ok(())
    }
}
```

#### E4: 外部工具示例 (点餐API)

```rust
// 示例: 点餐工具
struct FoodOrderingTool {
    api_key: String,
}

impl Tool for FoodOrderingTool {
    fn name(&self) -> &str { "order_food" }

    async fn execute(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        let food = args["food"].as_str().unwrap();
        // 调用外卖API...
        Ok(json!({ "order_id": "xxx", "status": "placed" }))
    }
}
```

---

## 6. 与 Example 1 相关的补充迭代

### Phase F: EntityExtractor 自动提取

```
Example 1 需求:
  "帮我总结前几天会议内容，生成PPT"

当前缺口:
  - 会议文件存入 CAS 时，不自动提取 KG 实体
  - 无法自动识别参会人、决议、截止日期

解决方案:
  create() 时自动触发 EntityExtractor
  → 自动创建 KG 节点 (Person, ActionItem)
  → 自动建立边 (HasAttendee, HasDecision)
```

```rust
// semantic_fs.rs create() 扩展
pub fn create(&self, content: Vec<u8>, tags: Vec<String>, ...) -> std::io::Result<String> {
    let cid = self.cas.put(&obj)?;

    // 现有: upsert_semantic_index + upsert_document_to_kg
    // 新增: async EntityExtractor 触发 (非阻塞)
    if let Some(ref kg) = self.knowledge_graph {
        let cid_clone = cid.clone();
        let tags_clone = tags.clone();
        let agent_id_clone = agent_id.clone();
        // spawn async extraction (non-blocking)
        tokio::spawn(async move {
            if let Err(e) = kg.extract_and_upsert_entities(&cid_clone, &tags_clone, &agent_id_clone) {
                tracing::warn!("Entity extraction failed for {}: {}", &cid_clone[..8], e);
            }
        });
    }

    Ok(cid)
}
```

---

## 7. 迭代优先级总结

| 优先级 | 迭代内容 | 服务于 |
|--------|---------|--------|
| **P0** | Agent 执行层完善 (LocalExecutor + Tool Registry) | Example 2/3 执行 |
| **P0** | ActionSuggestion → Scheduler 触发 + 用户确认 | Example 2/3 推送 |
| **P1** | EntityExtractor 自动提取 | Example 1 素材理解 |
| **P1** | 外部工具: 提醒推送 API | Example 3 提醒 |
| **P2** | 外部工具: 点餐 API | Example 2 点粥 |
| **P2** | Vision LLM (照片理解) | Example 1 照片素材 |

---

## 8. 迭代前后对比

### 迭代前 (当前状态)

```
用户: "和王总吃饭提醒我带酒"
AI:   [生成 ActionSuggestion: "提醒带酒" (在内存中)]
用户: ??? (无法推送提醒)
```

### 迭代后 (目标状态)

```
用户: "和王总吃饭提醒我带酒"
AI:   [生成 ActionSuggestion: "提醒带酒"]
      [Scheduler 在适当时间触发]
      [推送给用户: "明天和王总晚餐，是否提醒带红酒？"]
用户: "是"
AI:   [confirm_suggestion → Scheduler 执行 → 调用通知工具]
```

---

*本文档与 `plico_project_soul.md` 配套使用 — 每次迭代前对照检验。*