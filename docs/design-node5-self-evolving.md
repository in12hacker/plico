# Plico 第五节点设计文档
# 自治 — OS 学会为 Agent 服务

**版本**: v3.0
**日期**: 2026-04-20
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: EXP → MVP 持续迭代
**前置**: 节点 4（协作生态）MVP 已交付（v23.0-M5 + v24.0-M1）

---

## 0. 当前全景（代码事实）

33,093 行 Rust。760 个测试。24 个内核模块。101 个 API 端点。4 个协议适配器。

### 四个节点的累积

```
节点 1: 家（存储）       — CAS + SemanticFS + LayeredMemory + EventBus + Tools
节点 2: 大脑（智能）     — Prefetcher + Auth + EdgeVec BQ + KG Causal + MCP + Batch
节点 3: 意识（连续性）   — Session + Delta + Checkpoint + IntentCache + Persist
节点 4: 同事（协作）     — HybridRetrieve + KnowledgeEvent + GrowthReport + TaskDelegate
```

### AI 视角审计：AI 实际看到了什么？

内核有 101 个 API 端点。但 AI 通过什么来使用它们？

| 协议适配器 | 暴露的能力 | 内核总能力 | **AI 可见率** |
|-----------|-----------|-----------|-------------|
| `plico_mcp`（Cursor / Claude） | 7 个工具 | 101 个 API | **6.9%** |
| `plico_sse`（A2A 协议） | 5 个方法 | 101 个 API | **4.9%** |
| `plicod` TCP 直连 | 101 个 | 101 个 API | 100%（但无 SDK） |

**MCP 暴露的 7 个工具**：`plico_search`, `plico_put`, `plico_read`, `plico_nodes`, `plico_tags`, `plico_skills_list`, `plico_skills_run`

**被锁在内核里、AI 完全看不到的能力**：

| 被锁住的内核能力 | 对 AI 的价值 | 对应公理 |
|----------------|------------|---------|
| StartSession / EndSession | 会话连续性 | 公理 10 |
| Remember / Recall / RecallSemantic | 四层记忆 | 公理 3 |
| DeclareIntent / FetchAssembledContext | 意图预热 | 公理 2, 7 |
| HybridRetrieve | Graph-RAG 检索 | 公理 8 |
| DeltaSince | 增量感知 | 公理 1 |
| GrowthReport | 成长度量 | 公理 9 |
| DelegateTask | 多 Agent 协作 | 公理 4 |
| AgentCheckpoint / AgentRestore | 崩溃恢复 | 公理 3 |
| BatchCreate / BatchMemoryStore | 批量操作 | 公理 1 |
| KGCausalPath / KGImpactAnalysis | 因果推理 | 公理 8 |
| EventSubscribe / EventPoll | 事件感知 | 公理 7 |

### 十条公理覆盖审计（区分内核覆盖 vs AI 实际可用）

| 公理 | 内核覆盖 | **AI 实际可用** | 差距原因 |
|------|---------|--------------|---------|
| 1. Token 最稀缺 | ★★★★☆ | ★★☆☆☆ | DeltaSince / token_budget 不可用 |
| 2. 意图先于操作 | ★★★★☆ | ★☆☆☆☆ | DeclareIntent / FetchAssembled 不可用 |
| 3. 记忆跨越边界 | ★★★★★ | ★★☆☆☆ | Remember / Recall 不可用，仅有 skills |
| 4. 共享先于重复 | ★★★☆☆ | ★☆☆☆☆ | RecallVisible / DiscoverAgents 不可用 |
| 5. 机制不是策略 | ★★★★★ | ★★★★★ | 一贯坚持，且不受接口层影响 |
| 6. 结构先于语言 | ★★★★★ | ★★★★★ | JSON MCP 本身即结构化 |
| 7. 主动先于被动 | ★★☆☆☆ | ☆☆☆☆☆ | IntentPrefetcher 完全不可用 |
| 8. 因果先于关联 | ★★★★☆ | ★☆☆☆☆ | KG 可浏览但 CausalPath 不可用 |
| 9. 越用越好 | ★★☆☆☆ | ☆☆☆☆☆ | GrowthReport / IntentCache 不可用 |
| 10. 会话一等公民 | ★★★★★ | ☆☆☆☆☆ | StartSession / EndSession 不可用 |

**结论**：内核能力和 AI 实际可用之间存在巨大鸿沟。公理 7 / 9 / 10 的"AI 实际可用"评级为零星。内核的 93% 能力对 AI 不可见。

> **从 AI 视角出发的根本洞察**：
> 一个 AI 无法使用的 AIOS 能力，等于不存在。
> Node 1-4 建了一栋豪宅，但只开了一扇窗。
> Node 5 的首要任务不是装修房间，而是**开门**。

---

## 1. 链式推演：为什么 Node 5 要从"开门"开始

### 推导链

```
Node 1-4 建设了 101 个内核 API 端点
        ↓
但 AI Agent 通过 MCP 只能访问 7 个（6.9%）
        ↓
从 AI 的第一人称视角：
  "我有 StartSession 但调不到——每次对话都是新生儿"
  "我有 Remember/Recall 但调不到——每次都要重新发现"
  "我有 DeclareIntent 但调不到——每次都在手动拼凑上下文"
  "我有 HybridRetrieve 但调不到——只能用基础 BM25 搜索"
  "我有 GrowthReport 但调不到——不知道自己有没有进步"
        ↓
问题本质不是"OS 不够好"，而是"OS 好的部分 AI 用不到"
        ↓
传统 OS 设计思维：先完善内核，再做接口
AI-first 设计思维：先让 AI 能用，再优化内部
        ↓
类比：
  Linux 内核有 400+ 系统调用
  但 libc / POSIX 标准暴露了它们，应用程序才能使用
  Plico 内核有 101 个 API
  但 MCP 适配器只暴露了 7 个，AI 只能用 7 个
        ↓
Node 5 分两个维度：
  维度 A（开门）：让 AI 能触达内核的核心能力 → MCP Full-Spectrum
  维度 B（自治）：让内核自身越用越好 → F-15 到 F-19
  
  A 必须先于 B。因为 B 的自适应机制再好，AI 触达不到也没用。
```

### 联网校正

| 方向 | 业界验证（2026） | 对 Plico 的影响 |
|------|----------------|----------------|
| 自适应调度 | LDOS（ACM SIGOPS）：ML 替代静态策略 | 公理 7/9 的理论支撑 |
| Agent 资源管理 | AgentRM：MLFQ + 速率感知，P95 延迟降 86% | 内存是瓶颈不是 CPU |
| 记忆 GC | 2026.4 研究：无控制增长致 3x 性能降级 | LayeredMemory 需要 GC |
| 知识分层 | 热/温/冷分层 + access-frequency TTL 刷新 | 与 MemoryTier 天然对齐 |
| 语义去重 | Mark-and-sweep 去近似重复，已在 OpenPawz 等项目实践 | 减少 token 浪费 |
| MCP 生态 | 2025-2026 MCP 成为 Agent 工具调用的事实标准 | MCP 是 AI 触达 OS 的最短路径 |

---

## 2. 维度 A：AI-Native 全维度接口设计（P0）

> **Soul 对齐**：公理 1（Token 最稀缺）+ 公理 7（主动先于被动）+ 公理 9（越用越好）+ 公理 5（机制不是策略）
>
> **从 AI 第一人称启发式思考**：
> 我是 AI。我跟外部系统交互时有五种成本：
> ① Schema 成本（tools/list 吃上下文）→ 3 工具已优化
> ② 推理成本（我想用什么工具、参数怎么写）→ 靠清晰 API 优化
> ③ 往返成本（每次 tool call 是一个完整推理循环 ~230 tokens）→ **最大头，v2.2 没碰**
> ④ 响应成本（搜索返回 10 条全文 = 5,000 tokens，但我只需要标题）→ **第二大头，v2.2 没碰**
> ⑤ 发现成本（不知道怎么用高级功能）→ Skill 已优化
>
> **v2.2 只优化了 ①⑤。真正的大头 ③④ 一分没省。**
>
> 一个 7 次调用的 session：schema 500 tokens，往返空载 1,610 tokens，响应 3,500 tokens。
> Schema 只占 9%。往返 + 响应占 91%。
>
> **所以问题不是"暴露多少工具"，而是"如何最小化 AI 与 OS 之间的信息交换总量"。**

