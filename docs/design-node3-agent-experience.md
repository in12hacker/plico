# Plico 第三节点设计文档
# Agent 体验层——从原语到生命周期

**版本**: v1.0
**日期**: 2026-04-19
**灵魂依据**: `system-v2.md`（Soul 2.0）
**定位**: POC 第三阶段指导性文档

---

## 0. 第二节点回顾

第一节点建了存储层。第二节点建了智能原语。但原语是零件，不是体验。

| 节点 | 主题 | 成果 |
|------|------|------|
| 节点 1 | 存储基础 | CAS + SemanticFS + LayeredMemory + AgentScheduler + EventBus + ToolRegistry |
| 节点 2 | 智能原语 | IntentPrefetcher + AgentToken + edgevec BQ + MemoryScope + SSE + BatchAPI + KG Causal + TierMaintenance + Observability |

**节点 2 验证指标**：
- Token 节省：86.7%（目标 ≥50%）✅
- 工具调用减少：75%（目标 ≥60%）✅
- 时间减少：98.75%（目标 ≥30%）✅

**但这些指标衡量的是单次会话的效率。**

---

## 1. 链式推理：当前缺失什么

```
我是 Cursor。我每天帮人写代码。

昨天我修复了 auth 模块的 bug。
今天同一个人打开新会话，让我继续优化 auth 模块。

在 Linux 上：
  → 我是全新的进程。昨天的我已经死了。
  → 我重新读 CLAUDE.md（已经读过 100 次了）
  → 我重新 glob src/auth（昨天刚做过）
  → 我重新理解模块结构（昨天已经理解了）
  → 代价：和第一天完全一样

在 Plico 第二节点上：
  → DeclareIntent("优化 auth 模块") → FetchAssembledContext()
  → 获得预组装的上下文 → 开始工作
  → 比 Linux 便宜 86%
  → 但：每次新会话都要重新 DeclareIntent
  → 但：不知道昨天到今天代码有没有变
  → 但：昨天学到的经验要主动召回才能用
  → 但：不知道这次操作花了多少 token

缺什么？
  1. 我没有"会话连续性"——每次启动都是冷启动
  2. 我不知道"什么变了"——只能全量重读
  3. 我不知道"花了多少"——无法做成本权衡
  4. 系统不会"越用越快"——第 100 次和第 1 次一样贵
```

**核心洞察**：节点 2 解决了 **单次会话** 的效率。节点 3 要解决 **跨会话** 的效率。
当 Agent 在同一个项目上工作 100 次后，AIOS 应该比 Linux 快不止 10 倍。

---

## 2. 第三节点主题：复合智能

**定义**：系统越用越懂 Agent，Agent 越用越便宜。

| 会话编号 | Linux 成本 | AIOS 成本（节点 3） | 原因 |
|---------|-----------|-------------------|------|
| 第 1 次 | 100% | 100% | 冷启动，无缓存 |
| 第 3 次 | 100% | ~40% | 意图缓存命中，会话恢复 |
| 第 10 次 | 100% | ~15% | 程序记忆丰富，热路径稳定 |
| 第 50 次 | 100% | ~5% | 几乎所有意图缓存命中 |

实现这个目标需要四个功能：

```
F-6: Agent 会话生命周期
  → 解决"每次冷启动"问题
  → Soul 2.0 公理 10（会话是一等公民）

F-7: 增量上下文更新
  → 解决"不知道什么变了"问题
  → Soul 2.0 公理 1（token 是最稀缺资源）

F-8: Token 成本透明
  → 解决"不知道花了多少"问题
  → Soul 2.0 公理 1 推论

F-9: 意图缓存与热路径
  → 解决"不会越用越快"问题
  → Soul 2.0 公理 9（越用越好）
```

---

## 3. F-6：Agent 会话生命周期

### 3.1 问题定义

当前 Agent 与 Plico 的交互是无状态的请求-响应。每个 `ApiRequest` 是独立的。
Agent 没有"连接"、"在线"、"离线"的概念。

```
当前交互模式：
  Agent → ApiRequest → Kernel → ApiResponse → Agent
  Agent → ApiRequest → Kernel → ApiResponse → Agent
  （每个请求独立，无会话概念）

缺失的生命周期：
  Agent 连接 → 会话开始 → 恢复上次状态 → 工作 → 保存状态 → 会话结束
```

### 3.2 设计

**新增 API**：

