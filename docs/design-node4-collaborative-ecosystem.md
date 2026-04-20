# Plico 第四节点设计文档
# 协作生态 — 从单 Agent 智能到多 Agent 涌现

**版本**: v2.0
**日期**: 2026-04-19
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: EXP → MVP 交付
**前置**: 节点 3（认知连续性）全部完成
**驱动场景**: 2000 篇网络安全技术文章的 AI 知识库

---

## 0. 项目阶段与交付标准

### 阶段演进

```
POC（已完成）→ EXP（已完成）→ MVP（本节点已交付 v23.0-M5 + v24.0-M1）
```

| 阶段 | 目标 | 质量标准 | 技术债容忍度 |
|------|------|---------|-------------|
| POC | 验证核心假设 | 能跑通 | 允许——快速验证优先 |
| EXP | 验证完整场景 | 可复现 | 不允许——场景可靠性优先 |
| **MVP** | **可交付产品** | **可依赖** | **零容忍——用户信任优先** |

**MVP 的含义**：一个外部开发者可以用 Plico 构建 Knowledge Agent，接入 2000 篇安全文章，提供可靠的 Q&A 服务。不需要了解内部实现，不需要绕过已知 bug，不需要担心数据丢失。

### MVP 交付标准

1. **零已知技术债** — 所有 D-0 项在 MVP 之前完成，不是"以后修"
2. **崩溃恢复** — 任意时刻 kill 进程，重启后数据完整
3. **API 稳定** — 新增 API 遵循 v17.0 版本管理，不做破坏性变更
4. **场景端到端** — 知识库摄入 → 查询 → 答案的完整链路可工作
5. **测试覆盖** — 每个新功能 ≥ 3 个测试用例，关键路径有集成测试

---

## 1. 链式推演：为什么是"协作生态"

### 从 AI 视角的推导链

```
节点 1 → Agent 有了家（存储）
节点 2 → Agent 有了大脑（智能原语）
节点 3 → Agent 有了连续的意识（认知连续性）
        ↓
问题：一个 Agent 再强，也只是一个大脑。
      2000 篇安全文章 × 多个技术领域 × 持续更新 = 单 Agent 认知负载过重
        ↓
推论：需要多个 Agent 各自专精，通过 OS 协作形成涌现智能
        ↓
节点 4 → Agent 有了同事（协作生态）
```

### 知识库场景的真实需求驱动

用户场景：2000 篇网络安全技术文章覆盖漏洞分析、渗透测试、防御体系、合规标准等。
人类提问 → AI 查询 Plico 中的相关知识 → 给出综合答案。

**从 AI 视角重新理解这个场景**：

人类看到的是"问答系统"。但从 AI 的第一人称视角：

> 我是一个 Knowledge Agent。Plico 是我的大脑基础设施。
> 2000 篇文章是我的长期记忆。知识图谱是我的关联网络。
> 当人类问我一个问题时，我不是在"搜索文档"——
> 我是在**回忆**：哪些知识片段与这个问题相关？它们之间有什么因果链？
> 我记得上次有人问过类似问题吗？我上次的回答被接受了吗？
> 我能把这次的洞察共享出去，让其他 Agent 下次不用重新推导吗？

这个场景天然需要节点 4 的核心能力：

| 需求 | 为什么单 Agent 不够 | 节点 4 能力 |
|------|-------------------|-----------|
| 2000 篇文章的深度索引 | 单 Agent 上下文窗口装不下 | OS 层 Hybrid Retrieval |
| CVE → 攻击手法 → 防御策略的因果链 | 需要多跳推理，单次搜索不够 | Graph-RAG 原语 |
| "上次谁问过类似的？" | 需要跨 Agent 记忆共享 | 知识广播 + 共享记忆 |
| 知识持续更新 | 需要感知变化 | 事件驱动通知 |
| "我比上个月更了解这个领域了吗？" | 需要自我评估 | 成长报告 |

---

## 2. 基础加固（与功能并行，MVP 前置条件）

> MVP 不容忍技术债。以下项不是"Phase 0"——它们是 MVP 的**准入条件**。
> 任何新功能的 PR 不得合入，直到其依赖的加固项已完成。

### D-1: EventLog 容量管理

**问题**：`event_log: RwLock<Vec<SequencedEvent>>` 只追加不截断。长运行实例必然 OOM。

**现状代码**（`src/kernel/event_bus.rs:133-145`）：
```rust
event_log: RwLock::new(Vec::new()),  // 无界增长
// emit() 只有 push，永远没有 truncate
```

**修复方案**：

