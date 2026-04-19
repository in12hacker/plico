# Plico 第三节点设计文档
# 认知连续性——让 AI 永不遗忘

**版本**: v2.2（新增 Phase 0 持久化里程碑 + 合理性审计）
**日期**: 2026-04-19
**灵魂依据**: `system-v2.md`（Soul 2.0）
**定位**: POC 第三阶段指导性设计文档（可直接指导开发）

---

## 0. 地基盘点（v1.0 → v21.0 全量）

27,416 行 Rust 代码。693 个测试。20 个内核操作模块。~90 个 API 端点。

### 已完成的三层能力

**存储层**（节点 1）
CAS + SemanticFS + LayeredMemory + EventBus + ToolRegistry + PermissionGuard

**智能原语层**（节点 2 前半）
IntentPrefetcher + AgentToken + edgevec BQ + MemoryScope + SSE + BatchAPI +
KG CausalReasoning + TierMaintenance + Observability

**基础设施层**（节点 2 后半，v17-v21）
- v17.0 API 版本管理 — 语义版本 + 特性标志 + 废弃通知
- v18.0 模型热切换 — 运行时切换 embedding/LLM 模型
- v19.0 边缘缓存 — embedding/KG/search 三级 LRU 缓存
- v20.0 分布式模式 — 多节点集群 + Gossip 发现 + Agent 迁移
- v21.0 检查点 — AgentCheckpoint 完整认知状态序列化

### 当前 AIKernel 结构体字段

```rust
pub struct AIKernel {
    root, cas, memory, scheduler, fs, permissions, memory_persister,
    embedding: HotSwapEmbeddingProvider,    // v18.0
    llm_provider: HotSwapLlmProvider,       // v18.0
    summarizer, knowledge_graph, search_backend, search_op_count,
    tool_registry, message_bus, event_bus,
    prefetch: IntentPrefetcher,             // v9.0
    key_store: AgentKeyStore,               // v9.0
    tenant_store: TenantStore,              // v10.0
    metrics: KernelMetrics,                 // v14.0
    edge_cache: EdgeCache,                  // v19.0
    cluster: ClusterManager,                // v20.0
}
```

---

## 0.5 设计评审：合理性审计

> **原则**：在开发前识别设计中的不合理之处。标记为 ⚠️ 需调整 或 ✅ 合理可迭代。
> 开发时遇到 ⚠️ 项必须先调整再实现，不能照搬文档。

### 逐项审计

| 功能 | 设计点 | 判定 | 问题 | 调整方案 |
|------|--------|------|------|---------|
| F-7 | `DeltaSince.since_ms` 用时间戳 | ⚠️ 不合理 | `events_since()` 接受的是序列号 `since_seq: u64`，不是时间戳。SequencedEvent 有 `seq` 和 `timestamp_ms` 两个字段，但 API 是 seq-based。 | 改为 `since_seq: u64`，或在 delta.rs 中增加按时间戳查找最近 seq 的辅助函数。推荐前者——Agent 在 EndSession 时记录 last_seq，下次 StartSession 用它。 |
| F-7 | `ChangeEntry.summary` 声称 L0 摘要 | ⚠️ 需降级 | L0 摘要依赖 Summarizer（LLM）。当 Summarizer 未配置时（POC 常态），无法生成 L0。 | summary 降级为事件元数据拼接（event_type + cid 前 8 位 + agent_id + tags），不依赖 LLM。有 Summarizer 时可选增强。 |
| F-9 | 意图缓存用余弦相似度匹配 | ⚠️ 有条件 | 需要 EmbeddingProvider 生成 intent embedding。`StubEmbeddingProvider` 返回零向量，余弦相似度恒为 NaN/0。 | 增加双路匹配：有真实 embedding 时走语义匹配；stub 模式时回退到精确字符串匹配（只匹配完全相同的意图文本）。 |
| F-9 | 缓存 `BudgetAllocation` 在内存 | ⚠️ 需限制 | `BudgetAllocation.items` 包含 `LoadedContext.content: String`（完整文本）。一个装配可能有几十 KB。缓存 1000 条 = 几十 MB 内存。 | 默认 `max_entries` 从 1000 降为 64。增加 `max_memory_bytes` 硬限制（默认 32MB）。超限时 LRU 淘汰。 |
| F-10 | 意图转移矩阵用原始字符串做 key | ⚠️ 不实用 | Agent 意图几乎每次都不同（"修复 auth 的测试" vs "修复 auth 模块的单元测试"）。字符串精确匹配导致转移矩阵极度稀疏，几乎不会命中。 | **降级为 Phase D 探索性功能**。当前阶段用 tag 聚类代替原始字符串（从意图中提取关键 tag 做 key）。或者依赖 F-9 的 embedding 相似度来归并相似意图。 |
| F-10 | 声称"不需要 embedding" | ⚠️ 自相矛盾 | 不用 embedding 就只能精确匹配字符串，这使得 F-10 几乎无用（见上条）。 | 承认 F-10 在 stub embedding 模式下功能退化。有真实 embedding 时，用向量聚类归并相似意图做 key。无 embedding 时，F-10 自动禁用。 |
| F-6 | EndSession 未考虑异常退出 | ⚠️ 需补充 | Agent 崩溃/断连时不会调用 EndSession，session 成为孤儿。 | 增加 session 超时机制：超过 TTL（默认 30 分钟）无活动自动触发 EndSession + checkpoint。session_store 定期扫描。 |
| F-6 | `load_tiers: Vec<String>` | ⚠️ 类型不严谨 | 应该用已有的 `MemoryTier` 枚举而非字符串。 | 改为 `Vec<MemoryTier>` 或至少在 API 层做验证映射。 |
| F-8 | token 估算公式 `(ascii+3)/4 + (non_ascii+1)/2` | ✅ 可迭代 | 粗粒度但足够 POC。对纯代码会偏高（代码 token 密度 > 自然语言），对中文会偏低。 | Phase A 先用此公式。Phase D 可用实际 tokenizer（tiktoken-rs）校准。在 API 文档中标注 "estimate, not precise"。 |
| F-7 | Delta 99.6% 节省率 | ✅ 数学正确但需注明前提 | 仅在"大部分文件未变"的场景下成立。如果 Agent 跨项目工作或项目大规模重构，delta 可能接近全量。 | 在文档中注明"节省率取决于变更比例"。 |
| F-6 | StartSession 编排 | ✅ 合理可迭代 | 底层零件全部已有。编排逻辑清晰，依赖关系明确。 | 按设计实现。 |
| F-8 | 整体设计 | ✅ 合理 | 最简单、风险最低、价值明确。 | 作为 Phase A 第一个实现。 |
| 量化目标 | "第 10 次 ≤ 1:5" | ⚠️ 过于乐观 | 假设所有意图都命中缓存 + 所有检查点完美恢复。现实中 Agent 任务有很大变异性。 | 调整为"相似任务的第 10 次 ≤ 1:3"——限定"相似任务"前提条件。 |
| 量化目标 | "认知预取命中率 > 50%" | ⚠️ 过于乐观 | F-10 的稀疏性问题使 50% 不现实。 | 调整为"重复性工作流的预取命中率 > 30%"。F-10 降级为探索性功能后，此指标是 bonus 而非核心目标。 |
| 全局 | CheckpointStore 无持久化 | 🔴 阻塞 | v21.0 检查点只在内存中，重启全丢。F-6 Session 建在沙子上。 | Phase 0 P-2：CheckpointStore persist/restore，存入 CAS。 |
| 全局 | TenantStore 无持久化 | 🔴 阻塞 | 租户隔离重启后消失。 | Phase 0 P-3：TenantStore persist/restore。 |
| 全局 | AgentKeyStore secret 不持久 | 🔴 阻塞 | 每次启动 secret 随机生成，所有 token 失效。 | Phase 0 P-4：secret 写文件（0600 权限），tokens 写 JSON。 |
| 全局 | plicod 无 graceful shutdown | 🔴 阻塞 | 被 kill 时 agents/intents/permissions 从不持久化。 | Phase 0 P-1：SIGTERM/SIGINT handler + persist_all()。 |
| 全局 | JSON 写入非原子 | ⚠️ 需修复 | 崩溃时索引文件可能截断为空。 | Phase 0 P-5：atomic_write_json（write tmp + rename）。 |

