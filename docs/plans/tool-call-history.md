# Tool Call History — 思想文档

**状态**：设计完成，待实现
**优先级**：高（v52）
**日期**：2026-05-17

## 决策摘要

| 决策 | 结论 |
|------|------|
| 核心目标 | 调试 + 跨会话学习 + 技能提炼（三层 pipeline） |
| 粒度 | 意图作为 trace root，API 调用作为 children span |
| 存储 | 专用 JSONL trace 文件（不引入第四种存储） |
| 写入 | mpsc channel + 单线程 writer worker（非阻塞） |
| 保留期 | 7 天自动清理，可配置 |
| Session 关系 | 松散关联（trace 有 session_id 字段） |
| Phase 1 范围 | 完整基础设施（存储 + CLI + API） |
| Trace→Knowledge | Phase 2 再做 |

---

## 1. 问题陈述

Plico 的 Agent 通过 API（`semantic_create`, `semantic_search`, `recall` 等）与 kernel 交互，但这些调用没有被结构化记录。当前状态：

- **EventBus** 记录 Write/Update/Delete 事件，但没有层级关系（不知道哪个 search 属于哪个意图）
- **Memory** 存储知识，但不存储 "我做过什么"
- **KG** 存储实体关系，但不存储执行轨迹

缺失的能力：
1. **调试**：Agent 执行失败时，无法回溯 "它尝试了什么、为什么失败"
2. **学习**：Agent 无法从历史成功/失败中改进策略
3. **技能**：高频使用的 tool call 序列无法被提炼为可复用 procedure

## 2. 业界调研

### 2.1 Trace-based 可观测性（Langfuse / LangSmith / OpenTelemetry）

**核心模型**：Trace → Span 树
```
Trace (一个用户请求)
├── Span: LLM call (意图分类)
├── Span: Tool call (semantic_search)
│   ├── input: {query: "会议记录", tags: ["meeting"]}
│   ├── output: {results: [...], latency_ms: 45}
│   └── status: success
├── Span: Tool call (semantic_create)
│   ├── input: {content: "...", tags: [...]}
│   └── status: success
└── Span: LLM call (生成回复)
```

**Plico 可借鉴**：层级结构、input/output/status 记录、延迟追踪

### 2.2 Episodic Memory（Letta / TencentDB Agent Memory）

**核心模型**：从原始交互中提取结构化知识
```
原始对话 → 提取事实 → 用户画像 → 长期记忆
"用户问了X" → "用户对Y感兴趣" → "用户偏好: 技术文档" → 存入 Long-term
```

**TencentDB 4 层架构**（2026-04）：
1. 原始对话记录
2. 结构化事实提取
3. 场景化任务信息
4. 用户画像

**Plico 可借鉴**：pipeline 式渐进提炼，从 trace → knowledge → skill

### 2.3 Skills Evolution（EverOS, ACL 2026）

**核心模型**：从 tool call 序列中自动提炼技能
```
观察: Agent 搜索 "会议记录" 时，先 search(tags=["meeting"]) 再 search(query="会议")
模式: tag search + keyword search 组合效果好
提炼: 创建 skill "search_meeting" = [search_by_tags("meeting"), search_by_keyword(query)]
```

**Plico 可借鉴**：与现有 Cognitive Pipeline（`CognitiveTask::ProcessDocument`）集成

### 2.4 Cursor Agent Trace（2026-02）

**核心模型**：记录 AI vs 人类代码贡献的归属格式
- 每个代码变更标注来源（AI/Human）
- 版本控制中保留归属信息

**Plico 可借鉴**：tool call 结果标注来源（哪个 agent、哪个意图）

## 3. 设计方案

### 3.1 核心洞察

Tool call history 不是单一数据类型，而是三个层次，每个有不同的访问模式：

| 层次 | 数据 | 访问模式 | 存储 |
|------|------|---------|------|
| **Trace** | 原始执行记录 | 按时间/agent/工具查询 | 日志文件 |
| **Knowledge** | 从 trace 中提取的结构化知识 | 语义搜索、图遍历 | KG + CAS |
| **Skill** | 可复用的高频模式 | 按任务类型检索 | Procedural Memory |

**关键设计决策：用 pipeline 连接现有存储，不引入第四种存储。**

### 3.2 架构

```
Agent 执行
    │
    ▼
┌─────────────────────────────────────┐
│  Trace Layer (新增)                  │
│  tool_trace/<date>/<agent_id>.jsonl │
│  每行一个 Span:                      │
│  {trace_id, parent_id, span_id,     │
│   agent_id, tool_name, input,       │
│   output, status, latency_ms,       │
│   timestamp}                        │
└─────────────┬───────────────────────┘
              │ Cognitive Pipeline (异步)
              ▼
┌─────────────────────────────────────┐
│  Knowledge Layer (已有 KG)           │
│  提取: "Agent X 使用 search 找到 Y"  │
│  存储: (Agent, used, SearchOp)       │
│        (SearchOp, found, Document)   │
│        (SearchOp, triggered_by,      │
│         Intent)                      │
└─────────────┬───────────────────────┘
              │ Skills Evolution (异步)
              ▼
┌─────────────────────────────────────┐
│  Skill Layer (已有 Procedural Memory)│
│  提炼: 高频 tool call 序列 → skill   │
│  存储: MemoryTier::Procedural        │
│  例如: "search_meeting" =            │
│    [search_by_tags("meeting"),       │
│     search_by_keyword(query)]        │
└─────────────────────────────────────┘
```