```rust
struct RingEventLog {
    events: VecDeque<SequencedEvent>,
    max_capacity: usize,  // 默认 65536（可配置）
    min_seq: u64,         // 最小保留 seq，用于 events_since 越界检测
}

impl RingEventLog {
    fn push(&mut self, event: SequencedEvent) {
        if self.events.len() >= self.max_capacity {
            if let Some(evicted) = self.events.pop_front() {
                self.min_seq = evicted.seq + 1;
            }
        }
        self.events.push_back(event);
    }

    fn events_since(&self, since_seq: u64) -> Result<Vec<SequencedEvent>, EventLogGap> {
        if since_seq < self.min_seq {
            return Err(EventLogGap { requested: since_seq, oldest: self.min_seq });
        }
        Ok(self.events.iter().filter(|e| e.seq > since_seq).cloned().collect())
    }
}
```

**关键决策**：
- 默认 65536 条（约 10MB 内存）。选择依据：2000 篇文章的全量摄入产生约 6000 事件（create + KG node + KG edge），65536 可覆盖 10 轮完整摄入周期
- 当 Agent 调 `events_since(seq)` 时，若 seq 已被淘汰，返回 `EventLogGap` 错误而非静默丢失
- `EventLogGap` 告诉 Agent："你落后太多了，请做一次全量同步"
- 通过 `KernelConfig` 可调整容量

**技术选型**：VecDeque 是 Rust 标准库的环形缓冲实现，零额外依赖，性能满足 MVP。若未来需要无锁并发，`photon-ring`（2026.3 发布）可作为升级路径，但 MVP 不需要过度优化。

### D-2: Broadcast Channel 容量 + Lag 回补

**问题**：`broadcast::channel(256)` 容量过小。慢消费者丢消息，当前只 `tracing::warn`。

**修复方案**：
1. 容量 256 → 4096（16x，内存增量 < 1MB）
2. Lagged 时从 `event_log` 回补

```rust
Err(broadcast::error::TryRecvError::Lagged(n)) => {
    tracing::warn!("Subscription {} lagged by {} events, recovering from event_log", subscription_id, n);
    let recovered = self.events_since(last_seen_seq);
    events.extend(recovered);
    continue;
}
```

**容量比约束**：D-1 容量（65536）必须 ≥ 16× D-2 容量（4096），确保回补成功率 > 99.9%。

### D-3: token_estimate 全覆盖

**问题**：`estimate_tokens()` 存在但大多数 `ApiResponse` 未填充。MVP 用户看不到 token 成本。

**修复范围**：

| 响应类型 | 填充方式 | 优先级 |
|---------|---------|--------|
| `FetchAssembledContext` | 每个 `LoadedContext` 的内容 | P0 — 用户每次查询都用 |
| `SearchResult` | 每条搜索结果的内容 | P0 |
| `MemoryRecalled` | 每条记忆条目 | P0 |
| `HybridResult`（新增） | 每个 `HybridHit` 的 content_preview | P0 — 节点 4 新功能 |
| `DeltaSince` | 每条变更条目 | P1 |
| `GraphExploreResult` | 每个节点的 label + properties | P1 |

**token 估算精度**：当前公式 `(ascii+3)/4 + (non_ascii+1)/2` 是 MVP 可接受的。在 API 文档中标注 "estimate, ±20%"。后续版本可用 `tiktoken-rs` 校准。

### D-4: Soul 红线架构决策文档化

**问题**：Soul 2.0 红线 "内核零模型"，但 `AIKernel` 中直接调用 `self.embedding.embed()` 和 `self.summarizer.summarize()`。

**事实**：
- `src/kernel/ops/memory.rs` — `remember_long_term()` 调用 `self.embedding.embed()`
- `src/kernel/ops/prefetch.rs` — `declare_intent_impl()` 调用 `self.embedding.embed()`
- `src/fs/semantic_fs/mod.rs` — `store_object()` 调用 `summarizer.summarize()`

**架构决策**（不是妥协，是有意识的选择）：

| 维度 | 分析 |
|------|------|
| 为什么不分离 | Embedding 和 Summarizer 在存储热路径上，进程间通信增加 2-10ms 延迟，批量摄入 2000 篇文章时累计延迟不可接受 |
| 为什么可接受 | 通过 `EmbeddingProvider` / `Summarizer` trait 抽象，内核不绑定任何具体模型。`HotSwapProvider` 支持运行时切换 |
| 边界在哪里 | 仅限存储路径（索引构建、摘要生成）。内核永远不调用 LLM 做推理、决策、文本生成 |
| 演进路径 | 当 Plico 进入生产阶段且延迟预算允许时，可将 embedding/summarizer 分离为 sidecar。trait 抽象已为此预留 |

**修复**：在 `system-v2.md` 红线 2 增加附注：

> **红线 2 附注**：`EmbeddingProvider::embed()` 和 `Summarizer::summarize()` 是内核调用的唯二模型接口。
> 它们通过 trait 抽象实现模型无关，仅用于存储路径（索引构建、摘要生成），不用于推理决策。
> 此决策基于性能实测：批量摄入场景中，进程内调用比 IPC 快 10x+。
> 边界硬约束：任何推理/决策/文本生成的模型调用，禁止进入内核。

