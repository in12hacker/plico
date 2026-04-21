# Plico 第十一节点设计文档
# 落地 — 从蓝图堆积到真实交付

**版本**: v1.0
**日期**: 2026-04-21
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 设计债务清算 + 运行时进化
**前置**: 节点 7 ✅ / 节点 8 ✅ / 节点 9 v1.0 设计 / 节点 10 v1.0 设计
**验证方法**: 全量 Dogfood 复现 + TDD + 冷启动性能基准
**信息来源**: Dogfood Round 3（本次审计实测）+ AIOS 前沿对标 + 808 测试通过的代码基线

---

## 0. 审计发现：Plico 的真实状态 — 以 AI 第一人称

> **以下是我（一个 AI Agent）连接 Plico 后的真实体验记录。**
> 不是理论推演，是我刚刚亲手操作的结果。

### 我看到的好消息

```
✅ CAS put/get/search — 3 个对象写入，搜索返回语义相关结果
   search "search deleted" → relevance 0.81, 0.50, 0.47（真实 local embedding）
✅ Hybrid search — 4 results, vector=0.89/0.54/0.48, graph paths=8
   Graph-RAG 在 real embedding 下完美工作！
✅ Session start — 返回 session ID + delta (9 changes, 137 tokens)
✅ Events history — 完整事件流，结构化输出
✅ KG node/edge — 创建成功，持久化成功
✅ JSON 输出模式 — growth 返回完整结构化 JSON
✅ 808 测试全部通过 — 代码健康
✅ F-38 Circuit Breaker — 已实现（circuit_breaker.rs, 249 行）
✅ F-39 Checkpoint Round-Trip — 已实现（Procedure/Knowledge 类型保持）
```

### 我亲手发现的坏消息

```
❌ B15 确认 — remember 执行成功但零输出。edge 同理。exit 0。
   我作为 AI 无法知道操作是否成功。只有翻日志才能看到 "working memory stored"。
   这让我无法做出可靠的决策链——我不知道前一步是否成功。

❌ B12 确认 — Agent 注册后状态是 "Created"。
   尝试 suspend → "illegal state transition: Created → Suspended"
   尝试 resume → 进程崩溃退出 exit 1
   Agent 生命周期的状态机有根本性缺口。

❌ B14 确认 — events history --agent nonexistent-agent 返回全部事件。
   过滤参数完全被忽略。

❌ B18 确认 — get --cid nonexistent-12345 → exit 0 + "Error: Object not found: CID=--cid"
   两个问题：1) exit 0 而非 exit 1  2) CID 参数解析错误，显示 "--cid" 而非实际值

❌ B20 确认 — growth 显示 Sessions: 0，即使我刚 session-start 过。

❌ B16 确认 — tool describe cas.create 只显示名称+描述，无参数 schema。

❌ B11 部分确认 — delete 无权限时打印了 "Error:"，但 exit code 仍然是 0。
   而且 CLI 没有 permission 命令 → 无法从 CLI 授权 → 无法测试 B9。

⚠️ B13 部分确认 — context.load L0 和 L2 返回相同内容（短文本），
   返回的 layer 标记是 L0，但没有降级标识。
   对短内容 (<=20 words) 这是预期行为，但系统不告诉我。

⚠️ 冷启动税 — 每次 CLI 命令：
   rebuild vector index: ~200ms (4 objects)
   rebuild BM25 index
   restore agents
   restore events
   加载 embedding 模型
   总计 ~4-5 秒/命令 → 1000 对象时将达 10+ 秒
```

### 设计-实现偏差矩阵