```rust
ApiRequest::StartSession {
    agent_id: String,
    agent_token: Option<String>,
    /// 恢复上一个会话的检查点（如果有）
    resume_checkpoint: Option<String>,
    /// 本次会话的意图提示（可选，触发预热）
    intent_hint: Option<String>,
    /// 请求 OS 加载的记忆层
    load_tiers: Vec<MemoryTier>,
}

ApiResponse::SessionStarted {
    session_id: String,
    agent_id: String,
    /// 如果提供了 intent_hint，返回预组装的上下文
    warm_context: Option<BudgetAllocation>,
    /// 自动加载的近期记忆（MemoryEntry 的 L0 摘要投影，节省 token）
    recent_memories: Vec<MemoryEntrySummary>,  // 新增 DTO
    /// 上次会话结束后发生的变更摘要
    changes_since_last: Vec<ChangeSummary>,    // 新增 DTO，复用 ChangeEntry 格式
    /// 上次会话保存的检查点数据
    checkpoint_data: Option<serde_json::Value>,
}

ApiRequest::EndSession {
    agent_id: String,
    session_id: String,
    /// 保存工作状态，下次 StartSession 时可恢复
    save_checkpoint: Option<serde_json::Value>,
}
```

### 3.3 内部状态

```rust
struct AgentSession {
    session_id: String,
    agent_id: String,
    started_at_ms: u64,
    last_active_ms: u64,
    /// 本会话累计 token 消耗估算
    token_usage: AtomicU64,
    /// 本会话访问过的 CID 集合（用于 delta 追踪）
    accessed_cids: RwLock<HashSet<String>>,
    /// 本会话触发过的意图（用于缓存预热）
    declared_intents: RwLock<Vec<String>>,
}

struct SessionStore {
    /// 活跃会话
    active: RwLock<HashMap<String, AgentSession>>,
    /// 会话检查点（持久化到 CAS）
    checkpoints: RwLock<HashMap<String, String>>, // session_id → checkpoint CID
    /// 每个 Agent 的上次会话结束时间（用于 delta 计算）
    last_session_end: RwLock<HashMap<String, u64>>,
}
```

### 3.4 StartSession 执行流程

```
Agent 调用 StartSession(agent_id="cursor-1", intent_hint="优化 auth 模块")

Step 1: 验证身份（AgentKeyStore）
Step 2: 查找上次会话结束时间 → last_end_ms
Step 3: 收集变更摘要 → event_log.since(last_end_ms, tags=["auth"])
Step 4: 加载近期记忆 → memory.recall(agent_id, tiers=[Working, Procedural], limit=20)
Step 5: 如果有 intent_hint → 触发 DeclareIntent（复用现有预取引擎）
Step 6: 如果有 resume_checkpoint → 从 CAS 加载检查点数据
Step 7: 创建 AgentSession，返回 SessionStarted
```

**关键价值**：一次 `StartSession` 调用 = 过去需要 5-10 次独立 API 调用。
Agent 一开口就拿到上下文、记忆、变更摘要、预热的意图装配。

### 3.5 对 Cursor 的具体价值

```
第 1 天：Cursor 修复 auth bug
  → StartSession(intent_hint="修复 auth bug")
  → 冷启动，无缓存（和节点 2 一样）
  → EndSession(save_checkpoint={"focus": "auth", "files_touched": [...]})

第 2 天：Cursor 继续优化 auth
  → StartSession(intent_hint="优化 auth 模块", resume_checkpoint="昨天的 ID")
  → OS 返回：
    - warm_context: 预组装的 auth 相关上下文（意图缓存命中！）
    - recent_memories: 昨天存的工作记忆和程序记忆
    - changes_since_last: "src/api/agent_auth.rs 被另一个 Agent 修改了"
    - checkpoint_data: {"focus": "auth", "files_touched": [...]}
  → Cursor 直接开始工作，零导航开销
```

### 3.6 实现范围

- **新增文件**: `src/kernel/ops/session.rs`（~250 行）
- **修改文件**: `src/api/semantic.rs`（新增 2 个 ApiRequest 变体）、`src/kernel/mod.rs`（路由）
- **依赖**: 现有 EventBus、IntentPrefetcher、LayeredMemory
- **风险**: 低（additive，不修改现有 API）

---

## 4. F-7：增量上下文更新（Delta API）

### 4.1 问题定义

```
当前模式：
  Agent 请求 → OS 返回完整对象
  Agent 下次请求 → OS 再次返回完整对象（哪怕只改了一行）

问题：
  一个 10KB 的文件改了 1 行 → Agent 重新消费 10KB token
  10 个文件每次会话重读 → 100KB token 浪费
```

一个 AI Agent 在同一项目上迭代工作时，大部分上下文 **没有变化**。
OS 应该只告诉 Agent **什么变了**，而不是重发全量。