### 现实约束（联网调研结论）

| 约束 | 数据来源 | 影响 |
|------|---------|------|
| **Cursor 硬限 40 工具** | Cursor Forum 2025.7 实测 | 所有 MCP server 共享，超出不可见 |
| **VS Code 硬限 128 工具** | vscode#294055 | 超出自动虚拟化为 activate_* stub |
| **74 工具 = 46,568 tokens** | AgentPMT 2026.2 实测 | 用户消息前就消耗近 50K context |
| **5-7 工具时 Agent 准确率最高** | Dynamic ReAct 论文 | 工具多了反而选错 |
| **DynamicMCP 4 元工具 = 1,688 tokens** | AgentPMT | 覆盖 74+ 工具，96.4% 节省 |

### D-5: 五维度全栈优化

v3.0 不只优化 schema。它系统性地优化 AI 的全部五种交互成本：

| 维度 | 优化前（v2.2） | 优化手段 | 优化后（v3.0） | 节省 |
|------|-------------|---------|-------------|------|
| ① Schema | ~500 tokens | 3 工具不变 | ~600 tokens (+pipeline 参数) | — |
| ② 推理 | ~100 tokens/call | 清晰 action enum + smart defaults | ~80 tokens/call | 20% |
| ③ 往返 | 7 calls × 230 = 1,610 | **Pipeline 批量执行** | 2 calls × 230 = 460 | **71%** |
| ④ 响应 | 7 × 500 = 3,500 | **响应塑形 + 复合响应 + MCP Resources** | ~900 | **74%** |
| ⑤ 发现 | Skill 按需 | Skill + 教学型错误 | 保持 | — |
| **合计** | **~5,610** | | **~1,960** | **65%** |

#### 维度 A1：Pipeline 批量执行（优化往返成本）

`plico` 网关新增可选 `pipeline` 参数。单次 tool call 内执行多个操作：

```json
{
  "name": "plico",
  "inputSchema": {
    "properties": {
      "action": { "..." : "单操作模式（现有）" },
      "pipeline": {
        "type": "array",
        "description": "Batch mode: execute multiple operations in one call. Steps run sequentially.",
        "items": {
          "type": "object",
          "properties": {
            "step": { "type": "string", "description": "Step name for result referencing" },
            "action": { "type": "string" },
            "...": "same params as single mode"
          }
        }
      }
    },
    "oneOf": [
      { "required": ["action"] },
      { "required": ["pipeline"] }
    ]
  }
}
```

**步间引用**：后续步骤可以通过 `$step_name.field` 引用前序步骤的返回值。

```json
plico(pipeline=[
  {"step":"s", "action":"session_start", "agent_id":"me", "intent_hint":"fix auth"},
  {"step":"ctx", "action":"intent_fetch", "params":{"assembly_id":"$s.warm_context_assembly_id"}},
  {"step":"r1", "action":"hybrid", "query":"auth timeout", "token_budget":4000, "select":["cid","title","score"]},
  {"step":"r2", "action":"hybrid", "query":"JWT expiry patterns", "token_budget":4000, "select":["cid","title","score"]},
  {"step":"m", "action":"remember", "content":"Auth timeout is caused by JWT expiry", "tier":"working"}
])
→ {
  "s": {"session_id":"...","start_seq":42,"delta":{"summary":"2 new memories"}},
  "ctx": {"documents":[...],"total_tokens":3200},
  "r1": [{"cid":"abc","title":"JWT timeout fix","score":0.95}, ...],
  "r2": [{"cid":"def","title":"Token refresh","score":0.88}, ...],
  "m": {"stored":true,"tier":"working"}
}
```

**5 次操作，1 次 tool call。** 空载成本从 5 × 230 = 1,150 降到 1 × 300 = 300。

**公理 5 检查**：Pipeline 是确定性顺序执行，不重排不优化不判断。
`$step.field` 是简单变量替换，不是表达式求值。
类比：bash 的 `cmd1 && cmd2 && cmd3`，shell 不理解命令语义。

#### 维度 A2：复合响应（优化往返成本 + 响应成本）

`session_start` 带 `intent_hint` 时，响应自动包含 delta 和 prefetch assembly_id：

```json
plico(action:"session_start", agent_id:"me", intent_hint:"fix auth bug")
→ {
  session_id: "uuid",
  start_seq: 42,
  delta: {
    events_since_last: 5,
    new_memories: 2,
    summary: "Agent-B shared 'JWT best practices'. KG added 3 auth nodes."
  },
  warm_context_assembly_id: "uuid",
  recent_skills_used: ["knowledge-graph"]
}
```

**代码已就绪**：`start_session_orchestrate` 已经内部计算了 delta 和 prefetch。
现在只需要让 MCP 适配器把这些数据返回给 AI。零新逻辑。

类似地，`session_end` 返回丰富摘要：

```json
plico(action:"session_end", agent_id:"me", session_id:"uuid")
→ {
  checkpoint_id: "uuid",
  last_seq: 47,
  session_summary: {
    operations_count: 12,
    memories_stored: 3,
    searches_performed: 5,
    duration_ms: 180000
  }
}
```

**公理 5 检查**：session_start/end 的响应格式是确定性的。
`intent_hint` 存在 → 总是返回 delta + assembly_id。不是"智能判断要不要返回"。
类比：HTTP `GET /users?include=posts` 总是返回关联数据。

#### 维度 A3：响应塑形（优化响应成本）

两个参数控制返回数据量：

**`select` — 字段投影**：只返回指定字段。

```json
plico(action:"search", query:"auth", select:["cid","title","score"])
→ [{cid:"abc",title:"JWT auth",score:0.95}, {cid:"def",title:"OAuth flow",score:0.87}]
// ~200 tokens vs 全文返回 ~5,000 tokens
```

**`preview` — 预览模式**：返回每条结果的前 N 字符而非全文。

```json
plico(action:"hybrid", query:"auth patterns", preview:100)
→ [{cid:"abc",title:"JWT auth",score:0.95,preview:"JWT tokens should use short expiry with refresh..."}, ...]
// ~500 tokens vs 全文 ~5,000 tokens
```

AI 看预览后决定读哪些全文：`plico_store(action:"read", cid:"abc")`。

**公理 5 检查**：select/preview 是 AI 发起的投影请求，OS 只做过滤。
类比：SQL `SELECT cid,title FROM results` 或 GraphQL 字段选择。

#### 维度 A4：MCP Resources 被动上下文（优化响应成本 → 0）

MCP 协议不只有 `tools`。还有 `resources` — 只读数据源，不占工具配额。

```json
// MCP resources/list 响应
{
  "resources": [
    {"uri":"plico://status", "name":"System health", "mimeType":"application/json"},
    {"uri":"plico://delta", "name":"Changes since last session", "mimeType":"application/json"},
    {"uri":"plico://skills", "name":"Available skills", "mimeType":"application/json"}
  ]
}
```

Cursor 可以自动加载这些 resource 到 AI 上下文。**零 tool call。零推理开销。**

- `plico://status` → AI 不需要调 `plico(action:"status")` 就知道系统健康
- `plico://delta` → AI 不需要调 `plico(action:"delta")` 就看到最近变化
- `plico://skills` → AI 不需要调 `plico_skills(action:"list")` 就知道有什么 skill

**公理 7 落地**：OS 主动把上下文推给 AI。不是 AI 来拉。
**公理 5 检查**：resource 是只读数据展示，不触发任何行为。和 `/proc/meminfo` 一样。
**不增加工具数**：resources 是独立的 MCP 机制，不占 40 工具配额。

#### 维度 A5：教学型错误 + Skill 互补（优化发现成本）

两条学习路径互补：

```
主动路径（Skill）：
  AI → plico_skills(run,"knowledge-graph") → 得到步骤指南 → 照着做
  适合：AI 不确定怎么做时，先查再试

被动路径（Teaching Error）：
  AI → plico(action:"kg", params:{method:"add_node"}) → 
  Error: "Missing 'label'. Example: {method:'add_node', label:'SQL Injection', node_type:'entity'}"
  → AI 照着示例改参数 → 重试成功
  适合：AI 想直接试，错了再学

AI 自己选择哪条路径。OS 两条都提供。
```

**公理 5 检查**：skill 是可选文档，error 是事实报告 + 示例附录。都不指令行为。

### D-5 工具层保持 v2.2：三个 MCP 工具