---

## 3. 核心功能

### F-11: Hybrid Retrieval（Graph-RAG 原语）

> **Soul 2.0 对齐**：公理 2（意图先于操作）+ 公理 8（因果先于关联）
> **MVP 必要性**：知识库场景的核心检索能力。没有它，Agent 只能分别做向量搜索和图谱遍历再自行合并——浪费 token、增加延迟、结果质量不可控。

**为什么需要**：

当前 Plico 有两条检索路径，但它们是割裂的：
- **向量搜索**（SemanticFS）：找语义相似的内容
- **图谱遍历**（KnowledgeGraph）：沿着关系边走

对于知识库场景，Agent 需要的是两者融合：

```
问题："SQL 注入攻击的防御措施有哪些？已知 CVE-2024-XXXX 的修复方案？"
              │
    ┌─────────┴─────────┐
    ▼                   ▼
向量搜索              图谱遍历
找到 5 篇语义        从 "SQL注入" 节点
相关文章              沿 Causes → 找到相关 CVE
                      沿 HasResolution → 找到修复方案
    └─────────┬─────────┘
              ▼
         合并 + 去重 + 按相关度排序
              │
              ▼
    返回带因果路径的结果集
```

**联网校正**：Graph-RAG（2026 主流范式）在复杂多跳查询上准确率 94%，纯向量搜索仅 61%。因果链接准确率从 18% 提升到 76%。这正是网络安全领域的核心需求——CVE → 攻击路径 → 防御措施的因果链。

**API 设计**：

```rust
HybridRetrieve {
    query_text: String,           // 语义查询文本
    seed_tags: Vec<String>,       // 可选：KG 种子节点的 tag 过滤
    graph_depth: u8,              // 图谱遍历深度（默认 2）
    edge_types: Vec<String>,      // 可选：限定边类型（Causes, HasResolution 等）
    max_results: usize,           // 最大返回数（默认 20）
    token_budget: Option<usize>,  // token 预算限制
}

HybridResult {
    items: Vec<HybridHit>,
    token_estimate: usize,        // D-3 要求：必须填充
    vector_hits: usize,
    graph_hits: usize,
    paths_found: usize,
}

struct HybridHit {
    cid: String,
    content_preview: String,
    vector_score: f32,
    graph_score: f32,
    combined_score: f32,
    provenance: Vec<ProvenanceStep>,
}

struct ProvenanceStep {
    from_cid: String,
    edge_type: String,
    hop: u8,
}
```

**内核实现逻辑**：

```
HybridRetrieve 处理流程:
1. 向量检索：query_text → embedding → search_backend.search_semantic()
   → vector_seeds: Vec<(cid, score)>

2. KG 种子扩展：每个 vector_seed 的 cid，在 KG 中查找对应节点
   → CID 有 KG 节点则添加为图谱种子

3. 图谱遍历：从种子节点出发，沿指定 edge_types 遍历 graph_depth 层
   → 收集所有到达的节点及路径

4. 合并去重：向量结果 + 图谱结果，按 CID 去重
   → combined_score = α × vector_score + (1-α) × graph_score
   → α = 0.6（默认值，Agent 可通过参数组合调整检索策略）

5. Token 预算裁剪：按 combined_score 降序，累加 token_estimate
   → 达到 token_budget 时截断

6. 返回：带 provenance（来源路径）的结果集
```

**复用现有代码**：
- 向量搜索：`search_backend.search_semantic()`（src/fs/semantic_fs/mod.rs）
- 图谱遍历：`kg.get_neighbors()`（src/kernel/ops/graph.rs）
- 权威度：`kg.authority_score()`
- Token 估算：`estimate_tokens()`

不造新轮子——这是编排层操作，组合现有原语。

### F-12: Knowledge Event（知识事件增强）

> **Soul 2.0 对齐**：公理 4（共享先于重复）+ 公理 7（主动先于被动）
> **MVP 必要性**：知识库场景中，Ingest Agent 摄入新文章后，Knowledge Agent 必须能感知到。没有事件通知，只能轮询——违反灵魂公理 7。

当前 `KernelEvent` 有 6 个变体，没有关于"知识变化"的事件。

**新增变体**：

```rust
KnowledgeShared {
    cid: String,
    agent_id: String,
    scope: String,        // "shared" | "group:{group_id}"
    tags: Vec<String>,
    summary: String,      // 元数据拼接，不依赖 LLM
},
KnowledgeSuperseded {
    old_cid: String,
    new_cid: String,
    agent_id: String,
},
TaskDelegated {           // F-14 支撑
    task_id: String,
    from_agent: String,
    to_agent: String,
},
TaskCompleted {           // F-14 支撑
    task_id: String,
    agent_id: String,
    result_cids: Vec<String>,
},
```