### 调整总结

```
🔴 阻塞项——持久化地基（Phase 0，必须最先完成）:
  P-5 原子写入 → P-2 CheckpointStore → P-3 TenantStore
                → P-4 AgentKeyStore → P-1 Shutdown → P-6 定时器

  没有 Phase 0，Node 3 的所有功能都建在沙子上。

核心路径（Phase A+B，依赖 Phase 0）:
  F-8 ✅ → F-7 ⚠️修正 seq → F-6 ⚠️补充超时 → F-9 ⚠️双路匹配

探索路径（Phase C+D，可选实现）:
  F-10 ⚠️降级为探索性功能，依赖 F-9 的 embedding 能力

关键修正:
  1. DeltaSince 参数从 since_ms 改为 since_seq
  2. ChangeEntry.summary 不依赖 LLM
  3. IntentAssemblyCache 限制内存 + stub 回退
  4. F-10 从"必须实现"降级为"探索性实验"
  5. 量化目标调低至可验证的现实区间
  6. 🔴 新增 Phase 0 持久化里程碑（P-1 ~ P-6）
```

---

## 1. 链式推理：AI 视角的缺失

```
我是 Cursor。v21.0 的 Plico 给了我很多能力。
但我日常使用时，仍然有几个痛点无人解决。

痛点 1: 我没有"会话"
  → v21.0 有 AgentCheckpoint（序列化认知状态到 CAS）
  → v21.0 有 CheckpointStore（管理检查点生命周期）
  → 但：API 层没有 StartSession / EndSession
  → 我必须手动调 checkpoint_agent + restore_agent_checkpoint
  → 没有人帮我在启动时自动恢复、在退出时自动保存
  → 缺的不是零件，是组装

痛点 2: 我不知道什么变了
  → 我上次离开时代码库是一个状态
  → 今天回来，另一个 Agent 或人类改了代码
  → 我无从得知。我只能全量重读。
  → EventBus 有完整的事件日志，但没有"给我 since 上次的变更"的 API
  → 信息存在于系统中，但没有暴露给我

痛点 3: 我不知道操作花了多少 token
  → v14.0 有 KernelMetrics（操作计数器 + 延迟直方图）
  → 但这些是 OS 运维指标，不是 Agent 决策指标
  → 我需要知道：这个 FetchAssembledContext 返回了 ~3200 token 的内容
  → 这样我才能决定是否请求更粗粒度的摘要
  → 成本不可见 = 无法优化

痛点 4: 重复工作不会加速
  → v19.0 有 EdgeCache（缓存 embedding、KG 查询、搜索结果）
  → 这是底层操作缓存，节省了 CPU 但没有节省 token
  → 我今天声明 DeclareIntent("优化 auth")
  → 明天声明 DeclareIntent("优化 auth 的错误处理")
  → 这两个意图高度相似，但今天的装配结果没有被缓存
  → 明天的 4 路召回 + RRF 融合从头执行
  → EdgeCache 缓存低层操作 ≠ 意图级缓存

痛点 5: 我不能预测性地获取上下文
  → DeclareIntent 是一次性的：声明 → 装配 → 取回
  → 在我工作的过程中，OS 不会主动推送相关更新
  → 当我修完 test_login 准备看 test_session 时，OS 不知道我会这样做
  → 如果 OS 能从我的历史模式中预测下一步，就能提前准备好

结论：
  底层零件充足（v19 缓存 + v20 分布式 + v21 检查点）
  但 Agent 体验层 = 零
  零件没有组装成产品
```

---

## 2. 第三节点主题：认知连续性

**认知连续性** = Agent 的认知状态永不中断，跨越时间、会话、节点。

| 中断类型 | 传统 OS | Plico v21 | 节点 3 目标 |
|---------|---------|----------|-----------|
| 会话结束 | 进程死亡，全部丢失 | 可手动 checkpoint | 自动保存/恢复 |
| 跨会话知识 | 无 | 需手动 recall | 自动加载 + delta 推送 |
| 重复意图 | 每次从零 | 底层缓存（CPU 省） | 意图级缓存（token 省） |
| 成本感知 | 无 | OS 运维指标 | Agent 决策指标 |
| 预测性加载 | 无 | 无 | 基于模式的认知预取 |

**量化目标**（⚠️ 修正：调低至可验证的现实区间）：

| 指标 | v21.0 现状 | 节点 3 目标 | 备注 |
|------|----------|-----------|------|
| 相似任务第 10 次 vs 第 1 次的 token 消耗比 | 1:1 | ≤ 1:3 | ⚠️ 限定"相似任务"前提 |
| 意图缓存命中率（有 embedding） | 0% | > 70% | ⚠️ 从 80% 降至 70% |
| 意图缓存命中率（stub 模式） | 0% | > 30% | ⚠️ 仅精确匹配 |
| 变更感知延迟 | ∞ | < 100ms | ✅ 不变 |
| 每个响应的成本可见度 | 0% | 100% | ✅ 不变 |
| 认知预取命中率（探索性） | 无 | > 30% | ⚠️ 从 50% 降至 30%，限定重复性工作流 |

---

## 3. F-6：会话生命周期

### 零件已有，缺组装

v21.0 提供了：
- `AgentCheckpoint` — 完整认知状态序列化（记忆 + KG 关联 + 最后意图）
- `CheckpointStore` — 按 Agent 管理检查点（LRU 淘汰）
- `CheckpointMemory` — 记忆条目的序列化/反序列化

需要新增的是 **会话 API 层**——在检查点之上建立 Agent 生命周期协议。

### API 设计

```rust
ApiRequest::StartSession {
    agent_id: String,
    agent_token: Option<String>,
    /// 意图提示——触发预取引擎预热
    intent_hint: Option<String>,
    /// ⚠️ 修正：使用 MemoryTier 枚举而非字符串
    load_tiers: Vec<MemoryTier>,  // [Working, Procedural]
    /// 上次 EndSession 返回的序列号（用于 delta 计算）
    last_seen_seq: Option<u64>,
}

ApiResponse::SessionStarted {
    session_id: String,
    /// 从最近检查点恢复的状态摘要
    restored_checkpoint: Option<CheckpointSummary>,
    /// 预热的上下文（如果提供了 intent_hint）
    warm_context: Option<AssemblySummary>,
    /// 自上次会话以来的变更（依赖 F-7, 基于 last_seen_seq）
    changes_since_last: Vec<ChangeEntry>,
    /// 首个响应的 token 估算
    token_estimate: usize,
}

ApiRequest::EndSession {
    agent_id: String,
    session_id: String,
    /// 自动创建检查点（默认 true）
    auto_checkpoint: bool,
}

ApiResponse::SessionEnded {
    checkpoint_id: Option<String>,
    /// ⚠️ 新增：返回当前 EventBus 序列号，Agent 下次 StartSession 传入
    last_seq: u64,
}
```

