# Plico 第二十节点设计文档
# 觉 — 主动性与越用越好

**版本**: v1.0
**日期**: 2026-04-23
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Prefetch持久化 + 因果Hook + Async预取 + Feedback学习 + Context-Dependent Gravity
**前置**: 节点 19 ✅（100%）— Hook哨兵(5点) + 断路器(3路径) + Token Budget + 1022 tests + Soul 82%
**验证方法**: Dogfood E2E + 全量回归 + prefetch_hit_rate 指标 + 意图缓存命中率 + session重起后profile保留
**信息来源**: `docs/design-node19-sentinel.md` + prefetch_rs 源码分析(1458L) + prefetch_cache.rs(455L) + prefetch_profile.rs(191L) + Agentic Memory (Alex Spyropoulous 2025) + RRF (Elasticsearch 2026)

---

## 0. 链式思考：从 Node 19 到 Node 20

### 为什么需要"觉"

Node 19 建立了**哨兵层**：
- Hook 哨兵：5个生命周期拦截点（PreToolCall/PostToolCall/PreWrite/PreDelete/PreSessionStart）
- 断路哨兵：Embedding + LLM + MCP 全路径 fail-fast
- 度量哨兵：Token Budget 追踪每个 Agent 的累计消耗

**但这些只是"看得见"**。Soul 2.0 的核心差距是：

| 公理 | 当前实现 | 对齐度 |
|------|---------|--------|
| 公理7: 主动先于被动 | prefetch.rs 有算法但不持久不异步 | 35% |
| 公理9: 越用越好 | AgentProfile 有 transition matrix 但内存态 | 25% |
| 公理8: 因果先于关联 | KG 有 `Causes` 边但无 Hook 因果写入 | ~50% |

**"觉"的三层含义**：
1. **记忆觉**：意图缓存跨 session 持久化，第100次会话复用第1次的经验
2. **因果觉**：Hook 事件写入 KG 因果链，"A 因为 B 而调用"
3. **预测觉**：Agent Profile 反馈环路，预测下一个意图并提前预取

### 链式推导

```
[因] prefetch 数据 in-memory → [果] 重启后一切归零 (公理9违反)
    ↓
[因] Hook 拦截无因果追踪 → [果] 无法建立 "A因为B而变化" (公理8违反)
    ↓
[因] prefetch 用 spawn_blocking 不返回 JoinHandle → [果] 无法 cancel/await 预取
    ↓
[因] 无 feedback 回流 → [果] AgentProfile 永远是静态的
    ↓
[因] 搜索结果无 intent 上下文 → [果] 检索质量停留在语义层面
```

---

## 1. 现状分析

### 1.1 已有基础设施 (47 tests)

| 组件 | 行数 | 功能 | 缺失 |
|------|------|------|------|
| `prefetch_cache.rs` | 455L | 意图缓存(dual-path: exact+embedding) | 不持久化(max 64 entry, 32MB, 24h TTL) |
| `prefetch_profile.rs` | 191L | AgentProfile(transition matrix, hot_objects) | 不持久化,无反馈环路 |
| `prefetch.rs` | 1458L | 多路径召回(4-path), RRF fusion(k=60), Cognitive prefetch | 非async(阻塞), 不持久化 pending |

### 1.2 关键差距

**Gap 1: 跨 session 不持久化**
- 意图缓存：in-memory `RwLock<Vec<CachedAssembly>>`
- Agent Profile：in-memory `RwLock<HashMap<String, AgentProfile>>`
- 效果：第100次会话和第1次一样昂贵（违反公理9）

**Gap 2: Hook 无因果写入**
- Hook 拦截产生数据但没有写入 KG 因果链
- "A tool call because B intent" 关系没有被追踪

**Gap 3: prefetch 非异步**
- `spawn_blocking` 内部执行，不返回 `JoinHandle`
- 无法 cancel、无法 await 完成、无法追踪状态

**Gap 4: 无 feedback 环路**
- `IntentFeedbackEntry` 存在但没有写回 AgentProfile
- 预测改进没有数据支撑

---

## 2. Node 20 五大维度