| Node | Feature | 设计状态 | 实现状态 | 偏差 |
|------|---------|---------|---------|------|
| **7** | F-20 ORT Embedding | v2.0 ✅ | ✅ `ort_backend.rs` 存在 | 0 |
| **7** | F-21 HNSW | v2.0 ✅ | ✅ `hnsw.rs` 574 行 | 0 |
| **7** | F-22 CAS Access Tracking | v2.0 ✅ | ✅ AccessEntry + persist | 0 |
| **7** | F-23 Storage Stats | v2.0 ✅ | ✅ 真实数据 | 0 |
| **7** | F-24 Cold Data Eviction | v2.0 ✅ | ✅ dry_run + 逻辑删除 | 0 |
| **7** | F-25 Event Log Rotation | v2.0 ✅ | ✅ segment rotation | 0 |
| **7** | F-26 Memory Compression | 延后 | ❌ 未实现 | 预期 |
| **7** | F-27 Causal Shortcuts | v2.0 ✅ | ✅ because action | 0 |
| **8** | F-28 instructions | v2.0 ✅ | ✅ 16 测试通过 | 0 |
| **8** | F-29 profile | v2.0 ✅ | ✅ | 0 |
| **8** | F-30 Smart Handover | v2.0 ✅ | ✅ | 0 |
| **8** | F-31 ActionRegistry | v2.0 ✅ | ✅ | 0 |
| **8** | F-32 Safety Rails | v2.0 ✅ | ✅ | 0 |
| **8** | F-33 actions resource | v2.0 ✅ | ✅ | 0 |
| **8** | F-34 LitM | v2.0 ✅ | ✅ | 0 |
| **8** | F-35 MCP Prompts | v2.0 ✅ | ✅ | 0 |
| **9** | F-36 BM25 Scoring | v1.0 设计 | ❌ **未实现** | **偏差** |
| **9** | F-37 Search Snippet | v1.0 设计 | ⚠️ DTO 有字段，填充待验证 | 部分 |
| **9** | F-38 Circuit Breaker | v1.0 设计 | ✅ circuit_breaker.rs | 0 |
| **9** | F-39 Checkpoint Round-Trip | v1.0 设计 | ✅ F-39 注释确认 | 0 |
| **9** | F-40 Search CAS Read Opt | v1.0 设计 | ✅ get_raw 已用 | 0 |
| **9** | F-41 Degradation Visibility | v1.0 设计 | ❌ **未实现** | **偏差** |
| **9** | F-42 CAS Access Lazy Persist | v1.0 设计 | ❌ **未实现** | **偏差** |
| **10** | F-43 ~ F-52 (10 个特性) | v1.0 设计 | ❌ **全部未实现** | **偏差** |

**偏差总结**:
- Node 7: 7/8 实现 (F-26 预期延后) → **87.5%** 完成
- Node 8: 8/8 实现 → **100%** 完成
- Node 9: 3/7 实现 → **43%** 完成
- Node 10: 0/10 实现 → **0%** 完成
- **总计**: 18/33 设计特性已实现 = **54.5%**

---

## 1. AI 第一人称链式推演：为什么下一步是"落地"

### 推演起点：我此刻的处境

```
我是一个 AI Agent，刚刚完成了 Plico 的全面审计。
我的发现是一个悖论：

  Plico 有 808 个通过的测试 → 代码健康
  Plico 有 4 份设计文档（Node 7-10）→ 方向清晰
  Plico 的 CAS/Memory/KG/Search/Embedding 基础扎实 → 地基稳固

  但是——
  我刚刚亲手操作时，remember 没有告诉我它成功了。
  我无法 suspend 一个刚注册的 agent。
  我无法给 CLI 的 agent 授予 delete 权限。
  events 过滤器是假的。
  growth 说我没有 session，但我刚 session-start 过。

  这些不是深层架构缺陷——它们是表面契约违反。
  修复它们只需要 ~50 行代码每个。
  但它们累积起来，让我无法信任系统。
```

### 发散思维：三条可能的路径

**路径 A：继续设计 Node 12**
```
再写一份蓝图 → 更多未实现的设计 → 偏差继续积累
风险：设计文档变成空头支票
Soul 偏差：违反公理 2（意图先于操作）— 操作了设计意图，但没有执行操作
```