### 3.3 Trace 存储格式

**文件结构**：`~/.plico/tool_trace/<YYYY-MM-DD>/<agent_id>.jsonl`

**每行一个 Span**：
```json
{
  "trace_id": "uuid",          // 顶层意图的 ID
  "parent_id": "span_uuid",    // 父 span（null = root）
  "span_id": "uuid",           // 本 span 的 ID
  "agent_id": "user-agent-1",
  "tool_name": "semantic_search",
  "input": {"query": "会议记录", "tags": ["meeting"]},
  "output": {"results_count": 5, "top_cid": "abc123"},
  "status": "success",         // success | error | timeout
  "latency_ms": 45,
  "timestamp": "2026-05-17T10:30:00Z",
  "intent_id": "intent-uuid"   // 关联的意图（可选）
}
```

**设计选择**：
- **JSONL**：append-only，每行独立，易解析
- **按日期+agent 分文件**：天然的查询边界，易于清理旧数据
- **不存入 CAS**：trace 是时序数据，不是内容；CAS 的 content-hash 语义不适用
- **不存入 Memory**：Memory 是知识存储，trace 是执行记录；混在一起会污染 recall 结果
- **与 EventBus 解耦**：EventBus 是实时事件分发，trace 是持久化存储；可以 EventBus 发事件 → trace 写入文件

### 3.4 Pipeline 集成

#### 3.4.1 Trace → Knowledge（Cognitive Pipeline）

新增 `CognitiveTask::ExtractTraceKnowledge`：
- 输入：一段时间的 trace 数据
- 处理：提取关键模式（成功/失败的 tool call、常见参数、时间模式）
- 输出：写入 KG 的三元组

示例：
```
(Agent:user-agent-1, frequently_uses, semantic_search)
(semantic_search, effective_for, "meeting records")
(semantic_search, avg_latency_ms, 45)
```

#### 3.4.2 Knowledge → Skill（Skills Evolution）

新增 `CognitiveTask::EvolveSkill`：
- 输入：KG 中的 tool call 模式
- 处理：识别高频序列，评估效果
- 输出：写入 Procedural Memory 的 skill

示例：
```rust
// 从 trace 中观察到的模式
// Agent 搜索会议记录时，先 tag search 再 keyword search 效果最好
Skill {
    name: "search_meeting_records",
    steps: vec![
        Step { tool: "semantic_search", args: "{tags: ['meeting']}" },
        Step { tool: "semantic_search", args: "{query: original_query}" },
    ],
    success_rate: 0.85,
    usage_count: 12,
}
```

### 3.5 查询接口

#### CLI 查询
```bash
# 查看某个 agent 的最近 traces
aicli trace list --agent user-agent-1 --last 10

# 查看某个 trace 的完整 span tree
aicli trace show <trace_id>

# 查看失败的 tool calls
aicli trace failures --agent user-agent-1 --since 7d

# 查看高频 tool call 模式
aicli trace patterns --agent user-agent-1
```

#### API 查询
```json
// 新增 ApiRequest 变体
TraceList { agent_id, since, until, tool_name, status, limit }
TraceShow { trace_id }
TracePatterns { agent_id, min_occurrences }
```

### 3.6 数据生命周期