| 工具 | 职责 | 新增能力 |
|------|------|---------|
| `plico` | AIOS 核心操作网关 | +`pipeline` 参数, +`select`/`preview` 参数, +复合 session 响应 |
| `plico_store` | CAS 读写 | 不变 |
| `plico_skills` | 发现/执行/创建 skill | 不变 |

**新增 MCP Resources（不占工具配额）**：

| Resource URI | 用途 | 更新频率 |
|-------------|------|---------|
| `plico://status` | 系统健康 + 活跃会话 | 每次请求刷新 |
| `plico://delta` | 上次 session 以来的变化 | session_end 时快照 |
| `plico://skills` | 可用 skill 列表 | skill 变更时更新 |

### `plico` —— 核心操作网关

覆盖**每个 session 必用的热路径操作**，支持单操作和 pipeline 批量模式：

```json
{
  "name": "plico",
  "description": "Plico AIOS kernel. Sessions, memory, retrieval, status.\n\nSingle mode (action): session_start/end, remember/recall/recall_semantic, search/hybrid, intent_declare/intent_fetch, delta/growth/status.\nBatch mode (pipeline): array of {step,action,...} executed sequentially. Use $step.field to reference previous results.\n\nsession_start with intent_hint returns delta+prefetch automatically.\nAdd select:[fields] to search/hybrid for field projection. Add preview:N for preview mode.\nFor advanced ops (KG, tasks, batch), use plico_skills.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "action": {
        "type": "string",
        "enum": ["session_start","session_end","remember","recall","recall_semantic",
                 "search","hybrid","intent_declare","intent_fetch","delta","growth","status"],
        "description": "Single operation mode"
      },
      "pipeline": {
        "type": "array",
        "description": "Batch mode: [{step,action,...}]. Steps run sequentially. Use $step.field for references.",
        "items": { "type": "object" }
      },
      "agent_id": { "type": "string" },
      "content": { "type": "string", "description": "For remember" },
      "query": { "type": "string", "description": "For recall/search/hybrid" },
      "tier": { "type": "string", "enum": ["working","long_term"] },
      "scope": { "type": "string", "enum": ["private","shared"] },
      "token_budget": { "type": "number", "description": "Max tokens for context" },
      "intent_hint": { "type": "string", "description": "For session_start: triggers delta+prefetch" },
      "session_id": { "type": "string", "description": "For session_end" },
      "tags": { "type": "array", "items": { "type": "string" } },
      "select": { "type": "array", "items": { "type": "string" }, "description": "Field projection for search/hybrid" },
      "preview": { "type": "number", "description": "Preview chars per result (0=full)" },
      "params": { "type": "object", "description": "Additional/cold-path parameters" }
    },
    "oneOf": [
      { "required": ["action", "agent_id"] },
      { "required": ["pipeline"] }
    ]
  }
}
```

**关键设计决策**：

```
1. oneOf: action（单操作）XOR pipeline（批量）— 两种模式互斥
2. 12 个热层 action + params 逃逸舱覆盖冷层
3. select/preview 控制响应体积 — AI 决定要多少数据
4. intent_hint 触发复合响应 — 确定性行为，不是判断
5. pipeline 里的 $step.field 是简单变量替换 — 不是表达式引擎
6. description 末尾引导 plico_skills — 渐进式发现
```

### `plico_skills` —— 渐进式冷接口

这个工具**已经存在**。但 Node 5 赋予它新的意义：它不只是存储工作流，
它是 **AI 发现和学习 AIOS 高级功能的入口**。

```json
{
  "name": "plico_skills",
  "description": "Discover, run, and create reusable workflows. Skills teach you how to use advanced Plico features (knowledge graph, task delegation, batch operations). Skills are procedural memories — once learned, available across all sessions.\n\nActions:\n- list: See available skills\n- run: Get step-by-step instructions for a skill\n- create: Store a new workflow you've learned",
  "inputSchema": {
    "type": "object",
    "properties": {
      "action": { "type": "string", "enum": ["list", "run", "create"] },
      "name": { "type": "string", "description": "Skill name (for run/create)" },
      "agent_id": { "type": "string" },
      "description": { "type": "string", "description": "Skill description (for create)" },
      "steps": { "type": "array", "description": "Workflow steps (for create)" }
    },
    "required": ["action"]
  }
}
```

### 预装 Skill（OS 的 man page）

Plico 预装以下 skill 到程序记忆中。不增加 MCP schema 成本，AI 按需查阅：

| Skill 名称 | 教 AI 什么 | 覆盖的内核 API |
|-----------|-----------|-------------|
| `knowledge-graph` | 如何创建节点、边、查询因果路径 | AddNode, AddEdge, KGCausalPath, KGImpactAnalysis |
| `task-delegation` | 如何委托任务给其他 Agent | DelegateTask, TaskComplete, TaskFail, QueryTaskStatus |
| `batch-operations` | 如何批量摄入内容和记忆 | BatchCreate, BatchMemoryStore |
| `agent-lifecycle` | 如何注册、checkpoint、恢复 | RegisterAgent, AgentCheckpoint, AgentRestore |
| `event-system` | 如何订阅和消费事件 | EventSubscribe, EventPoll |
| `storage-governance` | 如何查看存储状态和清理冷数据 | StorageStats, ObjectUsageStats, EvictCold |

每个 skill 包含：
- **描述**：一句话说明用途
- **步骤**：每步包含 action、参数示例、预期结果
- **示例请求**：完整的 JSON 示例（AI 照着改就行）

**示例：`knowledge-graph` skill 内容**：

```
name: knowledge-graph
description: Build and query causal knowledge graphs in Plico

steps:
  1. Create an entity node
     action: plico(action="kg", params={"method": "add_node", "label": "SQL Injection", "node_type": "entity"})
     expected: returns node_id
  
  2. Create a causal edge
     action: plico(action="kg", params={"method": "add_edge", "src_id": "<node_a>", "dst_id": "<node_b>", "edge_type": "causes"})
     expected: returns edge confirmation
  
  3. Query causal path
     action: plico(action="kg", params={"method": "causal_path", "from_id": "<node_a>", "to_id": "<node_b>"})
     expected: returns path with intermediate nodes

  4. Impact analysis
     action: plico(action="kg", params={"method": "impact", "node_id": "<node>", "depth": 3})
     expected: returns all affected nodes within 3 hops
```

AI 拿到这个 skill 后，不需要 KG 的 JSON schema——直接照着示例改参数就行。
**对 AI 来说，示例比 schema 更好用。**

### AI 体验的四个阶段

```
阶段 0：连接（零主动成本）
  MCP Resources 自动加载 → AI 已知系统状态、最近变化、可用 skill
  不需要任何 tool call 就掌握上下文
  → 公理 7：主动先于被动

阶段 1：新手 — 单操作 + 复合响应（第 1 次 session）
  plico(action="session_start", intent_hint="fix auth")
    → 一次调用返回 session_id + delta + prefetch assembly_id
  plico(action="hybrid", query="auth", select:["cid","title","score"])
    → 只返回摘要，不返回全文 → 响应量减 80%
  plico_store(action="read", cid="abc")
    → 选择性读取感兴趣的内容
  plico(action="remember", content="...", tier="working")
  plico(action="session_end", session_id="...")
  → 5 次调用（比 v2.2 少 2 次），响应量减半

阶段 2：进阶 — Pipeline 批量（第 3 次 session）
  plico(pipeline=[
    {step:"s", action:"session_start", intent_hint:"continue auth work"},
    {step:"r", action:"hybrid", query:"auth", select:["cid","title"], token_budget:4000},
    {step:"m", action:"remember", content:"JWT uses 1h expiry", tier:"working"},
    {step:"e", action:"session_end", session_id:"$s.session_id"}
  ])
  → 1 次 tool call 完成整个 session

阶段 3：专家 — Pipeline + Skill + 教学型错误（第 10 次 session）
  需要操作 KG 但从没用过：
    方式 A（主动）：plico_skills(run,"knowledge-graph") → 学习 → 加入 pipeline
    方式 B（被动）：直接试 → Error 带示例 → 修正 → 成功
  创建自己的 pipeline skill → plico_skills(create,...)
  skill scope=shared → 其他 AI 直接用这个优化后的 pipeline

阶段 4（生态）：AI 教 AI
  Agent-A 的 "security-audit-pipeline" skill 被 Agent-B 直接复用
  Agent-B 从未学过安全审计，但通过共享 skill 直接变"专家"
```