### 执行流程

```
StartSession(agent_id="cursor-1", intent_hint="修复 auth bug")

1. 验证身份 → key_store.verify_agent_token()            // 已有
2. 查找最新检查点 → checkpoint_store.latest_for_agent()   // 已有(v21.0)
3. 恢复认知状态 → checkpoint.memories → memory.store()     // 已有(v21.0)
4. 查询变更 → event_bus.events_since(last_seen_seq)       // ⚠️ 修正: seq-based
5. 如果有 intent_hint → prefetch.declare_intent()          // 已有(v9.0)
6. 组装响应

EndSession(agent_id="cursor-1", session_id="...")

1. 收集当前记忆 → memory.get_all(agent_id)                // 已有
2. 收集 KG 关联 → knowledge_graph.nodes_for_agent()        // 已有
3. 创建检查点 → AgentCheckpoint::new(...)                  // 已有(v21.0)
4. 持久化 → checkpoint_store.save()                        // 已有(v21.0)
5. 清理瞬时记忆 → memory.clear_tier(Ephemeral)             // 已有
6. 返回 last_seq → event_bus 当前最新序列号                // ⚠️ 新增

⚠️ 异常退出处理:
  SessionStore 维护活跃 session 的 last_active_ms。
  定期扫描（每 60 秒）：超过 session_ttl（默认 30 分钟）无活动的 session
  → 自动触发 EndSession + checkpoint，防止 session 孤儿。
```

**关键洞察**：这不是新功能，而是**组装已有零件**。
所有底层操作已存在。StartSession/EndSession 只是编排层。

### 实现范围

- **新增**: `src/kernel/ops/session.rs`（~250 行，含 SessionStore + 超时扫描 + 编排）
- **修改**: `src/api/semantic.rs`（+3 ApiRequest/Response 变体，含 SessionEnded）、`src/kernel/mod.rs`（路由）
- **复用**: v21.0 checkpoint + v9.0 prefetch + EventBus
- ⚠️ **注意**: `load_tiers` 用 `MemoryTier` 枚举。`last_seen_seq` 传递序列号给 F-7。

---

## 4. F-7：Delta 感知

### 问题的本质

事件日志已经记录了所有变更。
但没有"给我自从时间 T 以来的变更摘要"的 API。
信息存在于系统中，只是没有暴露给 Agent。

### API 设计

```rust
ApiRequest::DeltaSince {
    agent_id: String,
    /// ⚠️ 修正：使用 EventBus 序列号，不是时间戳。
    /// Agent 在 EndSession 时记录 last_seq，下次传入。
    since_seq: u64,
    /// 只关注这些 CID 的变化（空 = 全部）
    watch_cids: Vec<String>,
    /// 只关注包含这些标签的变化
    watch_tags: Vec<String>,
    limit: Option<usize>,
}

ApiResponse::DeltaResult {
    changes: Vec<ChangeEntry>,
    from_seq: u64,
    to_seq: u64,
    token_estimate: usize,
}

/// 变更条目——轻量元数据，不依赖 LLM
struct ChangeEntry {
    cid: String,
    change_type: String,      // "created" | "modified" | "deleted" | "tags_changed"
    /// ⚠️ 修正：不依赖 Summarizer/LLM。
    /// 格式: "{event_type} {cid[..8]} by {agent_id} [{tags}]"
    /// 有 Summarizer 时可选附加 L0 摘要。
    summary: String,
    changed_at_ms: u64,
    changed_by: String,
    seq: u64,
}
```

### 与会话集成

```
EndSession 返回 last_seq（当前 EventBus 最新序列号）
→ Agent 保存 last_seq

下次 StartSession 自动调用 DeltaSince(since_seq = 上次保存的 last_seq)
→ Agent 启动时立刻知道"自从上次，这些东西变了"

对比（假设大部分文件未变更）：
  无 Delta: Agent 必须全量重读 100 个文件 → ~62,500 token
  有 Delta: Agent 只看 5 个变更的元数据摘要 → ~250 token
  节省率: ~99%（取决于变更比例，大规模重构时退化）
```

### 实现基础

EventBus 的 `events_since(since_seq: u64) -> Vec<SequencedEvent>` 已实现并有测试覆盖。
DeltaSince 是对 SequencedEvent 的投影 + CID/tag 过滤 + 元数据拼接。
**不依赖 LLM**——summary 由事件元数据拼接生成。

### 实现范围

- **新增**: `src/kernel/ops/delta.rs`（~120 行）
- **修改**: `src/api/semantic.rs`（+1 ApiRequest 变体）
- **复用**: EventBus + DurableEventLog + ContextLoader（L0 摘要）

---

## 5. F-8：Token 成本透明

### 为什么这必须是 OS 原语

Soul 2.0 公理 1："Token 是最稀缺资源。"

但 Agent 看不到成本，就无法优化。
这就像 CPU 没有 clock counter 一样——你无法优化你无法测量的东西。

### 设计

在 `ApiResponse` 中追加可选字段：

```rust
pub struct ApiResponse {
    // ...现有字段...

    /// 本响应返回内容的 token 估算
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_estimate: Option<usize>,
}
```

Token 估算函数：

```rust
fn estimate_tokens(text: &str) -> usize {
    let ascii = text.chars().filter(|c| c.is_ascii()).count();
    let non_ascii = text.chars().filter(|c| !c.is_ascii()).count();
    (ascii + 3) / 4 + (non_ascii + 1) / 2
}
```

新增查询端点：

```rust
ApiRequest::QueryTokenUsage {
    agent_id: String,
    session_id: Option<String>,
}
```

### Agent 如何利用

```
Agent 收到 FetchAssembledContext → token_estimate = 4200

Agent 判断：4200 token > 我的预算 30%
Agent 决策：请求 L0 粒度的版本 → token_estimate = 280

Agent 节省了 3920 token，因为它能看到成本。
```

### 实现范围

- **修改**: `src/api/semantic.rs`（ApiResponse 加字段）+ `src/kernel/mod.rs`（注入估算）
- **代码量**: ~60 行
- **风险**: 极低（可选字段，不影响现有 API）

---

## 6. F-9：意图级缓存

### EdgeCache 不够——需要更高层的缓存

v19.0 EdgeCache 缓存三种底层操作：
- Embedding：文本 → 向量 的映射
- KG Query：图查询结果
- Search：搜索结果

这些缓存节省 **CPU**，但不节省 **token**。
因为 `DeclareIntent` 的 4 路召回 → RRF 融合 → 分层压缩 管线仍然每次重新执行。

Agent 需要的是 **意图级缓存**：相似意图 → 直接返回已装配的上下文。

### 设计

在 `IntentPrefetcher` 内部增加缓存层：