**触发时机**：
- `KnowledgeShared`：`MemoryScope::Shared` 或 `Group` 记忆存储时自动 emit
- `KnowledgeSuperseded`：SemanticFS 中 `Supersedes` 边创建时自动 emit
- `TaskDelegated` / `TaskCompleted`：F-14 任务状态变化时 emit

**关键约束**：
- OS 只通知"有新知识"，不推送内容（节省 token）
- Agent 决定是否拉取（机制不是策略）
- summary 是元数据拼接，不调用 LLM

### F-13: Growth Report（成长报告）

> **Soul 2.0 对齐**：公理 9（越用越好）
> **MVP 必要性**：用户需要看到 AIOS 的价值。GrowthReport 是 Agent 向用户证明"越用越好"的数据依据。

**API 设计**：

```rust
QueryGrowthReport {
    agent_id: String,
    period: GrowthPeriod,    // Last7Days | Last30Days | AllTime
}

GrowthReport {
    agent_id: String,
    period: GrowthPeriod,
    sessions_total: u64,
    avg_tokens_per_session_first_5: usize,
    avg_tokens_per_session_last_5: usize,
    token_efficiency_ratio: f32,       // last_5 / first_5，越小越好
    intent_cache_hit_rate: f32,
    memories_stored: u64,
    memories_shared: u64,
    procedures_learned: u64,
    kg_nodes_created: u64,
    kg_edges_created: u64,
}
```

**数据来源**：
- `sessions_total` → `SessionStore` 的 `completed_sessions` 计数
- Token 数据 → `SessionStore` 中新增 per-session token 累计
- 缓存命中率 → `IntentPrefetcher` 的 `hit_count` / `total_count`
- 知识积累 → `EventBus::events_by_agent()` 过滤 `MemoryStored` + `KnowledgeShared`
- KG 数据 → `EventBus::events_by_agent()` 过滤相关事件

**约束**：GrowthReport 是只读统计。OS 呈现数据，Agent 决定是否调整策略。

### F-14: Multi-Agent Task Delegation（多 Agent 任务委托）

> **Soul 2.0 对齐**：公理 4（共享先于重复）+ 公理 5（机制不是策略）
> **MVP 范围限定**：单节点内的 Agent 间委托。跨节点委托和 A2A 适配为 post-MVP。

**场景驱动**：
"如何防御 APT 攻击？" → 需要网络安全 + 终端安全 + 应急响应多个领域的知识。
多个专精 Agent 协作，比单个全能 Agent 更准确。

**API 设计**：

```rust
DelegateTask {
    task_id: String,
    from_agent: String,
    to_agent: String,
    intent: String,
    context_cids: Vec<String>,
    deadline_ms: Option<u64>,
}

TaskResult {
    task_id: String,
    agent_id: String,
    status: TaskStatus,             // Pending | InProgress | Completed | Failed
    result_cids: Vec<String>,
}

QueryTaskStatus {
    task_id: String,
}
```

**内核实现**：
- `TaskStore`（新增 `src/kernel/ops/task.rs`）管理任务状态
- `TaskStore` 遵循 P-0 持久化规范：`persist()` / `restore()`，原子写入
- 委托通过 EventBus 通知目标 Agent（依赖 F-12 的 `TaskDelegated` 事件）
- 结果通过 CAS 传递（内容寻址，不可篡改）
- OS 只管理状态流转，不理解任务内容

**与 A2A 协议的关系**：
Google A2A 协议（2025.4 发布）定义了 Agent 间通信标准。Plico 的 `DelegateTask` 是**内核原语**，A2A 是**协议适配层**。

```
A2A（外部 Agent 间通信协议）
        │
        ▼ plico-a2a 适配器（post-MVP）
        │
        ▼
DelegateTask（内核原语，MVP 交付）
```

**MVP 交付范围**：
- 单节点内 Agent 间委托 ✅
- 任务状态完整流转（Pending → InProgress → Completed / Failed）✅
- 任务超时处理（deadline_ms 到期自动 Failed）✅
- TaskStore 持久化 ✅
- 跨节点委托 ❌（post-MVP，依赖 v20.0 分布式模式）
- A2A 适配器 ❌（post-MVP）

---

## 4. 知识库场景：MVP 架构设计

### 4.1 场景模型