### 4.2 设计

**新增 API**：

```rust
ApiRequest::DeltaSince {
    agent_id: String,
    /// 上次已知时间点
    since_ms: u64,
    /// 关注的对象列表（如果为空，返回所有变更）
    watch_cids: Vec<String>,
    /// 关注的标签（如果为空，不按标签过滤）
    watch_tags: Vec<String>,
    /// 返回的最大变更数
    limit: Option<usize>,
}

ApiResponse::DeltaResult {
    changes: Vec<ChangeEntry>,
    /// 本响应覆盖的时间范围
    from_ms: u64,
    to_ms: u64,
    token_estimate: usize,
}

struct ChangeEntry {
    cid: String,
    change_type: ChangeType,  // Created | Modified | Deleted | TagsChanged
    /// L0 摘要（~100 token）——Agent 可以决定是否需要看 L1/L2
    summary: String,
    changed_at_ms: u64,
    changed_by: String,       // 哪个 Agent 做的变更
}
```

### 4.3 与会话集成

`DeltaSince` 可以独立使用，也可以与 F-6 的 `StartSession` 集成：

```
StartSession 内部自动调用 DeltaSince(since_ms=上次会话结束时间)
→ 返回 changes_since_last 字段
→ Agent 立刻知道"自从上次，什么变了"
```

对于长期运行的会话（通过 SSE），OS 可以 **推送** delta 事件：

```rust
// 通过 plico-sse 推送
ApiRequest::SubscribeDeltas {
    agent_id: String,
    session_id: String,
    watch_cids: Vec<String>,
    watch_tags: Vec<String>,
}
// OS 通过 SSE 推送 ChangeEntry 事件
```

### 4.4 Token 节省估算

```
场景：Agent 跟踪 50 个文件，平均每个 5KB

全量重读：50 × 5KB = 250KB ≈ 62,500 token
Delta（假设 5 个文件有变更）：5 × 100 token (L0 摘要) = 500 token

节省率：99.2%
```

### 4.5 实现范围

- **新增文件**: `src/kernel/ops/delta.rs`（~150 行）
- **修改文件**: `src/api/semantic.rs`（新增 ApiRequest 变体）、`src/kernel/mod.rs`（路由）
- **依赖**: CAS（timestamp tracking）、EventBus（变更事件）
- **基础**: CAS 已有 `created_at` 时间戳，EventBus 已有事件日志

---

## 5. F-8：Token 成本透明

### 5.1 问题定义

```
Agent 视角：
  我调用了 FetchAssembledContext()，返回了一大段文本。
  这段文本花了我多少 token？我不知道。
  我应该请求 L0 还是 L1？我没有成本依据。
  这个会话累计花了多少 token？我不知道。
```

Soul 2.0 公理 1 说"token 是最稀缺资源"。
但如果 Agent 看不到成本，它就无法做成本优化。

### 5.2 设计

**在 ApiResponse 中追加字段**：

```rust
pub struct ApiResponse {
    // ... 现有字段 ...

    /// 本响应中返回内容的 token 估算
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<usize>,

    /// 本会话累计 token 消耗
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token_total: Option<usize>,
}
```

**Token 估算算法**：

```rust
fn estimate_tokens(text: &str) -> usize {
    // 粗粒度但足够准确的估算：
    // - ASCII/代码：~4 字符/token
    // - 中文/日文：~2 字符/token
    // - 混合内容：取加权平均
    let ascii_chars = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii_chars = text.chars().filter(|c| !c.is_ascii()).count();
    (ascii_chars / 4) + (non_ascii_chars / 2) + 1
}
```

**新增查询 API**：

```rust
ApiRequest::QueryTokenUsage {
    agent_id: String,
    session_id: Option<String>,  // 如果为空，返回总量
    since_ms: Option<u64>,       // 如果指定，只统计该时间之后的
}

ApiResponse::TokenUsage {
    agent_id: String,
    session_id: Option<String>,
    total_tokens: usize,
    breakdown: TokenBreakdown,
}

struct TokenBreakdown {
    context_assembly: usize,   // DeclareIntent/FetchAssembledContext
    search_results: usize,     // Search 返回
    object_reads: usize,       // Read 返回
    memory_recalls: usize,     // Recall 返回
    graph_queries: usize,      // KG 查询返回
    delta_updates: usize,      // DeltaSince 返回
}
```

### 5.3 对 Agent 决策的影响

```
Agent 收到 FetchAssembledContext 响应，token_estimate=3200

Agent 内部决策：
  if token_estimate > budget * 0.3:
    # 上下文太大，请求 L0 版本
    FetchAssembledContext(granularity="L0")
  else:
    # 在预算内，使用完整上下文
    proceed()
```