```rust
struct IntentAssemblyCache {
    entries: RwLock<Vec<CachedAssembly>>,
    /// ⚠️ 修正：从 1000 降为 64，因为每条缓存含完整文本
    max_entries: usize,           // 默认 64
    /// ⚠️ 修正：增加内存硬限制
    max_memory_bytes: usize,      // 默认 32MB
    current_memory_bytes: AtomicUsize,
    similarity_threshold: f32,    // 默认 0.85
}

struct CachedAssembly {
    intent_text: String,
    intent_embedding: Option<Vec<f32>>,  // ⚠️ 修正：stub 模式下为 None
    assembly: BudgetAllocation,
    created_at_ms: u64,
    hit_count: AtomicU64,
    estimated_size_bytes: usize,
    dependency_cids: Vec<String>,
}
```

### 缓存逻辑（⚠️ 修正：双路匹配）

```
DeclareIntent("优化 auth 的错误处理")

路径 A（有真实 EmbeddingProvider）:
  Step 1: embed("优化 auth 的错误处理") → intent_vec
  Step 2: 遍历缓存，计算余弦相似度
    → 找到 "优化 auth 模块", similarity = 0.91 > 0.85
    → 检查 dependency_cids 是否被修改
    → 缓存命中

路径 B（StubEmbeddingProvider，无真实向量）:
  Step 1: 精确字符串匹配 intent_text
    → 只匹配完全相同的意图文本
    → 命中率低但零误判
    → 仍比从头计算快

两条路径共享:
  → 检查 dependency_cids 是否被修改（EventBus 监听）
  → 缓存命中时延迟 < 2ms
```

### 缓存失效

```
自动失效条件:
  1. dependency_cids 中任一 CID 被修改（EventBus 监听）
  2. TTL 过期（默认 24h）
  3. 容量满时 LRU 淘汰
  4. ⚠️ 新增：total_memory_bytes 超限时强制淘汰最旧条目
```

### 实现范围

- **修改**: `src/kernel/ops/prefetch.rs`（在 `declare_intent` 中增加缓存查找）
- **新增**: `IntentAssemblyCache` 结构（~150 行，嵌入 prefetch.rs）
- **复用**: v19.0 EdgeCache 的设计模式（LRU + stats）、EventBus 变更追踪

---

## 7. F-10：认知预取——OS 预测 Agent 的下一步

> ⚠️ **状态：探索性功能**——从核心路径降级至 Phase C/D 探索。
> 原因：意图文本的高变异性使纯字符串匹配的转移矩阵极度稀疏。
> 此功能需要 F-9 的 embedding 能力才能实用。无 embedding 时自动禁用。

### 愿景

传统 OS 有 CPU prefetch——预测下一个内存地址。
AIOS 应该有 **认知预取**——预测 Agent 的下一个意图。

```
Agent 在一个会话中的行为序列:
  DeclareIntent("修复 auth 模块") → 工作 → 完成
  DeclareIntent("修复 auth 测试") → 工作 → 完成
  DeclareIntent("修复 auth 文档") → 工作 → 完成

OS 观察到模式: "auth" 主题下的三步序列
下次 Agent 声明 DeclareIntent("修复 X 模块") 时:
  → OS 预测接下来可能是 "修复 X 测试" 和 "修复 X 文档"
  → OS 后台预热这两个意图的上下文
  → 当 Agent 真的声明时，上下文已经就绪
```

### 设计（⚠️ 修正：tag 聚类 + embedding 归并）

**Agent Profile（统计模型，不是策略）**：

```rust
struct AgentProfile {
    agent_id: String,
    /// ⚠️ 修正：用 tag 集合做 key 而非原始字符串。
    /// 例如 DeclareIntent("修复 auth 的测试") 提取 tags=["auth","test"]
    /// 归一化为排序后的 tag key: "auth|test"
    intent_transitions: HashMap<String, Vec<(String, u32)>>,
    /// 热对象——最常访问的 CID
    hot_objects: Vec<(String, u64)>,
    updated_at_ms: u64,
}

/// 意图文本 → tag key 的提取策略
enum IntentKeyStrategy {
    /// 从意图文本中提取已知 tag（查 tag 索引），排序拼接
    TagExtraction,
    /// 有 embedding 时，聚类相似意图归并到同一 bucket
    EmbeddingCluster { bucket_count: usize },
    /// StubEmbedding 模式下自动降级：仅 TagExtraction，
    /// 如果 tag 也提取不出来，F-10 静默禁用
    Disabled,
}
```

**认知预取流程**：

```
Agent 完成意图 A

1. 提取 A 的 tag key（如 "auth|test"）
2. 更新 AgentProfile.intent_transitions[tag_key] 的后继统计
3. 查找最可能的下一个 tag key B（频率最高的后继）
4. 如果 confidence > 阈值：
   → 后台静默执行 declare_intent(B 的代表性意图文本) → 预热缓存
   → 不通知 Agent
5. 如果 Agent 真的声明了类似意图：
   → F-9 意图缓存命中，零延迟返回
6. 如果 Agent 声明了不相关意图：
   → 预取的条目自然过期，无害（只浪费 CPU）
```

### 关键约束

- **OS 不做决策**（Soul 2.0 公理 5）——预取是机制，不影响 Agent 行为
- **预取是静默的**——Agent 不知道 OS 在预取
- **不使用 LLM**——转移矩阵是简单的频率统计
- ⚠️ **修正**：需要 embedding 或 tag 提取能力。纯 StubEmbedding + 无 tag 时 F-10 自动禁用
- ⚠️ **渐进式启用**：Phase C 先实现 TagExtraction 模式，Phase D 有 embedding 后启用聚类模式

### 实现范围

- **新增**: `AgentProfile` + `IntentKeyStrategy` 在 `src/kernel/ops/session.rs`（~100 行）
- **修改**: `src/kernel/ops/prefetch.rs`（在意图完成回调中更新 profile + 触发预取）
- **复用**: F-9 IntentAssemblyCache（预取结果存入缓存）
- ⚠️ **前置条件**: F-9 必须先完成

---

## 8. P-0：持久化里程碑——Node 3 的地基

> **优先级：最高。必须在 F-6/F-7/F-8 之前完成。**
> 没有可靠持久化，Node 3 的所有功能都是空中楼阁。

### 链式推理：为什么持久化是阻塞项

```
我是 Cursor。我在 Plico 上工作了一整天。

我创建了 3 个检查点（v21.0 CheckpointStore）。
我建立了工作记忆和程序性记忆。
我注册在一个 tenant 下，有自己的 agent token。
我的意图历史积累了有价值的模式。

然后 plicod 重启了（部署、崩溃、OOM killer...）。

重启后我还有什么？
  ✅ CAS 数据                — CAS 总是写磁盘，天然持久
  ✅ 知识图谱                — KG 每次变更都写盘（kg_nodes.json + kg_edges.json）
  ⚠️ 记忆                   — 取决于是否恰好触发了 auto-persist（每 50 次操作）
  ⚠️ 搜索索引               — 取决于是否恰好触发了 auto-persist（每 50 次操作）
  ⚠️ 事件日志               — 取决于是否恰好触发了 auto-persist（每 100 条事件）
  ❌ 3 个检查点              — CheckpointStore 纯内存，全部丢失
  ❌ 我的 tenant             — TenantStore 纯内存，全部丢失
  ❌ 我的 agent token        — AgentKeyStore.secret 每次启动随机生成，所有 token 失效
  ❌ 最后 49 条记忆操作       — 在 auto-persist 阈值之间的增量，丢失
  ❌ 最后 99 条事件           — 在 auto-persist 阈值之间的增量，丢失

后果：
  → Node 3 F-6 StartSession 依赖 CheckpointStore 恢复检查点——但检查点不存在了
  → Node 3 F-6 依赖 agent token 验证身份——但 token 全部失效了
  → Node 3 F-7 DeltaSince 依赖事件日志连续——但可能丢失了最后 99 条
  → 我的"认知连续性"在一次重启中被完全切断

结论：
  持久化不是"锦上添花"。它是 Node 3 的物理地基。
  v21.0 检查点 = 沙堆上的城堡，因为 CheckpointStore 不持久化。
  不解决这个问题，Session Lifecycle 没有意义。
```