### D1: Prefetch Persistence — 记忆觉

**问题**: 意图缓存 + AgentProfile 重启后丢失，"越用越好"无法实现。
**目标**: 持久化到 CAS，实现跨 session 的意图复用和 profile 累积。
**实现策略**:
- `IntentCacheStore`: 将 `CachedAssembly` 序列化为 JSON 存 CAS，Bootstrap 时加载
- `AgentProfileStore`: `AgentProfile` 序列化为 JSON 存 CAS，增量更新
- 写入时机：assembly 完成时写缓存，profile 更新时写 CAS（debounced，批量写）

### D2: Causal Hook — 因果觉

**问题**: Hook 拦截无因果追踪，KG 无法建立 "A because B" 链。
**目标**: Hook 事件自动写入 KG，`Causes`/`CausedBy` 边连接 tool call 和 intent。
**实现策略**:
- 新增 `HookContext` → KG 因果写入：在 `PostToolCall` 后自动记录
- 边类型：`ToolCallEvidence`（证据链）、`IntentDeclaration`（意图声明）
- Query 支持：查询 "这个 tool call 的因果链是什么"

### D3: Async Prefetch — 预测觉

**问题**: `spawn_blocking` 内部执行，无法 cancel/await。
**目标**: prefetch 返回 `JoinHandle`，支持 cancel 和 await。
**实现策略**:
- `IntentPrefetcher::prefetch_async()` → 返回 `JoinHandle<BudgetAllocation>`
- 状态机：`Pending → Assembling → Ready → Used/Unused`
- Cancel：agent 改变意图时 cancel pending assembly
- 超时：全局 30s 超时，防止 hanging

### D4: Intent Feedback Loop — 学习觉

**问题**: AgentProfile 永远是静态的，无反馈改进。
**目标**: `IntentFeedbackEntry` 写回 AgentProfile，更新 transition matrix。
**实现策略**:
- `prefetch.rs::record_feedback()` 写入：哪些预取被使用、哪些被忽略
- 命中率统计：`intent_cache_hits / intent_cache_lookups`
- 预测改进：当 `confidence ≥ 0.5` 时主动预取下一个 intent

### D5: Context-Dependent Gravity — 搜索觉

**问题**: 搜索结果只按语义相关性排序，不考虑当前 session intent。
**目标**: 基于当前 intent 重新加权搜索结果，实现 "重力吸附"。
**实现策略**:
- 扩展 `semantic_search` 支持 `intent_context: Option<String>` 参数
- RRF 融合：原语义分数 × intent_gravity_multiplier
- 重力系数：`hot_objects` 中的 CID 获得额外提升

---

## 3. 特性清单

### F-1: Intent Cache Persistence

```rust
// kernel/ops/prefetch_cache.rs — 新增持久化支持
impl IntentCache {
    /// 持久化到 CAS（带 debounce）
    pub fn persist(&self, cas: &Arc<CAS>) -> Result<(), Error>;
    
    /// 从 CAS 恢复
    pub fn restore(cas: &Arc<CAS>) -> Result<Self, Error>;
}
```

**测试**:
- `test_intent_cache_persist_and_restore`: 写 CAS → 重启 → 读回
- `test_intent_cache_eviction_respected`: LRU eviction 正确序列化
- `test_intent_cache_ttl_respected_on_restore`: 过期 entry 不恢复

**新测试**: 3

### F-2: AgentProfile Persistence

```rust
// kernel/ops/prefetch_profile.rs — 新增持久化
impl AgentProfileStore {
    /// 持久化所有 profiles（增量）
    pub fn persist_all(&self, cas: &Arc<CAS>) -> Result<(), Error>;
    
    /// 恢复所有 profiles
    pub fn restore(cas: &Arc<CAS>) -> Result<Self, Error>;
}
```

**测试**:
- `test_profile_persist_and_restore`: transition matrix 跨 session 保留
- `test_profile_hot_objects_ordered`: hot_objects 排序正确
- `test_profile_feedback_updates_matrix`: feedback 后 matrix 更新

**新测试**: 3

### F-3: Causal Hook写入