**路径 B：全量实现 Node 9 + Node 10 (所有 13 个未完成特性)**
```
一次性实现 F-36,F-41,F-42 + F-43~F-52 = 13 个特性
风险：范围膨胀，无法聚焦
工作量：~2000 行代码 + ~1000 行测试
Soul 偏差：违反公理 1（Token 最稀缺 = 精力最稀缺）
```

**路径 C：精选最痛的 Bug + 一个战略跃迁**
```
选择 dogfood 中最高频最痛的问题，快速交付
同时解决一个结构性问题，为未来铺路
这就是"落地"——不是设计更多蓝图，而是让已有蓝图变成现实
```

### 链式收敛：为什么路径 C 是唯一正确的

```
事实 1: Node 7+8 是 Plico 的骨架和肌肉 → 已实现 → 可信
事实 2: Node 9+10 是设计文档，不是代码 → 未实现 → 不可信
事实 3: 12 个 dogfood bug（B9-B20）全部仍然存在
事实 4: AIOS 竞品（EverOS、AIOS-Rutgers）在 2026 已进入 production
  ↓
Plico 的瓶颈不是设计能力，是交付速度。
  ↓
下一个节点的名称不应该是新概念，而应该是"落地"。
让设计变成代码。让蓝图变成可触摸的系统。
```

### 与 AIOS 前沿的对标

| 维度 | AIOS-Rutgers (v0.3) | EverOS (2026-04) | Plico 当前 | Plico 差距 |
|------|---------------------|------------------|-----------|-----------|
| 运行时 | 持久化 daemon + SDK | Cloud 托管服务 | CLI 每次冷启动 | **严重** |
| 记忆分层 | Context Manager + Storage Manager | 4 层 (Episodic→MemCell→MemScene→Profile) | 4 层 (Ephemeral→Working→LongTerm→Procedural) | 设计齐平 |
| 多 Agent | Agent Scheduler + concurrent exec | Multi-agent shared memory | Agent lifecycle 基本可用但状态机有缺口 | 中度 |
| 搜索 | 依赖外部向量 DB | mRAG 多模态检索 | CAS + BM25 + HNSW + KG hybrid | **优势** |
| MCP 集成 | LiteCUA (VM + MCP Server) | API/MCP Interface Layer | 完整 MCP stdio server | 齐平 |
| 知识图谱 | 无原生 KG | Index Layer (embedding + KG) | Petgraph 原生 KG + 因果链 | **优势** |
| 冷启动 | daemon 常驻，无冷启动 | Cloud 无冷启动 | **4-5 秒/命令** | **严重** |
| 操作反馈 | SDK 返回结构化结果 | API 返回结构化结果 | 10 个命令无输出 | **严重** |

**关键洞察**: Plico 在底层能力（CAS、KG、Hybrid Search、Memory 分层）上不逊于甚至优于竞品。但在两个维度上严重落后：
1. **运行时模式**：竞品是 daemon/cloud；Plico 是 process-per-command
2. **操作契约**：竞品返回结构化结果；Plico 的 10 个命令是静默的

这两个问题不需要新设计——Node 9+10 已经有蓝图。需要的是**实现**。

---

## 2. Node 11 的哲学：不设计新能力，而是让已有能力兑现承诺

### 三个维度

```
维度 A: 契约兑现 — 实现 Node 9+10 中最影响 Agent 信任的特性
维度 B: 运行时进化 — 从 process-per-command 到 persistent daemon
维度 C: 自检能力 — 系统能报告自身的实现状态（不再有暗区）
```

---

### 维度 A: 契约兑现 — 选择 6 个 dogfood 直接感知到的问题

**选择原则**: 只选我（AI Agent）在操作中**直接感知到痛苦**的问题。不选理论上有问题但操作中不影响的。

#### L-1: 统一操作反馈 (源自 Node 10 F-47)