Token 成本透明让 Agent 从"盲目消费"变为"精打细算"。

### 5.4 实现范围

- **修改文件**: `src/api/semantic.rs`（ApiResponse 新增字段）、`src/kernel/mod.rs`（在响应中注入 token 估算）
- **新增**: `src/kernel/ops/session.rs` 中的 token 累计逻辑
- **代码量**: ~80 行
- **风险**: 极低（只是在响应中追加可选字段）

---

## 6. F-9：意图缓存与热路径

### 6.1 问题定义

```
DeclareIntent 的当前工作方式：
  1. 接收意图文本
  2. 生成 embedding
  3. 4 路并发召回（语义 + KG + 记忆 + 事件）
  4. RRF 融合
  5. 分层压缩

即使 Agent 昨天声明过完全相同的意图，今天仍然从头执行这 5 步。
```

大部分 Agent 的工作是 **迭代性** 的——在同一个领域反复工作。
相似的意图应该能命中缓存，跳过昂贵的多路召回。

### 6.2 设计

**意图缓存（内部数据结构，不暴露 API）**：

```rust
struct IntentCache {
    entries: RwLock<Vec<IntentCacheEntry>>,
    max_entries: usize,        // 默认 1000
    ttl_ms: u64,               // 默认 24 小时
    similarity_threshold: f32, // 默认 0.85
}

struct IntentCacheEntry {
    intent_text: String,
    intent_embedding: Vec<f32>,
    assembly: BudgetAllocation,
    agent_id: String,
    created_at_ms: u64,
    hit_count: AtomicU64,
    /// 缓存条目依赖的 CID 列表——如果这些对象被修改，缓存失效
    dependency_cids: Vec<String>,
}
```

### 6.3 缓存查找流程

```
DeclareIntent("优化 auth 模块的错误处理")

Step 1: 生成 intent embedding

Step 2: 在 IntentCache 中搜索
  → 余弦相似度 > 0.85 的缓存条目
  → 检查依赖 CID 是否被修改过（EventBus 变更追踪）
  → 如果找到 → 缓存命中（cache hit）

Step 3a (cache hit):
  → 返回缓存的 assembly，标记 hit_count + 1
  → 跳过 4 路召回 + RRF 融合
  → 延迟：<1ms（vs 正常路径 50-500ms）

Step 3b (cache miss):
  → 正常执行 4 路召回 + RRF 融合
  → 将结果存入缓存
  → 记录依赖的 CID 列表（用于失效判断）
```

### 6.4 缓存失效策略

```
缓存条目失效条件（满足任一）：
  1. TTL 过期（默认 24 小时）
  2. 依赖的 CID 被修改（通过 EventBus 监听）
  3. 缓存容量满时 LRU 淘汰

这确保：
  - 缓存不返回过期数据
  - 代码变更后自动刷新上下文
  - 内存使用受控
```

### 6.5 Agent Profile 与热路径

在意图缓存的基础上，OS 可以构建 **Agent Profile**——
记录每个 Agent 最常用的意图模式和访问模式：

```rust
struct AgentProfile {
    agent_id: String,
    /// 最常声明的意图（按 hit_count 排序）
    frequent_intents: Vec<(String, u64)>,
    /// 最常访问的 CID（按访问频率排序）
    hot_objects: Vec<(String, u64)>,
    /// 最常使用的工具
    preferred_tools: Vec<(String, u64)>,
    updated_at_ms: u64,
}
```

Agent Profile 不做决策（公理 5：机制不是策略），只记录统计数据。
Agent 可以查询自己的 profile 来优化自己的行为。

### 6.6 实现范围

- **修改文件**: `src/kernel/ops/prefetch.rs`（在 `declare_intent` 中增加缓存查找）
- **新增**: `IntentCache` 结构（~150 行，在 prefetch.rs 中）
- **新增**: `AgentProfile`（~80 行，在 `src/kernel/ops/session.rs` 中）
- **依赖**: EventBus（变更追踪用于缓存失效）
- **风险**: 低（缓存是加速层，miss 时回退到原始路径）

---

## 7. 第三节点实现路线图

### 7.1 优先级与依赖关系

```
F-8（Token 透明）       ← 最简单，先做，所有后续功能都受益
│
F-6（会话生命周期）      ← 依赖 F-8（会话级 token 统计）
│
├─ F-7（增量更新）       ← 依赖 F-6（DeltaSince 需要 session 跟踪）
│
└─ F-9（意图缓存）       ← 依赖 F-6（Agent Profile 需要 session 数据）
```