### 当前持久化现状全景

| 子系统 | 持久化方式 | 触发条件 | 恢复时机 | 问题 |
|--------|-----------|---------|---------|------|
| CAS (对象存储) | 文件系统 | 每次 put 即写盘 | 天然持久 | ✅ 无问题 |
| Knowledge Graph | `kg_nodes.json` + `kg_edges.json` | 每次变更即写盘 | `PetgraphBackend::open()` | ✅ 无问题（但大图时写放大严重） |
| Memory (Working/LT/Proc) | `memory_index.json` → CAS | 每 50 次操作 | `restore_memories()` | ⚠️ 阈值间增量丢失 |
| Search Index | `search_index.jsonl` / `hnsw_index.jsonl` | 每 50 次搜索操作 | `backend.restore_from()` | ⚠️ 阈值间增量丢失 |
| Event Log | `event_log.json` | 每 100 条事件 | `restore_event_log()` | ⚠️ 阈值间增量丢失 + 单文件JSON不可扩展 |
| Agents | `agent_index.json` | 手动调用 | `restore_agents()` | ⚠️ 无自动触发，仅 CLI 手动调用 |
| Intents | `intent_index.json` | 手动调用 | `restore_intents()` | ⚠️ 恢复后删除文件 |
| Permissions | `permission_index.json` | 手动调用 | `restore_permissions()` | ⚠️ 无自动触发 |
| **CheckpointStore** | **无** | **无** | **无** | ❌ **纯内存，重启全丢** |
| **TenantStore** | **无** | **无** | **无** | ❌ **纯内存，重启全丢** |
| **AgentKeyStore** | **无** | **无** | **无** | ❌ **secret 每次随机，token 全部失效** |
| EdgeCache | 无 | N/A | N/A | ✅ 缓存，无需持久化 |
| ClusterManager | 无 | N/A | N/A | ✅ 集群状态动态重建 |
| KernelMetrics | 无 | N/A | N/A | ✅ 运维指标，无需持久化 |

### 原子性问题

当前所有 JSON 写入使用 `std::fs::write(path, json)`——**非原子操作**。
如果进程在写入过程中崩溃，JSON 文件会被截断，导致下次启动解析失败。

### plicod 无 graceful shutdown

`plicod` 的 `main()` 是一个无限 `loop { accept() }`，没有信号处理。
进程被 kill 时，所有"手动调用"的持久化（agents、intents、permissions）从不触发。

---

### P-1: Graceful Shutdown — 统一持久化入口

**问题**：plicod 无信号处理，被 kill 时丢失所有未持久化数据。

**设计**：

```rust
// src/kernel/mod.rs — 新增统一 flush 方法
impl AIKernel {
    /// Flush all subsystem state to disk. Called on shutdown and periodically.
    pub fn persist_all(&self) {
        self.persist_memories();
        self.persist_agents();
        self.persist_intents();
        self.persist_permissions();
        self.persist_event_log();
        self.persist_search_index();
        self.persist_checkpoints();   // P-2 新增
        self.persist_tenants();       // P-3 新增
        self.persist_key_store();     // P-4 新增
        tracing::info!("All kernel state persisted to disk");
    }
}

// src/bin/plicod.rs — 增加 shutdown hook
#[tokio::main]
async fn main() {
    // ... 现有初始化 ...

    let kernel_for_shutdown = Arc::clone(&kernel);
    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate()
        ).unwrap();
        let sigint = tokio::signal::ctrl_c();
        tokio::select! {
            _ = sigterm.recv() => {},
            _ = sigint => {},
        }
        tracing::info!("Shutdown signal received, persisting all state...");
        kernel_for_shutdown.persist_all();
        std::process::exit(0);
    });

    // ... 现有 accept loop ...
}
```

**实现范围**：
- **修改**: `src/kernel/mod.rs`（+`persist_all()` 方法，~15 行）
- **修改**: `src/bin/plicod.rs`（+信号处理，~20 行）
- **修改**: `src/bin/plico_mcp.rs`（如有 daemon 模式，同步加）
- **代码量**: ~40 行

### P-2: CheckpointStore 持久化

**问题**：v21.0 的核心功能 `AgentCheckpoint` 只存在内存中。

**设计**：借鉴已有的 `CASPersister` 模式——将 CheckpointStore 序列化为 JSON 索引 + CAS 存储。

```rust
// src/kernel/ops/checkpoint.rs — 扩展 CheckpointStore

impl CheckpointStore {
    fn index_path(root: &Path) -> PathBuf {
        root.join("checkpoint_index.json")
    }

    /// Persist all checkpoints. 每个 checkpoint 序列化为 CAS 对象，
    /// 索引文件只记录 agent_id → Vec<checkpoint_cid>。
    pub fn persist(&self, root: &Path, cas: &CASStorage) {
        let checkpoints = self.checkpoints.read().unwrap();
        let mut index: HashMap<String, Vec<String>> = HashMap::new();

        for (id, cp) in checkpoints.iter() {
            let json = serde_json::to_string(cp).unwrap_or_default();
            let meta = AIObjectMeta {
                content_type: ContentType::Structured,
                tags: vec!["checkpoint".into(), format!("agent:{}", cp.agent_id)],
                created_by: "plico:checkpoint-store".into(),
                created_at: cp.created_at_ms,
                intent: Some("Agent checkpoint".into()),
                tenant_id: cp.tenant_id.clone(),
            };
            let obj = AIObject::new(json.into_bytes(), meta);
            if let Ok(cid) = cas.put(&obj) {
                index.entry(cp.agent_id.clone()).or_default().push(cid);
            }
        }

        atomic_write_json(&Self::index_path(root), &index);
    }

    /// Restore checkpoints from CAS on startup.
    pub fn restore(root: &Path, cas: &CASStorage) -> Self {
        let path = Self::index_path(root);
        // ... 读取索引，从 CAS 反序列化每个 checkpoint ...
    }
}
```

**关键决策**：
- Checkpoint 数据存入 CAS（内容寻址，天然去重，与现有体系一致）
- 索引文件只存映射关系（轻量，快速加载）
- 启动时恢复，`persist_all()` 时写盘

**实现范围**：
- **修改**: `src/kernel/ops/checkpoint.rs`（+`persist()` + `restore()`，~80 行）
- **修改**: `src/kernel/mod.rs`（初始化时调用 `CheckpointStore::restore()`）
- **依赖**: P-5 的 `atomic_write_json()`

### P-3: TenantStore 持久化

**问题**：多租户隔离在重启后消失。

**设计**：