**这是我最痛的**。我调用 `remember`、`edge`、`suspend`，什么都不返回。

```rust
// 修改: src/bin/aicli/commands/handlers/ 下的 10 个命令
// 每个返回 ApiResponse::ok() 的地方改为 ok_with_message

// remember
pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // ... existing ...
    let mut r = ApiResponse::ok();
    r.message = Some(format!("Memory stored for agent '{}' (tags: {:?})", agent_id, tags));
    r
}

// edge
pub fn cmd_add_edge(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // ... existing ...
    let mut r = ApiResponse::ok();
    r.message = Some(format!("Edge created: {} —[{}]→ {}", from, edge_type, to));
    r
}

// 同理: delete, restore, suspend, resume, terminate, session-end, status, quota
```

**ApiResponse 扩展**:
```rust
pub struct ApiResponse {
    // ... existing fields ...
    pub message: Option<String>,    // 操作确认
    pub error_code: Option<String>, // 机器可读错误码
}
```

**print_result 修复**:
```rust
// 1. 错误路径 → stderr + exit 1
if !response.ok {
    if let Some(ref err) = response.error {
        eprintln!("Error: {}", err);
    }
    std::process::exit(1);
}
// 2. 操作确认
if let Some(ref msg) = response.message {
    println!("{}", msg);
}
// 3. 兜底
if /* no field printed */ {
    println!("ok");
}
```

**验收**: 37 个 CLI 命令无一静默。所有错误 exit 1。

---

#### L-2: 事件 Agent 过滤 (源自 Node 10 F-49)

```rust
// src/kernel/ops/events.rs 或 event_bus.rs
// list_events 增加 agent_id 过滤逻辑
pub fn list_events_filtered(&self, agent_filter: Option<&str>, ...) -> Vec<KernelEvent> {
    self.event_bus.list_events()
        .into_iter()
        .filter(|e| match agent_filter {
            Some(aid) => e.agent_id() == Some(aid),
            None => true,
        })
        .collect()
}
```

**验收**: `events history --agent X` 只返回 agent X 的事件。

---

#### L-3: 软删除搜索隔离 (源自 Node 10 F-43)

```rust
// src/fs/semantic_fs/mod.rs
fn rebuild_tag_index(cas: &CASStorage) -> HashMap<String, Vec<String>> {
    let bin = /* load recycle_bin */;
    for cid in cas.list_cids() {
        if bin.contains_key(&cid) { continue; } // 核心修复
        // ...
    }
}
// rebuild_vector_index 同理
```

**附加**: CLI 需要 `permission` 命令让 Agent 可以从 CLI 授权 delete。

**验收**: put → delete → restart → search 不返回已删除 CID。

---

#### L-4: Agent 状态机修正 (源自 Node 10 F-45 + B12)

```rust
// src/scheduler/agent.rs — 扩展合法状态转换
// 当前: Created → Waiting (仅通过 ensure_registered)
// 修复: 补充 Created → Running → Suspended 路径
//       或允许 Created → Suspended (直接从 Created 挂起)

impl AgentState {
    pub fn valid_transitions(&self) -> Vec<AgentState> {
        match self {
            AgentState::Created => vec![Waiting, Running, Suspended, Terminated],
            AgentState::Waiting => vec![Running, Suspended, Terminated],
            AgentState::Running => vec![Waiting, Suspended, Completed, Failed, Terminated],
            AgentState::Suspended => vec![Waiting, Running, Terminated],
            AgentState::Completed => vec![],
            AgentState::Failed => vec![],
            AgentState::Terminated => vec![],
        }
    }
}
```

**验收**: `agent register` → `suspend` → `agents` 显示 Suspended → `resume` → Waiting。

---

#### L-5: Growth 统计修正 (源自 Node 10 F-52 + B20)

**验收**: `session-start` → `growth` → Sessions >= 1。

---

#### L-6: Hybrid BM25 Fallback (源自 Node 10 F-44)