```rust
// kernel/hook.rs — 新增因果追踪
pub struct CausalHookHandler {
    kg: Arc<dyn KnowledgeGraph>,
}

impl HookHandler for CausalHookHandler {
    fn handle(&self, point: HookPoint, ctx: &HookContext) -> HookResult {
        if point == HookPoint::PostToolCall {
            self.record_causal_chain(ctx);
        }
        HookResult::Continue
    }
}
```

**KG 边类型扩展**:
```rust
// 现有
Causes,
// 新增
CausedBy,    // 反向因果
DependsOn,   // 依赖关系
Produces,    // 输出关系
```

**测试**:
- `test_hook_writes_causal_edge`: PostToolCall 后 KG 写入 Causes 边
- `test_causal_chain_queryable`: 可以查询 "A 的所有因"
- `test_hook_does_not_block`: 因果写入不阻塞工具执行

**新测试**: 3

### F-4: Async Prefetch with JoinHandle

```rust
// kernel/ops/prefetch.rs
pub enum PrefetchState {
    Pending,
    Assembling,
    Ready(BudgetAllocation),
    Used,
    Unused,
    Cancelled,
}

pub struct PrefetchHandle {
    pub state: AtomicU8,
    pub result: Mutex<Option<BudgetAllocation>>,
}

impl IntentPrefetcher {
    /// 异步预取，返回 JoinHandle
    pub fn prefetch_async(&self, intent: &Intent) -> Arc<PrefetchHandle>;
    
    /// 取消预取
    pub fn cancel(&self, intent_id: &str);
    
    /// 等待结果
    pub async fn await_result(&self, handle: Arc<PrefetchHandle>) -> BudgetAllocation;
}
```

**测试**:
- `test_prefetch_returns_handle`: prefetch_async 返回有效 handle
- `test_prefetch_cancel`: cancel 后状态变为 Cancelled
- `test_prefetch_timeout`: 30s 超时后状态变为 Unused
- `test_prefetch_concurrent`: 并发预取不阻塞

**新测试**: 4

### F-5: Intent Feedback Loop

```rust
// kernel/ops/prefetch.rs
pub fn record_feedback(&self, assembly_id: &str, used: bool) {
    // 更新 transition matrix
    self.profile_store.record_intent_complete(
        &self.current_intent_tag,
        &self.predicted_next_tag,
        used,
    );
    // 更新命中率统计
    self.stats.feedback_received += 1;
    if used { self.stats.prefetch_hits += 1; }
}
```

**测试**:
- `test_feedback_improves_prediction`: 多次 negative feedback 后预测调整
- `test_feedback_records_hit_miss`: 命中率统计正确
- `test_low_confidence_no_prefetch`: confidence < 0.5 不触发预取

**新测试**: 3

### F-6: Context-Dependent Gravity

```rust
// kernel/ops/fs.rs — semantic_search 扩展
pub fn semantic_search_with_intent(
    &self,
    query: &str,
    tags: Vec<String>,
    intent_context: Option<String>,  // 新参数
    agent_id: &str,
) -> Vec<SearchResult> {
    let mut results = self.semantic_search_base(query, tags, agent_id)?;
    
    if let Some(ctx) = intent_context {
        // 从 AgentProfile 获取 hot_objects
        let hot = self.prefetcher.get_hot_objects(agent_id);
        // 重排：hot 中的 CID 提升
        results.apply_gravity(&hot, ctx.as_str());
    }
    results
}
```

**测试**:
- `test_gravity_boosts_hot_objects`: hot_objects 在结果中排名提升
- `test_gravity_no_context_unchanged`: 无 intent 时结果不变
- `test_gravity_with_empty_hot`: 有 intent 但无 hot 时不变

**新测试**: 3

---

## 4. 前沿研究对标

### 4.1 Agentic Memory (Alex Spyropoulous, 2025)