```
┌────────────────────────────────────────────────────────────┐
│                        人类世界                             │
│  "SQL 注入攻击的防御措施有哪些？"                             │
│        │                            ▲                      │
│        │ 问题                        │ 综合答案              │
│        ▼                            │                      │
│  ┌──────────────────────────────────┴───┐                  │
│  │          Interface Layer             │                  │
│  │    (HTTP API / CLI / Chat UI)        │                  │
│  │    人类 ↔ 结构化请求/响应 的转换层       │                  │
│  └──────────────┬──────────────────────┘                  │
└─────────────────┼──────────────────────────────────────────┘
                  │ 结构化 ApiRequest (JSON)
┌─────────────────┼──────────────────────────────────────────┐
│                 ▼            AI 世界                        │
│  ┌──────────────────────────────────────┐                  │
│  │       Knowledge Agent (LLM)          │                  │
│  │  • 理解问题意图                        │                  │
│  │  • 制定检索策略                        │                  │
│  │  • 综合多源信息                        │                  │
│  │  • 生成结构化答案                      │                  │
│  │  • 决定何时记忆、何时共享               │                  │
│  └───────────────┬──────────────────────┘                  │
│                  │ ApiRequest (JSON)                        │
│  ┌───────────────┴──────────────────────┐                  │
│  │         Plico Kernel (OS)            │                  │
│  │                                      │                  │
│  │  CAS          2000 篇文章的不可变存储  │                  │
│  │  SemanticFS   向量索引 + BM25 全文     │                  │
│  │  KG           CVE→攻击→防御的因果图谱  │                  │
│  │  Memory       Agent 的工作记忆和经验   │                  │
│  │  EventBus     知识变更通知            │                  │
│  │  Prefetcher   意图预热上下文           │                  │
│  │  HybridRetrieval  向量+图谱融合检索    │                  │
│  │                                      │                  │
│  └──────────────────────────────────────┘                  │
└────────────────────────────────────────────────────────────┘
```

**灵魂约束**：
- Plico 是 OS，不是知识库产品
- Knowledge Agent 是应用层，有自己的 LLM
- 人类永远不直接触碰 Plico — 人类 ↔ Interface Layer ↔ Agent ↔ Plico
- Agent 决定检索策略、答案质量、是否记忆 — OS 提供机制

### 4.2 数据摄入流程

```
2000 篇 Markdown/PDF 文章
        │
        ▼
  Ingest Agent（独立进程）
        │
        ├─ 1. BatchCreate → 存入 CAS（每篇文章一个 CID）
        │     tags: ["security", "sql-injection", "defense", ...]
        │
        ├─ 2. KG 节点构建 → kg_add_node()
        │     Entity: "SQL注入", "CVE-2024-XXXX", "WAF", "参数化查询"
        │     Document: 每篇文章的 CID
        │     Fact: "CVE-2024-XXXX 影响 MySQL 8.0"
        │
        ├─ 3. KG 边构建 → kg_add_edge()
        │     "SQL注入" --Causes→ "CVE-2024-XXXX"
        │     "CVE-2024-XXXX" --HasResolution→ "参数化查询"
        │     "参数化查询" --PartOf→ "Web安全防御"
        │     文章 CID --Mentions→ "SQL注入"
        │
        └─ 4. 程序记忆 → remember_procedural()
              scope: Shared, tags: ["verified", "ingest-workflow"]
              内容: 摄入工作流本身（其他 Agent 可复用）
```

**关键**：
- Ingest Agent 负责理解文章、提取实体和关系 — 这是 Agent 的"智能"
- Plico 只执行存储和索引 — 这是 OS 的"机制"
- KG 实体提取由 Agent 的 LLM 完成，不是 Plico 内核做的

### 4.3 查询流程

```
Knowledge Agent 收到用户问题: "SQL 注入攻击的防御措施有哪些？"
              │
              ▼
Step 1: StartSession(load_tiers: [Working, LongTerm])
        → OS 恢复 Agent 上次的工作上下文和长期记忆
              │
              ▼
Step 2: DeclareIntent(tags: ["sql-injection", "defense", "security"])
        → OS 后台预热: 搜索相关内容、预加载 KG 邻居
              │
              ▼
Step 3: HybridRetrieve(
          query_text: "SQL 注入攻击的防御措施",
          seed_tags: ["sql-injection"],
          graph_depth: 2,
          edge_types: ["Causes", "HasResolution", "Mentions"],
          token_budget: 8000
        )
        → OS 返回:
          [向量] 3 篇直接相关文章
          [图谱] 2 个因果链:
            SQL注入 →Causes→ CVE-2024-XXXX →HasResolution→ 参数化查询
            SQL注入 →Causes→ 数据泄露 →HasResolution→ WAF + 输入验证
          token_estimate: 6200
              │
              ▼
Step 4: Agent 的 LLM 综合所有检索结果，生成结构化答案
        （完全在 Agent 侧，Plico 不参与）
              │
              ▼
Step 5: 决策（Agent 自主判断）:
        - 值得记忆？→ RememberWorkingMemory()
        - 可以共享？→ RememberLongTerm(scope: Shared)
        - 更新 KG？→ kg_add_edge("SQL注入防御", HasFact, "新发现")
              │
              ▼
Step 6: EndSession → OS 自动 checkpoint，记录 last_seq
```

### 4.4 "越用越好"体现