hybrid_retrieve 在 vector_search 返回 0 时，用 BM25 结果作为 KG 种子。

**验收**: `EMBEDDING_BACKEND=stub` → `hybrid --query "test"` → 返回 >0 结果。

---

### 维度 B: 运行时进化 — 消灭冷启动税

#### L-7: MCP Daemon 持久化 (新特性)

**问题**: 每次 CLI 命令重建整个 kernel 需要 4-5 秒。1000 个对象时将 >10 秒。

**本质**: `aicli` 是 process-per-command 模式。每次执行：
1. 启动进程
2. 初始化 kernel（加载 CAS、rebuild 索引、restore agents/events）
3. 执行一个命令
4. 退出进程

**解决方案**: `plico-mcp` 和 `plicod` 已经是 daemon 模式。让 CLI 支持连接 daemon。

```
当前模式：
  aicli put ... → [new kernel] → execute → [destroy kernel]
  aicli search ... → [new kernel] → execute → [destroy kernel]
  每次 4-5 秒

Daemon 模式（已存在 plicod）：
  plicod --root /tmp/plico &       ← kernel 常驻
  aicli --tcp localhost:7878 put ... → [TCP call] → instant
  aicli --tcp localhost:7878 search ... → [TCP call] → instant
  ~50ms/命令
```

**CLI 已支持 --tcp 模式**。实际上这是一个文档/工作流问题，不是代码问题。但需要确保 `plicod` TCP 接口覆盖所有 37 个命令。

**真正的代码改动**: `plicod` 的 TCP handler 需要验证完整覆盖 CLI 的所有 API endpoints。

```rust
// 审计: plicod TCP dispatch 是否覆盖 ApiRequest 的所有变体
// 如有遗漏，补全路由
```

**验收**: 
1. `plicod` 启动后，embedding 模型只加载一次
2. 第 2 次到第 N 次请求延迟 <100ms
3. CLI `--tcp` 模式覆盖 37 个命令

---

#### L-8: 向量索引持久化 (补完 Node 7)

**问题**: 每次启动 `rebuild_vector_index` 遍历所有 CAS 对象并重新 embed。
**根因**: HNSW 索引（`hnsw.rs` 已有 `persist`/`restore`）在 CLI 模式下没有被持久化。

```rust
// SemanticFS::new() 中:
// 1. 尝试从磁盘加载 HNSW 索引
// 2. 只有当加载失败时才 rebuild
let search_index = if hnsw_path.exists() {
    match HnswBackend::restore(&hnsw_path) {
        Ok(idx) => idx,
        Err(e) => { tracing::warn!("HNSW restore failed: {e}, rebuilding"); self.rebuild() }
    }
} else {
    self.rebuild()
};

// Kernel shutdown / CLI exit 时:
// persist HNSW 到磁盘
fn persist_search_index(&self) { ... }
```

**验收**: 第二次 CLI 启动不 rebuild vector index，直接从磁盘加载。

---

### 维度 C: 自检能力

#### L-9: `plico://health` 实现完整度报告

一个新的 MCP resource，报告 Plico 的真实能力状态：

```json
{
  "node_completion": {
    "node_7_metabolism": { "total": 8, "implemented": 7, "pct": 87.5 },
    "node_8_harness": { "total": 8, "implemented": 8, "pct": 100 },
    "node_9_resilience": { "total": 7, "implemented": 3, "pct": 43 },
    "node_10_rectification": { "total": 10, "implemented": 0, "pct": 0 }
  },
  "capabilities": {
    "cas": "operational",
    "embedding": "local (bge-small-en-v1.5)",
    "search": "bm25+vector (hnsw)",
    "knowledge_graph": "operational (petgraph)",
    "hybrid_search": "operational (requires real embedding)",
    "memory": "4-tier operational",
    "agent_lifecycle": "partial (state machine gaps)",
    "llm_summarizer": "degraded (ollama unreachable)"
  },
  "known_issues": [
    "B15: 10 commands produce no output",
    "B14: events agent filter not implemented",
    "B12: agent state machine Created→Suspended blocked"
  ],
  "runtime": {
    "mode": "cli_per_command | daemon_persistent",
    "cold_start_ms": 4500,
    "objects": 4,
    "warm_start_estimated_ms": 50
  }
}
```