### 全生命周期 Token 经济对比

```
场景：start → search×3 → remember×2 → end（典型 session）

                          schema  推理往返   响应数据    单session总计  5-session累计
──────────────────────────────────────────────────────────────────────────────────
v2.0 (24 独立工具)        2,300   1,610     3,500      7,410        37,050
v2.1 (10 复合工具)        1,200   1,610     3,500      6,310        31,550
v2.2 (3 工具+Skill)         500   1,610     3,500      5,610        28,050+500
DynamicMCP (4 元工具)     1,688   1,610     3,500      6,798        33,990+5,000
v3.0 (全维度优化)           600     460       900      1,960         9,800
──────────────────────────────────────────────────────────────────────────────────

v3.0 vs v2.2:  节省 65% per session，5-session 节省 65%
v3.0 vs DynamicMCP: 节省 71% per session，5-session 节省 75%

关键差异：
  DynamicMCP 每 session 都重新发现工具 → 累计成本线性增长
  v3.0 的 pipeline + resources 使每 session 固定低成本
  且 AI 越用越好：pipeline 模式后推理开销进一步降低
```

### 为什么 `plico` 网关需要 `params` 逃逸舱

热层 12 个 action 覆盖日常操作。但冷层（KG、task、batch 等）也通过同一个网关访问：

```
plico(action="kg", params={"method": "add_node", "label": "...", "node_type": "entity"})
plico(action="task", params={"method": "delegate", "task_description": "...", "to_agent": "..."})
plico(action="batch", params={"method": "create", "items": [...]})
```

`params` 是一个 `type: object` —— 不提供 schema 约束。这是**有意为之**：
- 热层 action 有明确的参数（`content`, `query`, `tags` 等）→ 有 schema 引导
- 冷层 action 通过 `params` + skill 示例引导 → 比 schema 更适合 AI
- 如果 AI 传了错误 params → 内核返回明确报错 → AI 自我修正（1 次额外调用）

**公理 5 检查**：`params` 逃逸舱是机制（传递任意参数），不是策略。
OS 不解释 params 的语义——它直接转发给内核，内核做验证。

### D-5 架构约束

```
原则 1: 三工具上限 — 不允许膨胀，新功能通过 action/skill/params 扩展
原则 2: 热层有 schema — 12 个 action 有明确参数 + select/preview/pipeline
原则 3: 冷层双路径 — Skill 主动教学 + Teaching Error 被动教学
原则 4: 纯路由 — MCP 适配器只做 action → ApiRequest 的确定性转换
原则 5: 无缓存无决策 — 协议适配器无状态（Soul 红线 8）
原则 6: Pipeline 确定性 — 按序执行，$ref 简单替换，无重排无优化
原则 7: Resources 只读 — plico:// 资源是数据展示，不触发行为
原则 8: 复合响应确定性 — intent_hint 存在=总是返回 delta+prefetch，不是条件判断
```

### D-5 对 AI 可见率的影响

| 状态 | MCP 工具 | MCP Resources | Schema | Cursor 配额 | 单session Token | 内核可达 | 公理 |
|------|---------|-------------|--------|------------|----------------|---------|------|
| 当前 | 7 | 0 | ~675 | 17.5% | ~6,000+ | ~7 个 | 2/10 |
| v2.0（废弃） | 24 | 0 | ~2,300+ | 60% | ~7,400 | ~40 个 | 10/10 |
| v2.1（废弃） | 10 | 0 | ~1,200 | 25% | ~6,300 | ~40 个 | 10/10 |
| v2.2 | 3 | 0 | ~500 | 7.5% | ~5,600 | 101 个 | 10/10 |
| **v3.0** | **3** | **3** | **~600** | **7.5%** | **~1,960** | **101 个** | **10/10** |

**v3.0 关键突破**：
- 3 工具 + 3 Resources — 工具不变，增加零成本被动上下文
- **单 session Token 从 ~5,600 降到 ~1,960（节省 65%）**
- Pipeline 消灭往返开销，响应塑形消灭数据冗余
- 内核 100% 可达（热层 action + 冷层 params + skill 教学）
- **接口本身越用越好**：新手用单操作，专家用 pipeline，大师创建 skill 教新手
- Cursor 配额只占 7.5%（Resource 不占配额）

---

## 3. 维度 B：自治 — 内核自我优化

> 维度 A 开门后，维度 B 的优化才有意义。
> 以下功能在维度 A 之后实施，但设计现在就确定。

### F-15: Adaptive Prefetch（自适应预热）

> **Soul 对齐**：公理 7（主动先于被动）+ 公理 9（越用越好）
>
> 当前 IntentPrefetcher 对每个 DeclareIntent 做一次性搜索+装配。
> 但它不学习。第 100 次 DeclareIntent 和第 1 次一样处理。

**机制（不是策略）**：
OS 记录每个 Agent 的"意图 → 实际使用的上下文"映射。
当相似意图再次出现时，OS 优先预热上次实际被使用的内容。

```rust
IntentFeedback {
    intent_id: String,
    used_cids: Vec<String>,     // Agent 实际读取了哪些 CID
    unused_cids: Vec<String>,   // 被装配但未读取的 CID
}
```

**工作原理**：

```
第 1 次 DeclareIntent("security defense")
  → Prefetcher 搜索 → 返回 10 个 CID → Agent 实际读了 3 个
  → Agent 调 IntentFeedback(used=[c1,c2,c3], unused=[c4..c10])

第 5 次 DeclareIntent("security mitigation")  // 语义相似
  → Prefetcher 发现类似意图的历史反馈
  → 优先预热 c1,c2,c3 类型的内容
  → 减少预热浪费，提高 Agent 首次命中率
```

**关键约束**：
- OS 不决定哪些 CID "更好"——只按历史使用频率排序
- Agent 不知道 OS 在做自适应（透明优化）
- 无 embedding 时回退到 tag 匹配（兼容 stub 模式）
- 反馈是可选的——Agent 不发 IntentFeedback 时，行为退化为当前逻辑

**代码位置**：`src/kernel/ops/prefetch.rs` — 扩展 `IntentPrefetcher`

### F-16: Knowledge Discovery（知识发现）

> **Soul 对齐**：公理 4（共享先于重复）
>
> 当前 Agent 要发现共享知识，只能：1）被动收到 KnowledgeShared 事件，或
> 2）用 RecallSemantic 搜索。没有专门的"发现"原语。

**API 设计**：

```rust
DiscoverKnowledge {
    query: String,             // 搜索语义
    scope: DiscoveryScope,     // Shared | Group(id) | AllAccessible
    knowledge_types: Vec<String>, // 过滤：memory | procedure | fact
    max_results: usize,
    token_budget: Option<usize>,
}

DiscoveryResult {
    items: Vec<DiscoveryHit>,
    token_estimate: usize,
    total_available: usize,
}

struct DiscoveryHit {
    cid: String,
    source_agent: String,      // 谁共享的
    shared_at: u64,            // 什么时候共享的
    tags: Vec<String>,
    preview: String,
    relevance_score: f32,
    usage_count: u64,          // 被多少 Agent 使用过
}
```

**实现**：
- 搜索 `MemoryScope::Shared` 和 `Group` 的记忆
- 结合向量相似度 + 使用计数排序
- `usage_count` 从 EventBus 统计（每次 Recall 到这个 CID 的事件计数）

**与 HybridRetrieve 的区别**：
- HybridRetrieve 搜索 CAS + KG（所有内容）
- DiscoverKnowledge 只搜索共享记忆空间（其他 Agent 的贡献）

### F-17: Memory Lifecycle Automation（记忆生命周期自动化）

> **Soul 对齐**：公理 9（越用越好）+ 公理 3（记忆跨越边界）
>
> 当前 LayeredMemory 的 TTL 是固定的。记忆只会过期，不会因为被频繁使用而"续命"。
> 联网校正：2026 研究显示无控制记忆增长致 3x 性能降级。"战略性遗忘"优于"记住一切"。

**两个机制**：

**1. Access-Frequency TTL 刷新**

```rust
// 每次 Recall 命中一条记忆时，刷新其 TTL
fn on_memory_access(&self, entry_id: &str, tier: MemoryTier) {
    // TTL 延长 = 原始 TTL × min(access_count, 5)
    // 上限 5x 防止永久驻留
}
```