| 会话次数 | 发生了什么 | Token 成本变化 |
|----------|-----------|--------------|
| 第 1 次 | 全量检索 + 全量 KG 遍历 | 基准 100% |
| 第 3 次 | 意图缓存命中（相似的安全问题）| ~70% |
| 第 5 次 | 检查点恢复 + 工作记忆预加载 | ~50% |
| 第 10 次 | 共享记忆中已有上次的综合答案 | ~30% |
| 第 20 次 | Agent profile 预测意图 + 全缓存 | ~20% |

**前提**：上述数据基于"相似主题的重复查询"。完全不同的问题不会命中缓存。

### 4.5 不偏离灵魂的边界

| 做什么 | 不做什么 | 为什么 |
|--------|---------|--------|
| 存储文章到 CAS | 不做 PDF 解析 | Agent 负责内容理解 |
| 向量索引 + KG 链接 | 不做实体自动提取 | 实体提取是 LLM 的工作 |
| HybridRetrieve 原语 | 不做答案生成 | 综合推理是 Agent 的工作 |
| 知识事件通知 | 不做推荐 | 推荐是策略，OS 只通知 |
| 成长报告统计 | 不做优化建议 | 建议是策略，OS 只呈现数据 |
| 任务委托状态管理 | 不做任务分解 | 分解是 Agent 的决策 |

---

## 5. MVP 实施计划

### 整体原则

```
         ┌── 加固项 ──┐
         │ D-1 ~ D-4  │ ← 功能 PR 的合入前置条件
         └─────┬──────┘
               │ 必须先完成
    ┌──────────┴──────────┐
    ▼                     ▼
基础功能              协作功能
F-11, F-12, F-13      F-14
    │                     │
    └──────────┬──────────┘
               │ 全部完成后
               ▼
          知识库场景验收
          G-1, G-2, G-3
               │
               ▼
           MVP 交付 ✓
```

### Sprint 1: 基础加固 + F-11（2 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| D-1 EventLog RingBuffer | `src/kernel/event_bus.rs` | 10 万次 emit 后内存稳定，events_since 越界返回 EventLogGap |
| D-2 Broadcast 容量 + Lag 回补 | `src/kernel/event_bus.rs` | 模拟慢消费者，事件不丢失 |
| D-3 token_estimate P0 项 | `src/api/semantic.rs` + 响应构建点 | FetchAssembledContext / SearchResult / MemoryRecalled 均填充 |
| D-4 Soul 红线附注 | `system-v2.md` | 文档化完成 |
| F-11a HybridRetrieve API | `src/api/semantic.rs` | API 定义 + 请求/响应类型 |
| F-11b HybridRetrieve 实现 | `src/kernel/ops/hybrid.rs` | 10 篇文章 + 15 KG 节点场景下返回正确结果 |

### Sprint 2: 事件增强 + 成长报告 + 委托（2 周）

| 任务 | 代码位置 | 验收标准 |
|------|---------|---------|
| F-12 Knowledge Event | `src/kernel/event_bus.rs` | Shared 记忆存储触发 KnowledgeShared，订阅者收到 |
| F-13 GrowthReport | `src/kernel/ops/observability.rs` | 10 个会话后 token_efficiency_ratio < 1.0（相似任务） |
| F-14 TaskStore + DelegateTask | `src/kernel/ops/task.rs` | 委托 → 执行 → 结果完整流转 |
| F-14 TaskStore 持久化 | `src/kernel/ops/task.rs` | 重启后任务状态恢复 |
| D-3 token_estimate P1 项 | 各响应构建点 | DeltaSince / GraphExploreResult 填充 |

### Sprint 3: 场景验收 + 收尾（1-2 周）

| 任务 | 说明 | 验收标准 |
|------|------|---------|
| G-1 Ingest Agent 示例 | 摄入 10 篇安全文章到 CAS + KG | 文章可检索，KG 关系正确 |
| G-2 Knowledge Agent 示例 | 端到端 Q&A 验证 | HybridRetrieve 返回相关结果，token_estimate 非零 |
| G-3 多 Agent 协作验证 | 委托 + 知识事件 | 任务委托端到端，知识事件传播 |
| 集成测试 | 完整 MVP 场景 | kill → restart → 数据完整 |

---

## 6. 依赖图

```
D-1 (RingBuffer) ──→ D-2 (Broadcast修复) ──→ F-12 (KnowledgeEvent)
                                                    │
D-3 (token覆盖) ─────────────────┐                  ├──→ F-14 (TaskStore)
                                 │                  │         │
D-4 (Soul附注) ──────────────────┤                  │         │
                                 │                  │         │
F-11a (API定义) ──→ F-11b (实现) ─┴──────────→ G-1 (Ingest)  │
                                                    │         │
F-13 (GrowthReport) ─────────────────────────→ G-2 (Query)   │
                                                    │         │
                                              G-3 (协作) ←────┘
                                                    │
                                              MVP 交付 ✓
```