| 阶段 | 数据 | 保留期 | 清理策略 |
|------|------|--------|---------|
| 原始 trace | tool_trace/*.jsonl | 7 天 | 按日期自动删除 |
| 提取知识 | KG 三元组 | 永久 | 与 KG 一致 |
| 提炼技能 | Procedural Memory | 永久 | 与 Memory 一致 |
| 统计聚合 | 聚合指标 | 30 天 | 按日期自动删除 |

## 4. 与现有系统的关系

### 4.1 EventBus（不冲突）

EventBus 是实时事件分发机制（Write/Update/Delete 事件 → observer）。Trace 是持久化存储。

```
Agent 调用 semantic_search
    │
    ├──→ EventBus: 发出 SearchEvent（实时通知 observer）
    │
    └──→ Trace Store: 写入 span（持久化记录）
```

两者可以共存，职责不重叠。

### 4.2 Cognitive Pipeline（扩展）

现有 pipeline 任务：
- `ProcessDocument`：embedding + KG 提取
- `ExtractKnowledge`：从文档中提取知识

新增 pipeline 任务：
- `ExtractTraceKnowledge`：从 trace 中提取模式
- `EvolveSkill`：从模式中提炼技能

### 4.3 Memory System（桥接）

Trace 中的高频成功模式 → 提炼为 Procedural Memory skill。
这是 **单向流动**：trace → skill，不会反向污染。

## 5. 需要进一步讨论的问题

### Q1: Trace 的写入时机（已决定）

**决定**：立即写入 + 异步通道

**方案**：`mpsc::channel` + 单线程 writer worker
```
Tool call handler → sender.send(trace) → 立即返回（~1μs）
                        ↓
Background worker → receiver.recv() → 序列化 → append JSONL（串行，无锁）
```

**理由**：
- Sync 文件 append 本身 <10μs，但序列化（serde_json）10-100μs + 锁竞争会拖慢 tool call
- mpsc channel 非阻塞，tool call 零额外延迟
- 与现有 Cognitive Pipeline（`CognitiveTask::ProcessDocument`）模式一致
- 崩溃时最多丢失 channel 中的几条 trace（可接受）
- 单线程 writer 消除并发写入的锁问题

### Q2: Trace 的查询性能（已评估）

JSONL 文件按行追加，查询需要扫描。对于 7 天的 trace 数据：
- 假设每天 1000 个 tool call × 500 bytes = 500KB/天
- 7 天 = 3.5MB，全量扫描 < 100ms
- 可以接受，不需要索引

如果未来规模增大，可以：
- 添加按 trace_id 的内存索引
- 或迁移到 SQLite

### Q3: 多 Agent 的 trace 隔离（已决定）

**决定**：每个 agent 有独立的 trace 文件（`<agent_id>.jsonl`）。
跨 agent 的 trace 关联通过 `trace_id`（如果共享意图）。

### Q4: Trace 与 Session 的关系（已决定）

**决定**：松散关联。一个 session 可能包含多个 trace（多个意图），一个 trace 可能跨 session（长任务）。Trace 通过 `session_id` 字段关联 session，但不强绑定。

### Q5: 隐私与安全（已决定）

**决定**：
- Trace 文件存储在 `~/.plico/tool_trace/`（用户本地）
- 不自动上传或共享
- 清理策略默认 7 天，可配置

## 6. 实现路线图

### Phase 1: Trace 基础设施（v52）

**模块**：`src/kernel/trace/`
```
src/kernel/trace/
├── mod.rs          // TraceStore + Span 结构体
├── writer.rs       // mpsc channel + JSONL writer worker
└── query.rs        // CLI/API 查询逻辑
```

**Span 结构体**：
```rust
pub struct Span {
    pub trace_id: String,      // 顶层意图 ID
    pub parent_id: Option<String>,  // 父 span（None = root）
    pub span_id: String,       // 本 span ID
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
- `src/kernel/handlers/*.rs`：各 handler 返回时记录 output

**CLI 命令**：
- `aicli trace list [--agent X] [--last N] [--since 7d]`
- `aicli trace show <trace_id>`
- `aicli trace failures [--agent X] [--since 7d]`

**API 变体**：
- `TraceList { agent_id, since, until, tool_name, status, limit }`
- `TraceShow { trace_id }`

**测试**：
- 单元测试：Span 序列化/反序列化、writer worker 写入
- 集成测试：trace 记录 + CLI 查询

### Phase 2: Knowledge 提取（v53）
- 新增 `CognitiveTask::ExtractTraceKnowledge`
- KG 集成：从 trace 中提取 (Agent, used, Tool), (Tool, effective_for, QueryType) 等三元组
- 语义搜索 trace 知识

### Phase 3: Skills Evolution（v54）
- 新增 `CognitiveTask::EvolveSkill`
- Procedural Memory 集成：高频 tool call 序列 → skill
- 自动技能发现与评估

## 7. 竞争对手对比

| 系统 | Trace | Knowledge | Skill | Plico 差异 |
|------|-------|-----------|-------|-----------|
| Langfuse | Trace tree | 无 | 无 | Plico 有 KG + Skill 层 |
| Letta/MemGPT | 无专用 trace | Episodic Memory | 无 | Plico 有专用 trace + KG |
| TencentDB Agent Memory | 原始对话 | 4 层提炼 | 无 | Plico 有 Skills Evolution |
| EverOS | 无专用 trace | 无 | Skills Evolution | Plico 有完整 pipeline |
| Plico (proposed) | Trace Store | KG 提取 | Skills Evolution | **三层完整 pipeline** |

**Plico 的独特优势**：三层 pipeline（Trace → Knowledge → Skill）是完整的，竞争对手通常只做其中一层或两层。

## 8. 结论

**推荐方案**：
- **存储**：专用 JSONL trace 文件（按日期+agent 分文件）
- **Pipeline**：Trace → KG Knowledge → Procedural Skill（三层渐进提炼）
- **不引入第四种存储**：复用现有 KG + Procedural Memory
- **与 EventBus 解耦**：职责不重叠，可以共存

**下一步**：确认上述 5 个问题的答案后，进入 Phase 1 实现。