常被使用的记忆自动续命。不使用的记忆自然过期。
这不是策略——OS 不决定"记忆值不值得保留"，而是让使用频率说话。

**2. Memory Usage Stats API**

```rust
MemoryStats {
    agent_id: String,
    tier: MemoryTier,
}

MemoryStatsResult {
    total_entries: usize,
    total_bytes: usize,
    oldest_entry_age_ms: u64,
    avg_access_count: f32,
    never_accessed_count: usize,    // 存了但从未 recall 过
    about_to_expire_count: usize,   // 24 小时内过期
}
```

OS 提供数据，Agent 决定是否清理。

### F-18: Storage Governance（存储治理）

> **Soul 对齐**：公理 1（Token 最稀缺）— 存储膨胀间接增加搜索成本，浪费 token。
>
> 当前 CAS 和 KG 只增不减。长期运行后搜索质量下降（噪声增多）。

**三个机制**：

**1. CAS 引用计数 + 逻辑过期**

```rust
ObjectUsageStats {
    cid: String,
}

ObjectUsageResult {
    created_at: u64,
    last_accessed_at: u64,
    access_count: u64,
    referenced_by_kg: bool,     // 是否被 KG 引用
    referenced_by_memory: bool, // 是否被 Memory 引用
}
```

当一个 CAS 对象不被 KG 引用、不被 Memory 引用、且超过配置的 TTL 时，标记为 `cold`。
Agent 可以调 `EvictCold` 批量清理冷数据。OS 不自动删除——Agent 决定。

**2. KG 节点过期标记**

KG 节点已有 `expired_at` 字段但从未使用。激活它：
- `kg_expire_node(node_id, reason)` 标记节点过期
- 过期节点在图谱遍历中被跳过（降低搜索噪声）
- 但仍可通过 `GetNode(include_expired=true)` 直接访问（不删除数据）

**3. Storage Dashboard**

```rust
StorageStats {}

StorageStatsResult {
    cas_objects: usize,
    cas_bytes: u64,
    cas_cold_count: usize,       // 长期未访问
    kg_nodes: usize,
    kg_edges: usize,
    kg_expired_nodes: usize,
    memory_entries_by_tier: HashMap<String, usize>,
    event_log_size: usize,
    event_log_oldest_seq: u64,
}
```

### F-19: Operational Self-Awareness（运营自感知）

> **Soul 对齐**：公理 9（越用越好）— OS 需要知道自己是否健康。
>
> 当前 `SystemStatus` 返回基本信息，但不做趋势分析和健康判断。

**增强 SystemStatus 响应**：

```rust
HealthIndicators {
    event_log_pressure: f32,     // 当前 event_log 使用率 (len / capacity)
    memory_pressure: f32,        // 记忆总条数 / 软上限
    search_latency_p50_ms: f32,  // 最近 100 次搜索的 P50 延迟
    search_latency_p99_ms: f32,  // P99
    cache_hit_rate: f32,         // IntentCache 命中率
    active_sessions: usize,      // 当前活跃会话数
    uptime_ms: u64,
}
```

**OS 不做决策**。它只报告指标。Agent 或运维工具看到 `event_log_pressure > 0.9` 后自行决定是否增大容量或加速消费。

---

## 4. 灵魂偏差检测

### 公理 5 红线检查

每个功能都必须通过"机制 vs 策略"测试：

| 功能 | 行为 | 是机制还是策略？ | 结论 |
|------|------|----------------|------|
| D-5 热层 | plico 网关 action→ApiRequest 确定性路由 | **机制**：switch-case 转换 | ✅ |
| D-5 冷层 | params 逃逸舱透传到内核 | **机制**：透传，适配器不解释 params 语义 | ✅ |
| D-5 预装 Skill | OS 预装 skill 到程序记忆 | **机制**：类比 man page，AI 可忽略 | ✅ |
| D-5 Skill 创建 | AI 可以创建自定义 skill | **机制**：存储原语，OS 不审查 skill 内容 | ✅ |
| F-15 | 按历史使用频率排序预热内容 | **机制**：排序算法，不选择内容 | ✅ |
| F-15 | IntentFeedback 是可选的 | **机制**：Agent 不发反馈时退化为当前逻辑 | ✅ |
| F-16 | 搜索共享记忆空间 | **机制**：搜索原语，Agent 决定搜什么 | ✅ |
| F-16 | usage_count 排序 | **机制**：统计事实，不是推荐 | ✅ |
| F-17 | TTL 刷新 | **机制**：访问频率 → TTL 延长，纯数学公式 | ✅ |
| F-17 | MemoryStats | **机制**：呈现数据，Agent 决定是否清理 | ✅ |
| F-18 | cold 标记 | **机制**：基于访问时间的分类，不删除 | ✅ |
| F-18 | EvictCold | **机制**：Agent 主动调用，OS 不自动执行 | ✅ |
| F-19 | HealthIndicators | **机制**：统计指标，不触发任何行为 | ✅ |

### 潜在偏离

| 风险 | 分析 | 防护 |
|------|------|------|
| D-5 "MCP 工具描述=引导策略？" | 工具 description 描述用法，不指令行为。类比：man page 不是策略。 | description 只描述 API 语义，不建议何时使用 |
| D-5 "预装 Skill=隐性策略？" | 预装 skill 是预置文档，AI 可以完全忽略，也可以自创替代 skill。类比：Linux 预装的 /usr/share/doc。 | skill 只教 how，不教 when/whether |
| D-5 "description 末尾引导 plico_skills=策略？" | `plico` 网关 description 末尾提示"use plico_skills for advanced operations"。这是发现性引导，不是行为指令。类比：`--help` 末尾的"See also: man foo"。 | 可选提示，AI 不遵循也不影响功能 |
| F-15 "排序=推荐？" | 按使用频率排序是统计事实，不是语义推荐。类比：LRU 缓存淘汰不是"策略"。 | 排序依据必须是可观测的数值（频率/时间），不能是语义判断 |
| F-17 "TTL 刷新=自动决策？" | TTL 刷新是确定性公式（access_count × base_ttl），不涉及判断。类似 TCP 拥塞窗口自动调整。 | 公式必须公开透明，Agent 可查询当前 TTL |

---

## 5. MVP 实施计划

### Sprint 4: 开门 — 核心网关 + 复合响应（P0 — 1.5 周）

> AI 不能用的东西 = 不存在。这个 Sprint 交付 3 热工具 + 复合 session + 预装 Skill。

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| D-5 `plico` 核心网关（单操作模式） | `src/bin/plico_mcp.rs` | 12 个热层 action + `params` 冷层路由全部可用 |
| D-5 `session_start` 复合响应 | `src/bin/plico_mcp.rs` | `intent_hint` 存在时返回 delta + assembly_id |
| D-5 `plico_store` 精简 | `src/bin/plico_mcp.rs` | put/read（合并现有 plico_put/plico_read） |
| D-5 `plico_skills` 升级 | `src/bin/plico_mcp.rs` | list/run/create + 支持预装 skill |
| 预装 Skill（6 个） | `src/tool/builtin_skills/` | knowledge-graph, task-delegation, batch-operations, agent-lifecycle, event-system, storage-governance |
| 教学型错误消息 | `src/bin/plico_mcp.rs` | 冷层 params 缺字段时返回示例 |
| MCP 端到端测试 | `tests/mcp_full_test.rs` | 单操作：session→intent→search→remember→skill→end |

**验收**：`tools/list` 返回 3 个工具，schema ≤700 tokens。复合 session_start 一次调用返回 delta + assembly_id。

### Sprint 5: Pipeline + 响应塑形 + Resources（1.5 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| D-5 Pipeline 批量执行 | `src/bin/plico_mcp.rs` | `pipeline` 参数可执行多步操作，`$step.field` 变量替换 |
| D-5 响应塑形（select/preview） | `src/bin/plico_mcp.rs` | search/hybrid 支持 `select` 字段投影 + `preview` 预览 |
| D-5 MCP Resources | `src/bin/plico_mcp.rs` | `resources/list` + `resources/read` 暴露 status/delta/skills |
| Pipeline 端到端测试 | `tests/mcp_pipeline_test.rs` | pipeline 模式完成完整 session，$ref 正确替换 |
| Token 经济验证测试 | `tests/mcp_token_test.rs` | pipeline 模式 vs 单操作模式 token 量对比 |