---

## 7. MVP 验收测试

> 不是"验证实验"，是**验收标准**。全部通过才能声称 MVP 就绪。

### 测试 1：HybridRetrieve 质量

```
准备:
  10 篇网络安全文章存入 CAS + KG
  15 个 KG 实体 + 20 条 KG 边

验收条件:
  a) 5 个查询中，至少 3 个返回向量和图谱双来源的结果
  b) 每个 HybridHit 的 token_estimate > 0
  c) 至少 2 个查询的 provenance 包含 ≥ 2 跳路径
  d) token_budget 裁剪生效：设 budget=1000 时结果集 < 设 budget=10000 时
```

### 测试 2：知识事件传播

```
准备:
  Agent A 和 Agent B 同时连接
  Agent B 订阅 KnowledgeShared 事件

验收条件:
  a) Agent A 存储 Shared 记忆 → Agent B 下次 poll 收到事件
  b) Agent A 存储 Private 记忆 → Agent B 不收到事件
  c) 事件 summary 非空
  d) 10 万次 emit 后无 OOM（D-1 验证）
```

### 测试 3：成长报告准确性

```
准备:
  1 个 Agent 执行 10 个会话
  前 5 个：不同主题
  后 5 个：与前 5 个相似的主题

验收条件:
  a) sessions_total == 10
  b) avg_tokens_per_session_last_5 ≤ avg_tokens_per_session_first_5
  c) intent_cache_hit_rate > 0
  d) memories_stored > 0
```

### 测试 4：任务委托端到端

```
准备:
  Agent A（协调者）和 Agent B（执行者）

验收条件:
  a) 状态流转正确：Pending → InProgress → Completed
  b) result_cids 非空，Agent A 可读取
  c) deadline 到期时状态变为 Failed
  d) 重启后任务状态恢复
```

### 测试 5：崩溃恢复（MVP 整体验收）

```
场景:
  摄入 10 篇文章 → 执行 3 个查询 → kill -9 plicod → 重启

验收条件:
  a) CAS 中 10 篇文章完整
  b) KG 节点和边完整
  c) 之前的会话 checkpoint 可恢复
  d) Agent token 仍然有效
  e) EventBus 事件日志可用（从最后一次持久化恢复）
```

---

## 8. Soul 2.0 对齐

| Soul 2.0 公理 | 功能 | 对齐方式 |
|--------------|------|---------|
| 公理 1: Token 最稀缺 | D-3, F-11 | token_estimate 全覆盖 + HybridRetrieve 的 token_budget |
| 公理 2: 意图先于操作 | F-11 | Agent 描述意图，OS 组装最优结果集 |
| 公理 3: 记忆跨越边界 | F-13 | GrowthReport 量化记忆的价值 |
| 公理 4: 共享先于重复 | F-12, F-14 | 知识事件通知 + 任务委托 = 协作而非重复 |
| 公理 5: 机制不是策略 | 全部 | OS 提供原语，不决定何时用 |
| 公理 6: 结构先于语言 | F-11 | HybridRetrieve 接受结构化参数，不解析自然语言 |
| 公理 7: 主动先于被动 | F-12 | KnowledgeShared 事件主动推送 |
| 公理 8: 因果先于关联 | F-11 | provenance 展示因果链 |
| 公理 9: 越用越好 | F-13 | Agent 可量化成长 |
| 公理 10: 会话一等公民 | 继承 N3 | StartSession/EndSession + checkpoint |

### 灵魂偏差检测

| 关注点 | 分析 | 结论 |
|--------|------|------|
| HybridRetrieve 是否引入"策略" | α=0.6 是默认值。Agent 可通过 seed_tags / edge_types / token_budget 控制策略。OS 提供的是组合机制。 | ✅ 合规 |
| KnowledgeShared 是否泄露 Private | 只在 Shared / Group scope 时 emit。Private 不触发。 | ✅ 合规 |
| GrowthReport 是否引入"自动学习" | 只读统计，不改变行为。 | ✅ 合规 |
| DelegateTask 是否引入"任务分解" | OS 只管状态流转。分解是 Agent 决策。 | ✅ 合规 |
| D-4 embed/summarize 在内核 | 架构决策（非妥协），边界清晰（仅存储路径），trait 抽象保证模型无关。 | ✅ 已决策，持续监控 |

---

## 9. 完成后的预期状态