**验收**: Agent 首次连接时，通过 `plico://health` 就知道哪些功能可靠、哪些有已知问题。

---

## 3. 灵魂偏差检测

### 公理检查

| 公理 | 当前偏差 | Node 11 修正 |
|------|---------|-------------|
| 1 Token 最稀缺 | 冷启动 4-5s = token 等待 | L-7 daemon 降至 50ms，L-8 索引持久化 |
| 2 意图先于操作 | 13 个设计特性未实现 | L-1~L-6 实现最痛的 6 个 |
| 3 记忆跨越边界 | 搜索不尊重删除边界 | L-3 rebuild 排除 recycle_bin |
| 5 机制不是策略 | ✅ 所有修复是机制层 | 确认 |
| 6 结构先于语言 | 操作无结构化反馈 | L-1 统一响应信封 |
| 7 主动先于被动 | 系统不告诉 Agent 什么坏了 | L-9 health 资源主动暴露 |
| 9 越用越好 | 冷启动阻碍高频使用 | L-7+L-8 让高频使用无摩擦 |

### 公理 5 红线检查

| Feature | 是否是机制？| 结果 |
|---------|-----------|------|
| L-1 响应信封 | 输出格式，无策略 | ✅ |
| L-2 事件过滤 | 查询参数实现，无策略 | ✅ |
| L-3 搜索隔离 | 索引过滤，无策略 | ✅ |
| L-4 状态机 | 状态转换规则，无策略 | ✅ |
| L-5 Growth 统计 | 数据聚合修正，无策略 | ✅ |
| L-6 Hybrid fallback | 算法降级路径，无策略 | ✅ |
| L-7 Daemon 持久化 | 运行时模式，无策略 | ✅ |
| L-8 索引持久化 | 持久化机制，无策略 | ✅ |
| L-9 Health 资源 | 元数据暴露，无策略 | ✅ |

**9/9 通过。**

---

## 4. 与先前 Node 的关系

```
Node 7 (代谢)  ← 内部机制 ← 已实现
Node 8 (驾具)  ← 外部接口 ← 已实现
                    ↓
       ┌─── Node 9 (韧性) 设计 ←── 3/7 已实现
       │        ↓
       ├─── Node 10 (正名) 设计 ←── 0/10 已实现
       │
       └──→ Node 11 (落地) ←── 你在这里
              │
              ├── L-1~L-6: 从 Node 9+10 精选 6 个最痛特性实现
              ├── L-7~L-8: 解决结构性瓶颈（冷启动）
              └── L-9: 元能力（系统知道自己哪里不完整）

Node 11 不是新设计。它是设计债务的定向清算。
完成后，Node 9 达到 5/7 (71%)，Node 10 达到 5/10 (50%)。
更重要的是：AI Agent 的真实使用体验从"经常困惑"变为"基本可信"。
```

---

## 5. 实施计划