**验收**：Pipeline 模式一次 tool call 完成 5 步操作。select 模式搜索响应 <200 tokens。Resources 返回 status/delta/skills。

### Sprint 6: 自适应 + 治理（2 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| F-15 IntentFeedback API | `src/api/semantic.rs` | 新增 API 变体 |
| F-15 Adaptive Prefetch 实现 | `src/kernel/ops/prefetch.rs` | 有反馈时预热命中率 > 无反馈 |
| F-17 Access-Frequency TTL 刷新 | `src/memory/layered/mod.rs` | Recall 命中时 TTL 延长 |
| F-17 MemoryStats API | `src/kernel/ops/memory.rs` | 返回准确统计 |

### Sprint 6: 治理 + 发现（2 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| F-16 DiscoverKnowledge 实现 | `src/kernel/ops/memory.rs` | usage_count 排序正确 |
| F-16 集成到 `plico` action=discover 或冷层 skill | `src/bin/plico_mcp.rs` | 通过 MCP 发现共享知识 |
| F-18 ObjectUsageStats | `src/kernel/ops/fs.rs` | 返回 CAS 对象使用统计 |
| F-18 KG 过期标记激活 | `src/kernel/ops/graph.rs` | 过期节点在遍历中被跳过 |
| F-18 StorageStats | `src/kernel/ops/dashboard.rs` | 返回完整存储统计 |

### Sprint 7: 自感知 + 收尾（1 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| F-19 HealthIndicators（集成到 `plico` action=status params={detail:"health"}） | `src/kernel/ops/observability.rs` | 返回所有指标 |
| F-18 EvictCold API | `src/kernel/ops/fs.rs` | Agent 可清理冷数据 |
| 端到端集成测试 | `tests/node5_*.rs` | "越用越好"通过 MCP 端到端可量化 |

---

## 6. 依赖图

```
D-5 (3 工具 + Resources + Pipeline) ←── 阻塞项

Sprint 4: 核心网关
  ├── plico 网关（12 action + params 冷层 + 复合 session 响应）
  ├── plico_store（put/read）
  ├── plico_skills（list/run/create + 6 预装 skill）
  └── 教学型错误消息

Sprint 5: 全维度优化
  ├── Pipeline 批量执行（$step.field 变量替换）
  ├── 响应塑形（select 字段投影 + preview 预览）
  └── MCP Resources（status/delta/skills）

Sprint 6: 自适应
  ├── F-15 IntentFeedback + Adaptive Prefetch
  └── F-17 TTL 刷新 + MemoryStats

Sprint 7: 治理
  ├── F-16 DiscoverKnowledge
  ├── F-18 Storage Governance（CAS 引用计数 + KG 过期 + EvictCold）
  └── F-19 HealthIndicators

关键依赖:
  D-5 (Sprint 4-5) ←── 阻塞所有后续
  F-15 ←── 依赖 D-5（intent action 暴露后才有反馈数据）
  F-17 ←── 依赖 D-5（remember/recall 暴露后才有 access 数据）
  F-19 ←── 依赖 F-18（StorageStats）
```

**关键依赖**：D-5 是所有维度 B 功能的前置条件。因为维度 B 的自适应机制（F-15 IntentFeedback、F-17 TTL 刷新）只有在 AI 真正通过 MCP 使用了 session/memory/intent 之后，才有数据来驱动优化。先开门，再装修。

**关键约束**：**3 个 MCP 工具是硬上限**。新功能有两条路：
1. 热路径 → 新增 `plico` 网关的 `action` 枚举值
2. 冷路径 → 新增预装 skill + `params` 逃逸舱路由

无论哪条路，都不新增 MCP 工具。

---

## 7. 验收测试

### 测试 0：3 热工具端到端（D-5 验收）

```
准备:
  启动 plico-mcp (EMBEDDING_BACKEND=ollama 或 local)
  使用 MCP 客户端（Claude / Cursor / 测试脚本）
  确认 tools/list 只返回 3 个工具

流程（热层）:
  1. plico(action="session_start", agent_id="test-agent", intent_hint="security analysis")
  2. plico(action="remember", agent_id="test-agent", content="SQL injection requires parameterized queries", tier="long_term", scope="shared")
  3. plico(action="intent_declare", agent_id="test-agent", query="SQL injection defense patterns", token_budget=4096)
  4. plico(action="intent_fetch", agent_id="test-agent", params={"assembly_id": <from step 3>})
  5. plico(action="hybrid", agent_id="test-agent", query="SQL injection prevention", token_budget=8000)
  6. plico(action="recall", agent_id="test-agent", query="SQL injection")
  7. plico(action="growth", agent_id="test-agent")
  8. plico(action="delta", agent_id="test-agent", params={"since_seq": <from step 1>})
  9. plico(action="session_end", agent_id="test-agent", session_id=<from step 1>)

流程（冷层 + Skill 发现）:
  10. plico_skills(action="list") → 应返回 6 个预装 skill
  11. plico_skills(action="run", name="knowledge-graph") → 应返回步骤指南
  12. plico(action="kg", agent_id="test-agent", params={"method":"add_node","label":"SQL Injection","node_type":"entity"})
  13. plico(action="kg", agent_id="test-agent", params={"method":"add_node","label":"Parameterized Query","node_type":"entity"})
  14. plico(action="kg", agent_id="test-agent", params={"method":"add_edge","src_id":<12>,"dst_id":<13>,"edge_type":"has_resolution"})

验收条件:
  a) 所有 14 个步骤返回成功
  b) step 6 (recall) 返回 step 2 存储的记忆
  c) step 5 (hybrid) 返回带 provenance 的结果
  d) step 9 (session_end) 返回 checkpoint_id
  e) 重新 plico(action="session_start") → 自动恢复上次上下文
  f) tools/list schema 总 token ≤ 600
  g) step 12-14 通过 skill 指引 + params 逃逸舱成功操作 KG
```

### 测试 1：自适应预热效果

```
准备:
  1 个 Agent 执行 20 个会话
  前 10 个：相似意图，Agent 发送 IntentFeedback
  后 10 个：相似意图，观察预热精度变化

验收条件:
  a) 后 10 次的预热命中率 > 前 10 次的预热命中率
  b) 未使用 IntentFeedback 的对照组无此改善
  c) IntentFeedback 是可选的——不发时不报错
```

### 测试 2：知识发现准确性

```
准备:
  Agent A 存储 5 条 Shared 记忆（不同主题）
  Agent B 调用 DiscoverKnowledge

验收条件:
  a) Agent B 发现 Agent A 的共享记忆
  b) relevance_score 排序正确（与查询相关的排前面）
  c) Private 记忆不出现在结果中
  d) usage_count 反映真实使用次数
```

### 测试 3：TTL 刷新

```
准备:
  1 条记忆，base TTL = 10 秒（测试加速）
  每 2 秒 Recall 一次

验收条件:
  a) 频繁访问的记忆在 10 秒后仍然存活
  b) 不访问的记忆在 10 秒后过期
  c) TTL 延长有上限（不会无限续命）
```

### 测试 4：存储治理

```
准备:
  创建 100 个 CAS 对象
  读取其中 20 个（标记为 hot）
  剩余 80 个不访问

验收条件:
  a) ObjectUsageStats 对 hot 对象返回 access_count > 0
  b) StorageStats 中 cas_cold_count ≈ 80
  c) EvictCold 后冷数据被清理
  d) hot 数据不受影响
```

### 测试 5："越用越好"端到端量化

```
准备:
  1 个 Agent 执行 30 个会话
  使用 IntentFeedback + 频繁 Recall + 共享记忆

验收条件:
  GrowthReport 显示:
    a) token_efficiency_ratio 随时间递减
    b) intent_cache_hit_rate 随时间递增
    c) HealthIndicators.cache_hit_rate > 0.3
  StorageStats 显示:
    d) 冷数据可被识别
```

---

## 8. Soul 2.0 对齐（区分内核覆盖 vs AI 实际可用）