### 7.2 分阶段里程碑

**Phase A（第一轮，~1 周）**
- [ ] F-8：Token 估算函数 + ApiResponse 字段
- [ ] F-6 基础：StartSession / EndSession API + SessionStore

**Phase B（第二轮，~2 周）**
- [ ] F-6 完善：检查点持久化 + 会话恢复 + 变更摘要
- [ ] F-7：DeltaSince API + 事件日志时间范围查询
- [ ] F-9 基础：IntentCache 数据结构 + 缓存查找

**Phase C（第三轮，~2 周）**
- [ ] F-9 完善：缓存失效（EventBus 监听）+ AgentProfile
- [ ] F-7 推送：SubscribeDeltas（通过 plico-sse 推送）
- [ ] 验证实验：10 次迭代会话的 token 消耗曲线

**Phase D（验证与调优，~1 周）**
- [ ] 自动化基准测试：复合智能衰减曲线
- [ ] 缓存命中率调优（similarity_threshold、TTL）
- [ ] 文档更新：AGENTS.md + README 对齐

---

## 8. 验证实验设计

### 8.1 复合智能衰减曲线

**场景**："同一个 Agent 在同一个项目上连续工作 10 个会话"

```
测试流程：
  for session in 1..=10:
    token_count = 0

    StartSession(intent_hint="修复 ${module} 的 ${bug}")
    → 记录返回的 token_estimate

    FetchAssembledContext()
    → 记录返回的 token_estimate

    // 模拟工作
    Search("相关代码") → 记录 token
    Read(cid) → 记录 token

    EndSession(save_checkpoint=...)
    → 记录 session_token_total

    record(session, token_count)

绘制曲线：X=会话编号 Y=token 消耗
期望：递减曲线，session 10 < session 1 的 20%
```

### 8.2 意图缓存命中率

```
准备 50 个意图文本，分为 5 个主题（每主题 10 个变体）

for intent in intents:
    DeclareIntent(intent)
    记录：cache_hit / cache_miss / assembly_latency

期望：
  - 每主题的第 1 个：miss（冷启动）
  - 每主题的第 2-10 个：hit（语义相似度 > 0.85）
  - 缓存命中率 > 80%
  - 命中延迟 < 1ms（vs miss 延迟 50-500ms）
```

### 8.3 Delta API 效率

```
在 CAS 中创建 100 个对象
修改其中 5 个

对照组：Read 100 个对象 → 记录 token_estimate
实验组：DeltaSince(since_ms=修改前) → 记录 token_estimate

期望：
  - 对照组：~100 × average_object_size
  - 实验组：5 × L0_summary_size
  - 节省率 > 95%
```

---

## 9. 与 Soul 2.0 的对齐

| Soul 2.0 公理 | 节点 3 对应实现 | 对齐度 |
|--------------|---------------|--------|
| 公理 1：Token 是最稀缺资源 | F-8 Token 透明 + F-7 Delta API | ✅ |
| 公理 2：意图先于操作 | F-9 意图缓存（重复意图零成本） | ✅ |
| 公理 3：记忆跨越边界 | F-6 会话恢复（检查点持久化） | ✅ |
| 公理 5：机制不是策略 | AgentProfile 只记录统计，不做决策 | ✅ |
| 公理 7：主动先于被动 | F-6 StartSession 自动预热 | ✅ |
| 公理 9：越用越好 | F-9 缓存 + Profile（复合智能） | ✅ |
| 公理 10：会话是一等公民 | F-6 完整会话生命周期 | ✅ |

---

## 10. 节点 3 完成后的预期状态

| 指标 | 节点 2 | 节点 3 目标 |
|------|--------|-----------|
| 单次会话 token 节省 | 86.7% | 86.7%（保持） |
| 第 10 次会话 token 消耗 | = 第 1 次 | ≤ 第 1 次的 20% |
| 意图缓存命中率 | 无缓存 | > 80% |
| Delta vs 全量 token 节省 | 无 delta | > 95% |
| 会话冷启动时间 | 无会话概念 | < 200ms |
| Agent 成本可见性 | 无 | 每个响应都带 token 估算 |
| API 变体总数 | ~85 | ~91（+6 个新端点） |

**本质差异**：节点 2 让 Cursor 在 AIOS 上 **一次比 Linux 快**。
节点 3 让 Cursor 在 AIOS 上 **越用越快**。

---

*文档状态：指导性设计文档（Tier B）。实现细节以 Tier A（AGENTS.md + INDEX.md + 代码）为准。*