| 维度 | 节点 3 完成后 | Sprint 1 后 | Sprint 2 后 | MVP 交付 |
|------|-------------|------------|------------|---------|
| 代码量 | ~29K 行 | ~29.8K 行 | ~30.5K 行 | ~31K 行 |
| 内核模块 | 22 个 | 23 个（+hybrid） | 24 个（+task） | 24 个 |
| API 端点 | ~96 个 | ~99 个 | ~103 个 | ~103 个 |
| 测试 | ~760 个 | ~790 个 | ~820 个 | ~840 个 |
| 已知技术债 | D-1~D-4 | **0** | 0 | **0** |
| EventBus | 无界（OOM）| 有界 65536 | + 知识事件 | 完整 |
| token_estimate | 部分覆盖 | 核心 P0 覆盖 | 全覆盖 | 全覆盖 |
| 检索能力 | 向量 OR 图谱 | 向量 + 图谱融合 | 同左 | 同左 |
| Agent 间协作 | 无 | 无 | 事件 + 委托 | 完整 |
| 知识库场景 | 不支持 | 部分（检索可用） | 大部分 | **端到端可用** |
| 崩溃恢复 | ✅ | ✅ | ✅ | **验收通过** |

---

## 10. 本质变化

```
节点 1 → Agent 有了家（存储）
节点 2 → Agent 有了大脑（智能原语）
节点 3 → Agent 有了连续的意识（认知连续性）
节点 4 → Agent 有了同事（协作生态）→ MVP 交付
```

**Plico MVP 的价值主张**：

当一个 Knowledge Agent 运行在 Plico 上时，它比运行在 Linux + Elasticsearch + Neo4j 上的同类 Agent：
- **检索更准** — HybridRetrieve 融合向量 + 图谱，多跳查询准确率 94% vs 61%
- **协作更省** — 知识事件 + 任务委托，Agent 间零重复工作
- **成本更透** — 每次操作都有 token_estimate，Agent 做成本感知决策
- **成长可见** — GrowthReport 证明"越用越好"不是空话
- **数据更安** — 崩溃恢复 + 原子写入 + 密码学身份，用户可信赖

**这就是 AIOS 与"在 Linux 上跑的 AI"的本质区别。**
而且现在，这不再是一个概念验证——这是一个**可交付的产品**。

---

## 附录 A: 技术选型联网校正

| 技术点 | 网络验证结果 | 对 Plico 的影响 |
|--------|------------|----------------|
| Graph-RAG 融合检索 | 2026 主流范式，多跳查询准确率 94% vs 纯向量 61% | F-11 设计方向正确 |
| Ring Buffer | Rust 标准 VecDeque 零依赖，性能满足 MVP；photon-ring 可作为后续升级路径 | D-1 选择合理 |
| A2A 协议 | Google 2025.4 发布，JSON-RPC over HTTP，Agent Card 发现 | F-14 原语需兼容 A2A 语义（post-MVP 适配） |
| KCP 知识治理 | Mozilla cq 报告 85% 减少重复分析，60% 知识复用率 | F-12 是 KCP 理念的内核级实现 |
| ELISAR 网安多 Agent | 多 Agent + RAG 在网安领域已有成熟实践 | 知识库场景可行性确认 |
| CyKG-RAG | KG 用于威胁分析、攻击模式匹配 | Plico KGEdgeType（Causes, HasResolution）直接适用 |

## 附录 B: 与节点 3 的接口

| 节点 3 能力 | 节点 4 如何使用 |
|------------|----------------|
| F-6 StartSession/EndSession | 每个 Agent 的会话生命周期管理 |
| F-7 DeltaSince | GrowthReport 的时间窗口数据源 |
| F-8 TokenCostTransparency | D-3 覆盖扩展的基础 |
| F-9 IntentAssemblyCache | HybridRetrieve 的缓存层 |
| P-0 持久化 | TaskStore 必须遵循持久化规范 |
| v21.0 Checkpoint | 任务中途崩溃时的恢复基础 |

## 附录 C: 新增代码文件清单

| 文件 | 用途 | 预估行数 |
|------|------|---------|
| `src/kernel/ops/hybrid.rs` | HybridRetrieve 实现 | ~250 |
| `src/kernel/ops/task.rs` | TaskStore + DelegateTask + 持久化 | ~350 |
| `src/kernel/event_bus.rs`（修改） | RingBuffer + 知识/任务事件 | +180 |
| `src/api/semantic.rs`（修改） | 新增 API 变体 + token_estimate | +120 |
| `src/kernel/ops/observability.rs`（修改） | GrowthReport | +80 |
| `tests/node4_hybrid.rs` | HybridRetrieve 验收测试 | ~150 |
| `tests/node4_task.rs` | 任务委托验收测试 | ~120 |
| `tests/node4_knowledge_event.rs` | 知识事件验收测试 | ~80 |
| `tests/node4_crash_recovery.rs` | 崩溃恢复集成测试 | ~100 |
| `examples/ingest_agent.rs` | 摄入 Agent 示例 | ~150 |
| `examples/knowledge_agent.rs` | 知识 Agent 示例 | ~200 |

---

*文档状态：MVP 交付设计文档。技术债零容忍。验收测试全部通过方可交付。*