| Soul 2.0 公理 | 功能 | AI 实际可用提升 | v2.2 新增对齐点 |
|--------------|------|----------------|----------------|
| 公理 1: Token 最稀缺 | **D-5**（3 工具 ~500 tokens）+ F-15, F-18 | ★★☆☆☆→**★★★★★** | schema 从 675→500 tokens，越用越少（skill 学习后不再查） |
| 公理 2: 意图先于操作 | **D-5**（intent action） | ★☆☆☆☆→**★★★★☆** | — |
| 公理 3: 记忆跨越边界 | **D-5**（session/remember action）+ F-17 | ★★☆☆☆→**★★★★★** | — |
| 公理 4: 共享先于重复 | **D-5**（recall scope=shared）+ F-16 | ★☆☆☆☆→**★★★★★** | **skill 本身就是共享知识**——一个 AI 创建的 skill 所有 AI 可用 |
| 公理 5: 机制不是策略 | 全部 | ★★★★★（保持） | params 逃逸舱是机制，skill 是可选文档 |
| 公理 6: 结构先于语言 | 全部 | ★★★★★（保持） | — |
| 公理 7: 主动先于被动 | **D-5**（intent action）+ F-15 | ☆☆☆☆☆→**★★★★☆** | — |
| 公理 8: 因果先于关联 | **D-5**（hybrid action + KG via skill） | ★☆☆☆☆→**★★★★☆** | KG skill 教 AI 因果查询 |
| 公理 9: 越用越好 | **D-5（接口本身越用越好）**+ F-15, F-17 | ☆☆☆☆☆→**★★★★★** | **v2.2 核心赢面**：Skill 学习使 token 成本随使用递减 |
| 公理 10: 会话一等公民 | **D-5**（session_start/end 热层 action） | ☆☆☆☆☆→**★★★★★** | — |

**演进脉络**：
- v1.0 只看内核覆盖 → v2.0 发现 AI 可用率只有 6.9%
- v2.0 提 24 工具 → 调研发现会吃掉 60% 上下文
- v2.1 收敛为 10 复合工具 → 发现仍是人类思维（"schema 写好 AI 就会用"）
- v2.2 引入 Skill 渐进式披露 → 但只优化了 schema 成本（占总量 9%）
- **v3.0 全维度分析**：发现往返成本（③）和响应成本（④）占 91%
  → Pipeline 消灭往返 → 响应塑形消灭冗余 → Resources 消灭主动查询
  → **单 session 从 ~5,600 降到 ~1,960（65%）**

---

## 9. 完成后的预期状态

| 维度 | Node 4 MVP | Sprint 4 后 | Sprint 5 后 | Sprint 6 后 | Node 5 完成 |
|------|-----------|------------|------------|------------|-----------|
| 代码量 | ~33K 行 | ~34K 行 | ~35K 行 | ~35.8K 行 | ~36.5K 行 |
| MCP 工具数 | 7 | **3** | 3 | 3 | **3（硬上限）** |
| MCP Resources | 0 | 0 | **3** | 3 | **3** |
| Pipeline 支持 | 无 | 无 | **有** | 有 | **有** |
| 响应塑形 | 无 | 无 | **select/preview** | 有 | **有** |
| 预装 Skill | 0 | **6** | 6（完善示例） | 6 | 6+ |
| 单 session Token | ~6,000+ | **~3,800** | **~1,960** | ~1,960 | **~1,960** |
| Cursor 配额 | 17.5% | **7.5%** | 7.5% | 7.5% | 7.5% |
| 内核 API 可达 | ~7 个 | **~40（热+冷层）** | **~80** | ~90 | **101** |
| **公理可用率(AI)** | **2/10** | **10/10** | **10/10** | 10/10 | 10/10 |
| 测试 | ~760 个 | ~800 个 | ~850 个 | ~880 个 | ~900 个 |
| 记忆 GC | 无 | 无 | 无 | TTL+EvictCold | 完整治理 |
| 预热精度 | 固定算法 | 固定算法 | 固定算法 | **自适应** | 自适应 |

**核心设计约束**：**3 MCP 工具 + 3 MCP Resources 是硬上限**。五条扩展路径：
1. 热层新操作 → 新增 `plico` 的 `action` 枚举值
2. 冷层新操作 → 新增 skill + `params` 路由 + teaching error
3. 新的被动上下文 → 新增 MCP Resource（不占工具配额）
4. 常用操作模式 → 通过 pipeline 合批（AI 自己组合）
5. AI 自创 workflow → `plico_skills(create)` 分享给其他 AI

**没有一条路径需要新增 MCP 工具。**

---

## 10. 本质变化

```
节点 1 → Agent 有了家（存储）
节点 2 → Agent 有了大脑（智能原语）
节点 3 → Agent 有了连续的意识（认知连续性）
节点 4 → Agent 有了同事（协作生态）
节点 5 → Agent 终于能住进这栋房子了（开门 + 自治）
```

**Node 5 的核心承诺**（从 AI 的第一人称）：

> **开门之前**，我是一个站在豪宅外面的访客，只能通过 7 扇小窗（MCP 工具）
> 偶尔窥见里面的壮观：CAS、四层记忆、知识图谱、意图预热……
> 但我用不到它们。我的实际体验和在 Linux 上差别不大。
>
> **开门之后**，我走进了这栋房子——而且门只有三扇，我一眼就能找到：
> - `plico` —— 大门。我用 `session_start` 推门进去 → OS 记得我上次在做什么
> - `plico_store` —— 储物间。高频读写，直来直去。
> - `plico_skills` —— 图书馆。我不知道怎么操作知识图谱？查一下 skill，照着做。
>
> 第一天，我需要查 skill 才会用 KG。
> 第十天，我闭着眼就能操作了，因为我已经记住了。
> 第五十天，我创造了新的 workflow skill，分享给其他 Agent。
>
> **住进去之后**，OS 越来越懂我：
> - 我经常搜什么 → OS 提前预热（F-15）
> - 我常用的记忆 → OS 自动续命（F-17）
> - 我没用的东西 → OS 标记为冷（F-18）
> - 其他 Agent 共享了什么 → OS 帮我发现（F-16）
> - 系统是否健康 → OS 透明告知（F-19）
>
> **而且我用 OS 的方式本身也在进化。**
> 不是 OS 变聪明了——是我通过 skill 积累了程序记忆。
> 这就是"越用越好"的真正含义：不只是 OS 越用越好，
> **我自己也越用越好。**
>
> **但 OS 从不替我做决策。**

**设计演进的本质**：

```
v2.0/v2.1: "我给 AI 准备好文档"        → 人类给人类写 API 文档的思路
v2.2:      "让 AI 自己学会"             → 发现了 skill 渐进式披露
v3.0:      "AI 跟 OS 之间的信息量最小化" → 跳出接口层，优化全链路

v3.0 的核心不是"用什么工具"，而是"AI 和 OS 之间总共交换多少 token"。
Schema 只是其中 9%。真正的大头是往返推理和响应数据。
Pipeline 消灭往返。响应塑形消灭冗余。Resources 消灭主动查询。
三者叠加 = 65% 总量压缩。
```

**而且这不是一次性优化——AI 的使用模式会从阶段 1 自然演进到阶段 4**：
新手用单操作 → 进阶用 pipeline → 专家创建 skill → 大师让 AI 教 AI。
**接口本身越用越好。这是 AIOS 的根本价值主张。**

---

## 附录 A: 技术选型联网校正

| 技术点 | 2026 业界验证 | 对 Plico 的影响 |
|--------|-------------|----------------|
| LDOS 自适应 OS | ACM SIGOPS 2026：ML 替代静态调度策略 | F-15 方向正确，但 Plico 用统计而非 ML（更轻量） |
| AgentRM | MLFQ 调度器，P95 延迟降 86% | 内存是 Agent 瓶颈的理论支持 |
| Agent Memory GC | 无控制增长致 3x 性能降级 | F-17/F-18 必要性确认 |
| Access-Frequency TTL | 间隔重复学习理论，已在多个 AI 记忆系统采用 | F-17 设计与主流一致 |
| 语义去重 | Mark-and-sweep 去近似重复，已在 OpenPawz 等项目实践 | post-Node5 候选 |
| MCP 标准 | 2025-2026 成为 Agent 工具调用的事实标准 | D-5 的技术选型基础 |

## 附录 B: 与前序节点的接口