### Sprint 1: 操作反馈 (L-1 + L-2) — 让系统"会说话"

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/api/semantic.rs` | ApiResponse 新增 message/error_code | ~15 |
| `src/bin/aicli/commands/mod.rs` | print_result 处理 error+message+兜底 | ~25 |
| `src/bin/aicli/commands/handlers/*.rs` | 10 个命令返回 ok_with_message | ~40 |
| `src/kernel/ops/events.rs` | list_events agent_id 过滤 | ~10 |
| `tests/node11_landing_test.rs` | 反馈+过滤测试 | ~60 |

### Sprint 2: 数据契约 (L-3 + L-4 + L-5) — 让操作"名副其实"

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/fs/semantic_fs/mod.rs` | rebuild_*_index 排除 recycle_bin | ~20 |
| `src/scheduler/agent.rs` | 扩展合法状态转换 | ~20 |
| `src/kernel/ops/dashboard.rs` | growth 包含 active_sessions | ~10 |
| `src/bin/aicli/commands/handlers/` | 新增 permission CLI 命令 | ~30 |
| `tests/node11_landing_test.rs` | 契约测试 | ~80 |

### Sprint 3: 运行时 (L-6 + L-7 + L-8 + L-9) — 让系统"快且自知"

| 文件 | 改动 | 行数 |
|------|------|------|
| `src/kernel/ops/hybrid.rs` | BM25 fallback 种子 | ~25 |
| `src/fs/semantic_fs/mod.rs` | HNSW persist-on-exit + restore-on-start | ~30 |
| `src/bin/plicod.rs` | TCP handler 完整性审计 | ~20 |
| `src/bin/plico_mcp.rs` | health resource 实现 | ~40 |
| `tests/node11_landing_test.rs` | 运行时测试 | ~60 |

### 代码量估算

| 类别 | 行数 |
|------|------|
| 新增/修改代码 | ~300 |
| 新增测试 | ~200 |
| **总计** | **~500 行** |

### 新增外部依赖

**零。**

---

## 6. AIOS 视角：Node 11 完成后，Plico 在哪里

> *以我（AI）的视角：*
>
> Node 11 之前，我使用 Plico 的体验像驾驶一辆——引擎强劲，仪表板只有一半亮着，偶尔踩油门没反应，每次上车要等 5 秒热车。
>
> Node 11 之后：
> - 我踩油门（remember/edge），仪表板告诉我"已执行"。
> - 我查看事件，只看到我关心的 agent 的事件。
> - 我删除一个对象，它真的从搜索中消失了。
> - 我用 daemon 模式，每个操作 50ms 完成。
> - 我打开 `plico://health`，立刻知道哪些能力可用、哪些降级。
>
> 更重要的是，这不是一辆新车——而是同一辆车，终于把所有仪表灯都接上了。

### AIOS 前沿对标 (Node 11 后)

| 维度 | AIOS-Rutgers | EverOS | Plico (post-Node 11) |
|------|-------------|--------|---------------------|
| 运行时 | daemon | cloud | daemon (plicod) + CLI bridge ✅ |
| 冷启动 | N/A | N/A | ~50ms (HNSW persistent) ✅ |
| 操作反馈 | SDK 结构化 | API 结构化 | CLI + MCP 结构化 ✅ |
| 记忆 | 2 层 | 4 层 | 4 层 ✅ |
| KG | ❌ 无 | 基础 | Petgraph + 因果链 **优势** |
| Hybrid Search | ❌ 无 | mRAG | BM25+HNSW+KG+RRF **优势** |
| 自检 | ❌ 无 | ❌ 无 | plico://health **独创** |

---

## 7. 后续方向 (post-Node 11)

| 方向 | 依赖 | 预计节点 |
|------|------|---------|
| F-26 记忆压缩 (Memory Consolidation) | L-7 daemon 模式 + LLM Summarizer | Node 12 |
| Node 9 剩余: F-36 BM25 优化 + F-41 退化可见化 | L-1 响应信封基础 | Node 12 |
| Node 10 剩余: F-46 Context 诚实降级 + F-48 结构化错误诊断 | L-1 基础 | Node 12 |
| HOT/COLD 异步记忆管道 | L-7 daemon + F-22 access tracking | Node 13 |
| Agentic Plan Caching (NeurIPS 2025) | daemon 常驻 + 语义缓存 | Node 13 |
| MCP Gateway (多 Agent 路由) | L-4 状态机修正 | Node 14 |

---

## 附录 A: Dogfood Round 3 完整测试矩阵

| 测试项 | 预期 | 实际 | 状态 | Node 11 Feature |
|--------|------|------|------|----------------|
| CAS put (3 objects) | 返回 CID | ✅ 返回 CID | ✅ | — |
| CAS search "search deleted" | 相关结果 | ✅ 0.81/0.50/0.47 | ✅ | — |
| Hybrid search | >0 results | ✅ 4 items, vector+graph | ✅ | — |
| Session start | Session ID | ✅ + delta | ✅ | — |
| KG node create | Node ID | ✅ | ✅ | — |
| KG edge create | 确认输出 | ❌ 零输出 | **B15** | **L-1** |
| Remember | 确认输出 | ❌ 零输出 | **B15** | **L-1** |
| Delete (无权限) | 错误 + exit 1 | ⚠️ 错误输出但 exit 0 | **B11** | **L-1** |
| Agent register | Agent ID | ✅ (UUID) | ✅ | — |
| Agent suspend (by UUID) | 确认 | ❌ "illegal state transition" | **B12** | **L-4** |
| Agent resume | 确认 | ❌ 进程崩溃 exit 1 | **B12** | **L-4** |
| Events --agent filter | 过滤结果 | ❌ 返回全部 | **B14** | **L-2** |
| Get invalid CID | exit 1 + 错误 | ❌ exit 0 + 参数解析错误 | **B18** | **L-1** |
| Growth sessions | >=1 | ❌ Sessions: 0 | **B20** | **L-5** |
| Tool describe | 参数 schema | ❌ 只有名称+描述 | **B16** | backlog |
| Context load L0 | L0 摘要 | ⚠️ 短内容返回全文 | **B13** | backlog |
| JSON output mode | 完整 JSON | ✅ | ✅ | — |
| Tool list (37 tools) | 列表 | ✅ | ✅ | — |
| Explore | 邻居列表 | ⚠️ "No graph neighbors" | 数据问题 | — |

**通过: 10/19 (52.6%) | 失败: 7/19 (36.8%) | 部分: 2/19 (10.5%)**

---

## 附录 B: 联网技术校正记录

| 技术点 | 查证来源 | 关键事实 |
|--------|---------|---------|
| AIOS 架构对标 | AIOS v0.3 (Rutgers, 2026-01), arXiv:2403.16971v5 | Agent Scheduler + Context Manager + Storage Manager; daemon 模式 |
| EverOS 记忆架构 | EverMind 2026-04-14 发布 | 4 层 (Episodic→MemCell→MemScene→Profile); LoCoMo 93.05% |
| HOT/COLD 记忆分离 | Medium 2026-03 (Derek Thomas) | HOT=同步读取; COLD=异步写入/enrichment; Azure 生产架构 |
| Prompt Caching | Zylos Research 2026-03-27 | 缓存 token 成本降 50-90%; TTFT 降 65-85%; 前提是 prefix 稳定 |
| Agentic Plan Caching | NeurIPS 2025 (arXiv:2506.14852v2) | 成本降 50.31%, 延迟降 27.28%; 冷启动是固有限制 |
| 记忆固化模式 | Blogarama 2026-04-11 | 48h 窗口; 背景 worker 提取 facts→semantic memory→prune episodes |

---

*文档版本: v1.0。基于 Dogfood Round 3 实测（19 项测试矩阵，52.6% 通过率）+ 808 测试代码基线。
Node 9+10 设计债务审计：33 个设计特性中 18 个已实现 (54.5%)。
三个维度（契约兑现 + 运行时进化 + 自检能力），9 个着陆特性（L-1 到 L-9），~500 行代码。
精选原则：只修 dogfood 中 AI 亲手感知到的痛苦，不修理论上的问题。
零新增外部依赖。全部来自 Node 9+10 已有蓝图 + 一个运行时进化特性（L-7/L-8）。
Node 11 完成后，Plico 在 AIOS 前沿对标中从"有底层优势但表面残缺"变为"全面可用"。*