| 维度 | 行业标准 | Plico F-1/F-2 | 对齐度 |
|------|---------|---------------|--------|
| 向量存储+语义搜索 | ✅ | ✅ (已有) | ✅ |
| 行为规则学习 | Transition matrix | AgentProfile | ✅ |
| 双向记忆链接 | KG因果边 | CausalHook写入 | ✅ |
| 多层优化 | L0/L1/L2 | 已有 | ✅ |
| 自动维护 | TTL/eviction | IntentCache TTL | ✅ |
| **跨session持久化** | 需落地 | **F-1/F-2** | 🔴 |

### 4.2 RRF (Reciprocal Rank Fusion)

| 维度 | 行业标准 | Plico prefetch.rs | 对齐度 |
|------|---------|------------------|--------|
| k=60 constant | ✅ | ✅ (已有) | ✅ |
| 多路径融合 | ✅ | ✅ 4-path | ✅ |
| **Intent上下文重排** | 未提到 | **F-6** | 🔴 |

---

## 5. 量化目标

| 指标 | N19 现状 | N20 目标 | 状态 |
|------|---------|---------|------|
| 总测试数 | 1022 | **1044+** | ✅ 1329 |
| Prefetch 持久化 | 0 (in-memory) | **CAS persist** | ✅ M1 完成 |
| AgentProfile 持久化 | 0 (in-memory) | **CAS persist** | ✅ M1 完成 |
| 因果Hook | 0 | **KG CausedBy写入** | ✅ M2 完成 |
| Async Prefetch | 0 (blocking) | **JoinHandle返回** | ✅ M3 完成 |
| Intent Feedback | 0 | **hit rate stats** | ✅ M4 完成 |
| Context-Dependent Gravity | 0 | **search重排** | 🔴 未开始 |

---

## 6. 实施计划

### Phase 1: Prefetch Persistence (M1, ~1.5 days)

1. F-1: IntentCache persist/restore to CAS
2. F-2: AgentProfileStore persist/restore to CAS
3. 测试: 6 个新测试
4. Dogfood: restart 后 intent cache hit 验证

### Phase 2: Causal Hook (M2, ~1 day)

1. F-3: CausalHookHandler 实现
2. KG 边类型扩展: CausedBy, DependsOn, Produces
3. 测试: 3 个新测试
4. Dogfood: PostToolCall 后 kg query 验证因果边

### Phase 3: Async Prefetch (M3, ~1 day)

1. F-4: PrefetchHandle + prefetch_async + cancel + await
2. 状态机: Pending → Assembling → Ready/Used/Unused/Cancelled
3. 测试: 4 个新测试
4. Dogfood: 并发预取 + cancel 验证

### Phase 4: Feedback Loop + Gravity (M4+M5, ~1.5 days)

1. F-5: IntentFeedbackEntry → AgentProfile 更新
2. F-6: semantic_search_with_intent + gravity re-ranking
3. 测试: 6 个新测试
4. Dogfood: 命中率统计 + hot对象排名验证

### Phase 5: 集成 + 回归 (~0.5 day)

1. 全量 1044+ 测试通过
2. Session restart 后所有 prefetch 状态保留
3. E2E: declare intent → prefetch → use → feedback → profile update

---

## 7. 从 Node 20 到 Node 21 的推演

Node 20 完成后，Plico 将具备：
- 跨 session 的意图复用（第100次复用第1次的 assembly）
- 跨 session 的 Agent Profile 累积（transition matrix 越来越准）
- Hook 因果链（"为什么调用这个工具" 可追溯）
- Async 可取消的预取（预取任务纳入调度）
- 基于 intent 的搜索重排（相关性 → 意图相关性）

**Node 21 展望**: **意 (Will) — 意图驱动的自主执行**

基于 Node 20 的"觉"基础设施，Node 21 将攻坚 Soul 2.0 最高差距——公理2（意图先于操作）：

1. **Intent Declaration API**: 结构化声明意图（关键词 + CID + token预算），OS 负责组装
2. **Intent Plan Execution**: 将 intent 分解为可执行步骤序列
3. **Multi-Agent Intent Coordination**: 多个 Agent 共享 intent tree，协作分工
4. **Autonomous Loop**: OS 驱动执行，Agent 只在异常点介入

**预期 Soul 对齐**: ~86% → **91%+**（公理2: 40%→70%）