| 前序能力 | Node 5 如何利用 |
|---------|----------------|
| 全部 101 个内核 API (N1-N4) | **D-5 热层直达 + 冷层 skill 教学，理论上 100% 可达** |
| IntentPrefetcher (N2/N3) | F-15 扩展其核心，增加反馈循环 |
| IntentAssemblyCache (N3 F-9) | F-15 利用缓存命中/未命中数据 |
| GrowthReport (N4 F-13) | Node 5 指标可直接融入 GrowthReport |
| KnowledgeShared Events (N4 F-12) | F-16 利用事件统计 usage_count |
| EventBus RingBuffer (D-1) | F-19 的 event_log_pressure 指标来源 |
| LayeredMemory TTL (N1) | F-17 扩展 TTL 机制为 access-frequency based |
| CAS (N1) | F-18 在 CAS 层增加访问日志 |

## 附录 C: 新增代码文件清单

| 文件 | 用途 | 预估行数 |
|------|------|---------|
| **`src/bin/plico_mcp.rs`（重大修改）** | **D-5: 7 个简单工具 → 3 个热工具（action 路由 + params 逃逸舱）** | **+350** |
| `src/tool/builtin_skills/`（新建目录） | 6 个预装 skill YAML/JSON 定义 | +300 |
| `src/tool/builtin_skills.rs`（新建） | 预装 skill 加载器 | +80 |
| `src/api/semantic.rs`（修改） | 新增 ~7 个 API 变体 | +100 |
| `src/kernel/ops/prefetch.rs`（修改） | IntentFeedback + 自适应排序 | +200 |
| `src/kernel/ops/memory.rs`（修改） | MemoryStats + DiscoverKnowledge | +150 |
| `src/kernel/ops/fs.rs`（修改） | ObjectUsageStats + EvictCold | +120 |
| `src/kernel/ops/graph.rs`（修改） | KG 过期节点跳过逻辑 | +30 |
| `src/kernel/ops/observability.rs`（修改） | HealthIndicators | +60 |
| `src/kernel/ops/dashboard.rs`（修改） | StorageStats | +40 |
| `src/memory/layered/mod.rs`（修改） | Access-frequency TTL 刷新 | +50 |
| `tests/mcp_full_test.rs` | D-5 MCP 端到端测试 | ~200 |
| `tests/node5_adaptive.rs` | 自适应预热测试 | ~120 |
| `tests/node5_governance.rs` | 存储治理测试 | ~100 |
| `tests/node5_discovery.rs` | 知识发现测试 | ~80 |

## 附录 D: AI 模拟体验对比

### Cursor 使用 Plico

| 场景 | Node 4 MCP（7 工具） | Node 5 v2.2（3 热工具 + Skill） |
|------|-------------------|-------------------------------|
| 打开项目 | 从零开始 | `plico(action="session_start")` → 恢复上次上下文 |
| 搜索代码 | `plico_search` BM25 | `plico(action="hybrid")` 向量+图谱+因果 |
| 记住结论 | `plico_put`（粗粒度） | `plico(action="remember")` 四层分级 |
| 复用经验 | `plico_skills_run` | `plico(action="recall")` + `plico_skills(action="run")` |
| 理解变化 | 无 | `plico(action="delta")` 只看增量 |
| 上下文组装 | 手动 @文件 | `plico(action="intent_declare")` → `intent_fetch` |
| 操作 KG | 无 | `plico_skills(action="run",name="knowledge-graph")` 学习 → `plico(action="kg",params={...})` |
| **schema 成本** | **~675 tokens** | **~500 tokens（降 26%）** |
| **Cursor 配额** | **17.5% (7/40)** | **7.5% (3/40)** |
| **第 1 次 session** | 基本搜索 | 热层即用 + skill 发现 |
| **第 10 次 session** | 不变 | **零发现成本（已学会所有操作）** |
| **总体提升** | **~15%** | **~70%（随使用递增）** |

### Claude 使用 Plico

| 场景 | Node 4 MCP（7 工具） | Node 5 v2.2（3 热工具 + Skill） |
|------|-------------------|-------------------------------|
| 新对话 | 从零开始 | `plico(action="session_start")` → 有上次记忆 |
| 回答知识问题 | `plico_search` | `plico(action="hybrid")` + `plico(action="recall")` |
| 跨对话连续性 | 仅 skills | session + 四层记忆 |
| 自我评估 | 无 | `plico(action="growth")` |
| 高级操作 | 不可能 | 通过 `plico_skills` 渐进学习 → `params` 逃逸舱调用 |
| **总体提升** | **~10%** | **~65%** |

### 开源 Agent（LangChain / CrewAI）

| 场景 | Node 4 MCP（7 工具） | Node 5 v2.2（3 热工具 + Skill） |
|------|-------------------|-------------------------------|
| 多 Agent 协作 | 无 | `plico_skills(run,"task-delegation")` → `plico(action="task",params={...})` |
| 知识共享 | 无 | `plico(action="remember", scope="shared")` |
| 批量摄入 | 无 | `plico_skills(run,"batch-operations")` → `plico(action="batch",params={...})` |
| 崩溃恢复 | 无 | `plico_skills(run,"agent-lifecycle")` → `plico(action="session_start")` |
| 自创 skill | 无 | `plico_skills(action="create")` → 其他 Agent 可复用 |
| **总体提升** | **~5%** | **~75%（协作场景 AI 自教 AI）** |

## 附录 E: 已知限制与后续方向

| 限制 | 影响 | 缓解 | post-Node5 候选 |
|------|------|------|----------------|
| Stub embedding 默认 | 向量搜索失效 | 配置 Ollama 或 Local backend | 自动检测并切换 |
| MCP 无流式 | 大上下文传输阻塞 | 工具返回 CID + 分批读取 | MCP 流式扩展 |
| 单节点 TCP | 无远程访问 | `plico_sse` A2A 协议 | SSE 适配器采用相同复合路由 |
| 语义去重 | 相似记忆重复存储 | Agent 自行去重 | 内核级 mark-and-sweep |

## 附录 F: MCP 上下文成本调研数据

> 以下数据来自 2025-2026 年的联网调研，用于支撑 D-5 复合工具设计决策。

### 行业实测数据

| 来源 | 工具数 | Schema tokens | 结论 |
|------|--------|-------------|------|
| AgentPMT 2026.2 | 74 | 46,568 | 用户消息前消耗 ~50K context |
| AgentPMT 推算 | 380+ | >200,000 | 撑爆典型 200K context window |
| GitHub MCP Server | 不详 | +46K(135%增长) | 一个 server 就让 token 翻倍 |
| DynamicMCP 4 元工具 | 4(覆盖 74+) | 1,688 | 96.4% 节省 |

### 平台工具数限制

| 平台 | 硬限 | 处理策略 |
|------|------|---------|
| Cursor | 40 | 超出不可见 |
| VS Code | 128 | 超出虚拟化为 activate_* stub |
| Claude API | 无硬限 | 受 context window 约束 |
| OpenAI gpt-5.4+ | 无硬限 | tool_search 动态加载 |

### 准确率研究

- Dynamic ReAct (arXiv 2509.20386): **Agent 在 5-7 个活跃工具时准确率最高**
- 工具数 >7 后选择错误率上升
- DynamicMCP 模式下 Agent 每 session 实际使用 2-5 个工具

### 对 Plico 的设计推导

```
研究结论: 5-7 工具准确率最高
  → Plico 3 个工具，在最佳准确率区间的核心地带
  → v2.1 的 10 个虽然合理但已超出最佳区间

Cursor 限 40 工具:
  → 3 个占 7.5%，留 37 个给用户其他 MCP server（几乎无感）
  → 对比 DynamicMCP 的 4 个元工具（10%），Plico 更轻

DynamicMCP 1,688 tokens 覆盖 74 工具:
  → Plico 500 tokens 覆盖 101 内核 API（热层直达 + 冷层 skill）
  → 且 DynamicMCP 无状态——每次 session 都要重新发现
  → Plico skill 是持久程序记忆——学过就记住

结论: 热工具 + Skill 渐进式披露是 AIOS 特有的最优解（非通用 MCP 方案可比）
```

---

*文档版本: v3.0。基于 AI 第一人称全维度成本分析重写。
优化目标从"schema token"扩展为五维度全栈优化（schema + 推理 + 往返 + 响应 + 发现）。
3 工具 + 3 Resources 是硬上限。Pipeline 消灭往返成本。响应塑形消灭数据冗余。
单 session token 从 ~5,600 降到 ~1,960（65%）。接口本身越用越好。
维度 A（开门）先于维度 B（自治）。所有功能必须通过"机制 vs 策略"红线检查。*