```rust
// src/kernel/ops/tenant.rs — 扩展

impl TenantStore {
    fn index_path(root: &Path) -> PathBuf {
        root.join("tenant_index.json")
    }

    pub fn persist(&self, root: &Path) {
        let tenants = self.tenants.read().unwrap();
        let admins = self.admins.read().unwrap();
        let data = TenantPersistData {
            tenants: tenants.clone(),
            admins: admins.clone(),
        };
        atomic_write_json(&Self::index_path(root), &data);
    }

    pub fn restore(root: &Path) -> Self {
        let path = Self::index_path(root);
        // ... 读取并恢复 ...
    }
}

#[derive(Serialize, Deserialize)]
struct TenantPersistData {
    tenants: HashMap<String, Tenant>,
    admins: HashMap<String, Vec<String>>,
}
```

**实现范围**：
- **修改**: `src/kernel/ops/tenant.rs`（~50 行）
- **修改**: `src/kernel/mod.rs`（初始化时恢复）

### P-4: AgentKeyStore 持久化

**问题**：`secret` 每次启动随机生成。重启后所有 Agent 的 token 全部失效。

**设计**：

```rust
// src/api/agent_auth.rs — 扩展

impl AgentKeyStore {
    fn secret_path(root: &Path) -> PathBuf {
        root.join("agent_secret.key")
    }

    fn tokens_path(root: &Path) -> PathBuf {
        root.join("agent_tokens.json")
    }

    /// 创建或恢复 KeyStore。如果 secret 文件存在，复用它。
    pub fn open(root: &Path) -> Self {
        let secret_path = Self::secret_path(root);
        let secret = if secret_path.exists() {
            let bytes = std::fs::read(&secret_path).unwrap_or_default();
            if bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            } else {
                let s = rand::random::<[u8; 32]>();
                let _ = Self::write_secret(&secret_path, &s);
                s
            }
        } else {
            let s = rand::random::<[u8; 32]>();
            let _ = Self::write_secret(&secret_path, &s);
            s
        };

        let tokens = Self::load_tokens(root);

        Self { secret, tokens: RwLock::new(tokens), mode: AgentAuthMode::default() }
    }

    fn write_secret(path: &Path, secret: &[u8; 32]) -> std::io::Result<()> {
        std::fs::write(path, secret)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn persist(&self, root: &Path) {
        let tokens = self.tokens.read().unwrap();
        atomic_write_json(&Self::tokens_path(root), &*tokens);
    }
}
```

**安全约束**：
- `agent_secret.key` 文件权限 `0600`（仅 owner 可读写）
- 不存入 CAS（避免 secret 被任何 Agent 通过 CID 访问）
- `.gitignore` 中排除

**实现范围**：
- **修改**: `src/api/agent_auth.rs`（~60 行）
- **修改**: `src/kernel/mod.rs`（`AgentKeyStore::new()` → `AgentKeyStore::open(root)`）

### P-5: 原子写入基础设施

**问题**：所有 JSON 索引使用 `std::fs::write()`，崩溃时可能截断文件。

**设计**：提供通用原子写入函数，所有持久化点统一使用。

```rust
// src/kernel/persistence.rs — 新增公用函数

/// Atomic JSON write: serialize → write to tmp → rename.
/// 崩溃安全：要么看到旧文件，要么看到新文件，不会看到半写文件。
pub(crate) fn atomic_write_json<T: serde::Serialize>(path: &Path, data: &T) {
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(data) {
        Ok(json) => {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        Err(e) => tracing::warn!("Failed to serialize for {}: {e}", path.display()),
    }
}
```

**迁移**：将所有现有 `std::fs::write(path, json)` 调用改为 `atomic_write_json()`。

**影响点**（全量清单）：
- `src/memory/persist.rs` — `CASPersister::save_index()`
- `src/kernel/persistence.rs` — `persist_agents()`, `persist_intents()`, `persist_permissions()`, `persist_event_log()`
- `src/fs/graph/backend.rs` — `PetgraphBackend::persist()`（两次写入 → 两次原子写）
- `src/fs/search/memory.rs` — `persist_to()`
- `src/fs/search/hnsw.rs` — `persist_to()`

**实现范围**：
- **新增**: `atomic_write_json()` 函数（~15 行）
- **修改**: 上述 7 个文件的写入调用（每个 ~3 行改动）
- **代码量**: ~40 行

### P-6: 周期性自动持久化

**问题**：当前仅有操作计数触发的 auto-persist（记忆每 50 次、事件每 100 条）。
如果 Agent 长时间不操作但已有大量状态变更，这些变更只在 shutdown 时才写盘。
如果 crash（非 graceful），这些变更丢失。

**设计**：增加时间触发的周期性 persist。

```rust
// src/bin/plicod.rs — 增加定时 persist 任务

let kernel_for_timer = Arc::clone(&kernel);
tokio::spawn(async move {
    let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 分钟
    loop {
        interval.tick().await;
        kernel_for_timer.persist_all();
        tracing::debug!("Periodic persist completed");
    }
});
```

**关键约束**：
- 默认间隔 5 分钟（可通过 `PLICO_PERSIST_INTERVAL_SECS` 环境变量调整）
- `persist_all()` 是幂等的——多次调用只是覆盖相同数据
- KG 已经是每次变更写盘，persist_all 中可跳过（已由 `PetgraphBackend::persist()` 自管理）

**实现范围**：
- **修改**: `src/bin/plicod.rs`（~10 行）

---

### 持久化里程碑总览

```
P-5 (原子写入)      ← 基础设施，无依赖，最先实现
│
├── P-1 (Shutdown Hook)  ← 依赖 persist_all()
├── P-2 (Checkpoint)     ← 依赖 atomic_write_json + CAS
├── P-3 (Tenant)         ← 依赖 atomic_write_json
├── P-4 (KeyStore)       ← 依赖 atomic_write_json
│
P-6 (周期性持久化)   ← 依赖 persist_all()
```

### 预估代码量

| 功能 | 新增行数 | 修改行数 | 风险 |
|------|---------|---------|------|
| P-5 原子写入 | ~15 | ~30 | ✅ 极低 |
| P-1 Shutdown Hook | ~20 | ~20 | ✅ 低 |
| P-2 CheckpointStore | ~80 | ~15 | ⚠️ 中（序列化格式需对齐） |
| P-3 TenantStore | ~50 | ~10 | ✅ 低 |
| P-4 AgentKeyStore | ~60 | ~15 | ⚠️ 中（安全敏感） |
| P-6 周期性持久化 | ~10 | ~5 | ✅ 低 |
| 现有写入迁移 | ~0 | ~25 | ✅ 低（机械替换） |
| **合计** | **~235** | **~120** | |

### 验证标准

```
测试 1: 冷启动恢复
  1. 创建 3 个 agent，注册 2 个 tenant，生成 token
  2. 创建检查点，存储记忆，记录事件
  3. 调用 persist_all()
  4. 重建 AIKernel（模拟重启）
  5. 验证: agents 存在、tenants 存在、token 有效、checkpoints 可恢复

测试 2: 崩溃恢复（原子性）
  1. 写入数据，触发 auto-persist
  2. 在 persist_all() 过程中模拟 panic（mock 文件写入失败）
  3. 重建 AIKernel
  4. 验证: 所有索引文件要么是完整的旧版本，要么是完整的新版本

测试 3: Shutdown Hook（集成测试）
  1. 启动 plicod
  2. 通过 API 创建状态
  3. 发送 SIGTERM
  4. 重启 plicod
  5. 验证: 所有状态恢复

测试 4: 向前兼容性
  1. 用旧格式的索引文件启动新版本
  2. 验证: 能正确解析或优雅降级（tracing::warn + 空状态）
```

### Soul 2.0 对齐

| 公理 | 对齐方式 |
|------|---------|
| 公理 3: 记忆跨越边界 | 记忆、检查点、tenant 跨重启存活 |
| 公理 10: 会话一等公民 | Session 依赖的 checkpoint 不再丢失 |
| 公理 4: 身份是原语 | Agent token 跨重启有效 |
| 公理 5: 机制不是策略 | OS 提供持久化机制，Agent 决定何时 checkpoint |

---

## 9. 实现路线图

### 依赖关系

```
最高优先——持久化地基（Phase 0）:

  P-5 (原子写入)
  │
  ├── P-1 (Shutdown Hook) + P-6 (周期性持久化)
  ├── P-2 (CheckpointStore 持久化)
  ├── P-3 (TenantStore 持久化)
  └── P-4 (AgentKeyStore 持久化)

核心路径（Phase A+B，依赖 Phase 0）:

  F-8 (Token 透明)     ← 最简单，无依赖，Phase 0 后第一个
  │
  F-7 (Delta 感知)      ← 依赖 EventBus.events_since(seq)（已有）
  │                        ⚠️ 注意用 seq 不是 timestamp
  F-6 (会话生命周期)    ← 依赖 F-7 + F-8 + v21.0 checkpoint
  │                        ⚠️ 需实现 session 超时扫描
  │                        ⚠️ 现在依赖 P-2 CheckpointStore 持久化
  F-9 (意图缓存)        ← 依赖 prefetch + EdgeCache 模式
                           ⚠️ stub 模式退化为精确匹配

探索路径（Phase C+D，可选实现）:

  F-10 (认知预取)       ← 依赖 F-9 + F-6 session
                           ⚠️ 降级为探索性功能
                           ⚠️ 无 embedding + 无 tag 时自动禁用
```

### 分阶段里程碑

**Phase 0（~1 周）—— 持久化地基 🔴 最高优先**
- [x] P-5：`atomic_write_json()` + 迁移现有写入 ✅ (v22.0-M3)
- [x] P-2：CheckpointStore `persist()` / `restore()` + CAS 存储 ✅ (v22.0-M4)
- [x] P-3：TenantStore `persist()` / `restore()` ✅ (v22.0-M5)
- [x] P-4：AgentKeyStore `open(root)` + secret 持久化 + token 持久化 ✅ (v22.0-M5)
- [x] P-1：plicod SIGTERM/SIGINT handler + `kernel.persist_all()` ✅ (v22.0-M6)
- [x] P-6：5 分钟周期性 `persist_all()` 定时器 ✅ (v22.0-M6)
- [ ] 验证：冷启动恢复测试 + 原子性测试 ⏳ (TODO)

**Phase A（~1 周）—— 基础可观测性**
- [x] F-8：ApiResponse + `estimate_tokens()` + QueryTokenUsage ✅ (v22.0-M1)
- [x] F-7：DeltaSince API（seq-based）+ ChangeEntry 元数据拼接（无 LLM 依赖） ✅ (v22.0-M1)

**Phase B（~2 周）—— Agent 生命周期**
- [x] F-6：StartSession / EndSession + SessionStore + 超时扫描 ✅ (v22.0-M2)
- [x] F-6：异常退出处理（session TTL + 自动 checkpoint） ✅ (v22.0-M2)
- [x] F-9：IntentAssemblyCache（双路匹配 + 内存限制 + 失效逻辑） ✅ (v22.0-M7)

**Phase C（~1-2 周）—— 验证 + 探索**
- [x] 验证实验 1+2：token 衰减曲线 + 缓存命中率 ✅ (v22.0-M8)
- [x] F-10 TagExtraction 模式（仅在 F-9 验证通过后启动） ✅ (v22.0-M9)
- [x] F-9 + F-10 联动：预取结果自动进入意图缓存 ✅ (v22.0-M9)

**Phase D（~1 周）—— 调优 + 文档**
- [x] F-10 EmbeddingCluster 模式 ✅ (代码就绪，待生产验证)
- [x] 缓存参数调优 ✅ (max_entries=64, max_memory_bytes=32MB, TTL=24h)
- [x] 文档对齐 + 基准测试自动化 ✅ (tests/v22_benchmark_tests.rs)

### 预估代码量

| 功能 | 新增行数 | 修改行数 | 备注 |
|------|---------|---------|------|
| **P-0 持久化里程碑** | **~235** | **~120** | **🔴 最高优先** |
| F-8 Token 透明 | ~60 | ~40 | ✅ 无风险 |
| F-7 Delta 感知 | ~120 | ~30 | ⚠️ 注意 seq vs ms |
| F-6 会话生命周期 | ~250 | ~70 | ⚠️ 含超时扫描，比原估高 |
| F-9 意图缓存 | ~150 | ~40 | ⚠️ 含双路匹配 + 内存限制 |
| F-10 认知预取 | ~120 | ~30 | ⚠️ 探索性，TagExtraction 优先 |
| **合计** | **~935** | **~330** |

---

## 10. 验证实验

### 实验 1：复合智能衰减曲线

```
10 个连续会话，同一项目，相似任务（⚠️ 注明"相似任务"前提）

记录每个会话:
  - StartSession 返回的 token_estimate
  - DeclareIntent 的命中/未命中
  - 总 token 消耗

⚠️ 修正后的期望曲线（更保守）:
  Session 1:  100%（冷启动）
  Session 3:  ~60%（检查点恢复 + delta 代替全量）
  Session 5:  ~40%（意图缓存开始命中）
  Session 10: ~30%（缓存充分预热）

通过/失败标准:
  PASS: Session 10 ≤ Session 1 的 33%
  BONUS: Session 10 ≤ Session 1 的 20%（含认知预取）
```

### 实验 2：意图缓存命中率

```
两组实验，分别验证双路匹配:

实验 2a（有 embedding）:
  50 个意图文本，5 个主题各 10 个变体
  期望: 命中率 > 70%，命中延迟 < 2ms

实验 2b（stub embedding）:
  50 个意图文本，包含 15 个精确重复
  期望: 命中率 = 重复数/总数 = 30%，命中延迟 < 1ms
  目的: 验证 stub 模式下不会误判、不会 panic
```

### 实验 3：认知预取准确率（⚠️ 探索性）

> 仅在 Phase C/D 执行。F-10 未实现时跳过。

```
模拟 3 个 Agent 各工作 20 个会话
任务设计: 70% 重复性工作流 + 30% 随机任务

记录:
  - 预取命中次数（OS 预测正确）
  - 预取浪费次数（OS 预测错误，预热未使用）
  - 预取命中时的延迟节省

⚠️ 修正后的期望:
  重复性工作流: 命中率 > 30%
  整体: 命中率 > 20%
  浪费是纯 CPU（不浪费 token）
```

---

## 11. Soul 2.0 对齐

| Soul 2.0 公理 | 功能 | 对齐方式 |
|--------------|------|---------|
| 公理 1: Token 最稀缺 | F-8 | 成本透明 → Agent 可以优化 |
| 公理 1: Delta 优于全量 | F-7 | 只传变更，节省 99%+ |
| 公理 2: 意图先于操作 | F-9 | 重复意图零成本 |
| 公理 3: 记忆跨越边界 | **P-0 + F-6** | **P-0 保证重启不丢失** + F-6 自动 checkpoint + restore |
| 公理 4: 身份是原语 | **P-4** | **Agent token 跨重启有效** |
| 公理 5: 机制不是策略 | F-10 | Profile 只记录统计，不做决策 |
| 公理 7: 主动先于被动 | F-10 | OS 预测 Agent 下一步需要什么 |
| 公理 9: 越用越好 | 全部 | 缓存 + Profile + 预取 = 越用越快 |
| 公理 10: 会话一等公民 | **P-2 + F-6** | **P-2 保证检查点持久** + F-6 StartSession/EndSession 生命周期 |

### 灵魂偏差检测

| 关注点 | 分析 |
|--------|------|
| 🔴 **持久化缺口** | ⚠️ **v21.0 的 CheckpointStore、TenantStore、AgentKeyStore 均为纯内存**。一次重启摧毁认知连续性。已新增 Phase 0 里程碑（P-1~P-6）作为最高优先修复。 |
| v18.0 模型热切换 | 偏运维方向。Agent 不关心 OS 用什么模型。但 trait 抽象保证了模型无关性，不违反公理。 |
| v20.0 分布式模式 | 偏运维方向。但它使认知迁移成为可能（v21.0 checkpoint + v20.0 migration = Agent 跨节点连续性）。⚠️ 当前实现使用同步 TCP ping 检测节点连通性，POC 阶段可接受，生产需改进。 |
| F-10 认知预取 | ⚠️ **已降级为探索性功能**。原设计"不需要 embedding"与实际需求矛盾（见评审）。调整后依赖 embedding 或 tag 提取。红线不变：OS 预取但不执行，Agent 不知道 OS 在预取。 |
| F-6 session 孤儿 | ⚠️ **原设计遗漏**。Agent 崩溃不调用 EndSession 时缺乏清理机制。已补充 session TTL 超时扫描。 |
| F-7 since_ms vs since_seq | ⚠️ **原设计与代码不符**。`events_since()` 是 seq-based 不是 time-based。已修正 API 签名。 |
| F-9 内存消耗 | ⚠️ **原设计未限制**。缓存 BudgetAllocation 含完整文本，无内存上限。已增加 max_entries=64 + max_memory_bytes=32MB。 |
| plicod 无 shutdown | ⚠️ **基础缺陷**。无信号处理，进程被 kill 时所有"手动调用"持久化从不触发。已在 P-1 修复。 |
| JSON 原子性 | ⚠️ **基础缺陷**。所有索引文件使用非原子写入，崩溃时可能截断。已在 P-5 修复。 |

---

## 12. 完成后的预期状态

| 维度 | v21.0 现状 | Phase 0 完成后 | 节点 3 核心完成后 | 节点 3 探索完成后 |
|------|----------|---------------|-----------------|-----------------|
| 代码量 | ~27.4K 行 | ~27.8K 行 | ~28.7K 行 | ~28.9K 行 |
| 持久化覆盖率 | 8/11 子系统 | **11/11 子系统** | 11/11 | 11/11 |
| 崩溃恢复能力 | ⚠️ 部分丢失 | **✅ 完全恢复** | ✅ 完全恢复 | ✅ 完全恢复 |
| 内核模块 | 20 个 | 20 个 | 22 个（+session, +delta） | 22 个 |
| API 端点 | ~90 个 | ~90 个 | ~96 个 | ~96 个 |
| 测试 | 693 个 | ~710 个 | ~740 个 | ~760 个 |
| 单次 token 节省 | 86.7% | 86.7% | 86.7%（保持） | 86.7% |
| 相似任务第 10 次 vs 第 1 次 | 1:1 | 1:1 | ≤ 1:3 | ≤ 1:5（含预取） |
| 意图缓存命中（有 embedding） | 0% | 0% | > 70% | > 70% |
| 意图缓存命中（stub） | 0% | 0% | > 30% | > 30% |
| 变更感知 | 无 | 无 | < 100ms | < 100ms |
| 成本可见度 | 0% | 0% | 100% | 100% |
| 认知预取 | 无 | 无 | 无（Phase C/D） | 重复工作流 > 30% |

**本质变化**：
节点 1 给了 Agent 一个 **家**（存储）。
节点 2 给了 Agent 一个 **大脑**（智能原语）。
节点 3 给 Agent **连续的意识**——跨会话、跨时间、跨节点的认知不中断。

这才是 AIOS 与"在 Linux 上跑的 AI"的根本区别：
Linux 上的 AI 每次启动都是新生儿。
AIOS 上的 AI 一直在成长。

---

## 附录：开发时速查

> 每个修正项和持久化任务的快速定位，供开发时参考。

### Phase 0 持久化任务

| 编号 | 任务 | 涉及代码位置 | 做什么 |
|------|------|-------------|--------|
| P-5 | 原子写入 | `src/kernel/persistence.rs` | 新增 `atomic_write_json()`，迁移所有 `fs::write(path, json)` |
| P-2 | CheckpointStore 持久化 | `src/kernel/ops/checkpoint.rs` | 新增 `persist()` / `restore()`，索引 + CAS 存储 |
| P-3 | TenantStore 持久化 | `src/kernel/ops/tenant.rs` | 新增 `persist()` / `restore()`，`tenant_index.json` |
| P-4 | AgentKeyStore 持久化 | `src/api/agent_auth.rs` | `new()` → `open(root)`，secret 文件 0600 权限 |
| P-1 | Shutdown Hook | `src/bin/plicod.rs` | SIGTERM/SIGINT → `kernel.persist_all()` |
| P-6 | 周期性持久化 | `src/bin/plicod.rs` | 每 5 分钟 `kernel.persist_all()` |

### Phase A/B/C/D 修正项

| 编号 | 修正项 | 涉及代码位置 | 做什么 |
|------|--------|-------------|--------|
| C-1 | DeltaSince seq-based | `src/kernel/ops/delta.rs` | 参数用 `since_seq: u64`，调 `event_bus.events_since(since_seq)` |
| C-2 | ChangeEntry 不依赖 LLM | `src/kernel/ops/delta.rs` | summary = 事件元数据拼接，有 Summarizer 时可选增强 |
| C-3 | F-9 双路匹配 | `src/kernel/ops/prefetch.rs` | 检测 embedding provider 是否为 stub，stub 走精确匹配 |
| C-4 | F-9 内存限制 | `src/kernel/ops/prefetch.rs` | `max_entries=64, max_memory_bytes=32MB` |
| C-5 | F-6 session 超时 | `src/kernel/ops/session.rs` | SessionStore 定期扫描，超 TTL 自动 EndSession |
| C-6 | F-6 load_tiers 类型 | `src/api/semantic.rs` | 用 `Vec<MemoryTier>` 而非 `Vec<String>` |
| C-7 | F-6 EndSession 返回 last_seq | `src/api/semantic.rs` | `SessionEnded { checkpoint_id, last_seq }` |
| C-8 | F-10 tag key 策略 | `src/kernel/ops/session.rs` | `IntentKeyStrategy` 枚举，无 embedding 时用 TagExtraction |
| C-9 | F-10 自动禁用 | `src/kernel/ops/prefetch.rs` | stub + 无 tag → F-10 静默禁用，不影响其他功能 |

---

*文档状态：指导性设计文档（Tier B），含合理性审计。⚠️ 标记项在开发时必须先修正再实现。实现细节以 Tier A（AGENTS.md + INDEX.md + 代码）为准。*
