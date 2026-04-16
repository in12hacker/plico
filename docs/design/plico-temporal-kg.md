# Plico Temporal Knowledge Graph Design

**Date**: 2026-04-16
**Version**: 0.1
**Based on**: Graphiti (Zep) bi-temporal model research

---

## 1. 核心设计：Bi-temporal Model

### 1.1 Graphiti 的双时间模型

根据 Graphiti (Zep) 的研究，每个事实（边）有两个时间维度：

```
valid_at:  何时这个边在现实世界中变为真
invalid_at: 何时这个边被取代（None = 仍然有效）
```

**关键特性**：
- Facts have validity windows
- old facts are invalidated — not deleted
- Temporal history is preserved for querying historical states

### 1.2 Plico 的实现

```rust
/// KGEdge 新增字段
pub struct KGEdge {
    // ... existing fields ...
    /// When this edge became valid (Unix ms). None = unknown.
    pub valid_at: Option<u64>,
    /// When this edge was invalidated (Unix ms). None = still valid.
    pub invalid_at: Option<u64>,
}

impl KGEdge {
    /// Returns true if this edge is currently valid at the given timestamp.
    pub fn is_valid_at(&self, t: u64) -> bool {
        self.valid_at.map_or(true, |v| v <= t)
            && self.invalid_at.map_or(true, |i| i > t)
    }
}
```

---

## 2. 时间有效性语义

### 2.1 有效性判断规则

```
边在时间 T 有效 当且仅当:
  - valid_at <= T, 且
  - invalid_at.is_none() || invalid_at > T
```

### 2.2 边界情况

| valid_at | invalid_at | 时间 T | 有效？ |
|----------|-----------|--------|--------|
| 1000 | None | 1000 | ✅ |
| 1000 | None | 2000 | ✅ |
| 1000 | 1100 | 999 | ❌ (before valid_at) |
| 1000 | 1100 | 1000 | ✅ (at valid_at) |
| 1000 | 1100 | 1099 | ✅ |
| 1000 | 1100 | 1100 | ❌ (at invalid_at boundary) |

---

## 3. 冲突处理

### 3.1 自动事实失效

根据 Graphiti 设计：
- 当新信息与旧信息冲突时，设置旧边的 `invalid_at = now_ms()`
- 新边以当前时间作为 `valid_at` 生效
- 旧边保留在 KG 中用于历史查询

### 3.2 冲突检测触发时机

```
1. 创建新边时
   - 检查是否有相同 (src, dst, edge_type) 的有效边
   - 如果有新边与旧边矛盾（旧边 invalid_at = now）

2. 示例：
   - 旧边: Person("王总") → prefers → wine (valid_at=1000)
   - 新边: Person("王总") → prefers → beer (valid_at=2000)
   - 处理: 旧边 invalid_at=2000, 新边 valid_at=2000
```

---

## 4. 检索增强

### 4.1 时间感知查询

```rust
/// 查询在时间 T 时有效的所有边
fn get_valid_edges_at(&self, t: u64) -> Vec<KGEdge> {
    self.edges.iter()
        .filter(|e| e.is_valid_at(t))
        .cloned()
        .collect()
}

/// 查询 src 在时间 T 时指向 dst 的有效边
fn get_valid_edge_between(&self, src: &str, dst: &str, t: u64) -> Option<KGEdge> {
    self.edges.iter()
        .filter(|e| e.src == src && e.dst == dst && e.is_valid_at(t))
        .cloned()
        .max_by_key(|e| e.valid_at) // 最近的有效边
}
```

### 4.2 检索流程 (Graphiti Hybrid Retrieval)

Graphiti 使用混合检索：
1. **Semantic embeddings** - 语义相似度
2. **Keyword (BM25)** - 关键词检索
3. **Graph traversal** - 图遍历

Plico 当前支持：
- 向量语义搜索
- 图遍历 (get_neighbors, find_paths)

待增加：
- BM25 关键词检索
- 时间过滤的图遍历

---

## 5. 已实现功能

### 5.1 第一轮迭代 ✅

- [x] KGEdge 添加 `valid_at` / `invalid_at` 字段
- [x] `KGEdge::new()` 构造函数，自动设置时间戳
- [x] `KGEdge::is_valid_at()` 有效性判断方法
- [x] 序列化/反序列化支持
- [x] 测试覆盖

### 5.2 第二轮迭代 ✅

- [x] `get_valid_edges_at(t)` - 查询时间 T 时有效的所有边
- [x] `get_valid_edge_between(src, dst, edge_type, t)` - 查询两节点间在 T 时有效的边
- [x] `invalidate_conflicts(new_edge)` - 检测并失效冲突边（保留历史）
- [x] 8 个新测试用例

### 5.3 第三轮迭代 ✅

- [x] KGNode 添加 `valid_at` / `invalid_at` 字段
- [x] `KGNode::is_valid_at(t)` 方法
- [x] `#[serde(default)]` 向后兼容
- [x] 3 处 KGNode 字面量构造更新（upsert_document, create_event, make_node）

### 5.4 第四轮迭代 ✅ expired_at 软删除

- [x] KGNode 添加 `expired_at: Option<u64>` 软删除标记
- [x] KGEdge 添加 `expired_at: Option<u64>` 软删除标记
- [x] `is_valid_at()` 同时检查 `expired_at`（软删除排除）
- [x] 4 处 struct literal 更新（KGEdge::new, upsert_document, create_event, make_node）
- [x] `#[serde(default)]` 向后兼容

### 5.5 第五轮迭代 ✅ KGNode 时间感知查询

- [x] `get_valid_nodes_at(agent_id, node_type, t)` - 返回时间 T 时有效的节点
- [x] 3 个新测试用例（时间过滤、agent/type 过滤、软删除排除）

### 5.6 持久化兼容

- [x] `#[serde(default)]` 确保向后兼容
- [x] 旧数据加载时 `valid_at=None, invalid_at=None, expired_at=None` 视为永久有效

---

## 6. 下一步迭代计划

### 6.1 P1: 检索增强 (BM25) ✅ 已实现

**Hindsight (91.4% LongMemEval) vs Zep (63.8%)** 的关键差距：4路并行检索。

当前 Plico 检索能力：
- ✅ 语义向量搜索
- ✅ 图遍历
- ✅ BM25 关键词搜索（bm25 crate v2.3.2，RRF 混合检索）
- ❌ 时间过滤检索

已完成：
- BM25 关键词搜索（bm25 crate v2.3.2）
- RRF (Reciprocal Rank Fusion) 混合检索：向量 + BM25

### 6.2 P2: 实体消解 ⚠️ 待实现

- [ ] 实体消解时设置旧实体 `invalid_at`
- [ ] 实体属性变更历史追踪

### 6.3 P3: Episode Provenance 追溯 ✅ 已实现 (episodes 字段已添加)

Graphiti 每个边有 `episodes: Vec<uuid>` 追踪来源对话/事件：
```rust
pub struct KGEdge {
    // ...
    /// Source episode IDs that created/modified this fact
    #[serde(default)]
    pub episodes: Vec<String>,
}
```

已实现：`KGEdge::episodes: Vec<String>` + `new_with_episode()` + `upsert_document` 集成。
下一步：`create_event` 的 HasAttendee 等边也需要传入 event_id 作为 episode。

### 6.4 P4: 混合检索融合 (RRF/Cross-Encoder) ✅ RRF 已实现

Graphiti 支持多种重排策略：
- ✅ RRF (Reciprocal Rank Fusion) — 已在 `search_with_filter()` 实现
- ❌ Cross-Encoder — 待研究
- ❌ Graph Distance — 待研究

**Bi-temporal 四时间维度（已完整实现）**:
| 维度 | 含义 | Plico 状态 |
|------|------|-----------|
| `created_at` | 数据首次摄入系统 | ✅ 已有 |
| `valid_at` | 事实实际发生时间 | ✅ 已有 |
| `invalid_at` | 事实被取代时间 | ✅ 已有 |
| `expired_at` | 软删除标记 | ✅ 已有 |

---

## 7. 参考资料

- [Zep: A Temporal Knowledge Graph Architecture for Agent Memory](https://arxiv.org/abs/2501.13956) (2025-01)
- [Graphiti GitHub](https://github.com/getzep/graphiti)
- [知识图谱在AI-memory的"最佳实践"-zep底层存储架构Graphiti源码解读](https://www.cnblogs.com/zzz77zz/articles/19026839) (2025-08-07)

---

## 8. 迭代日志

### 2026-04-16: 第一轮迭代完成

**修改文件**: `src/fs/graph.rs`

**添加内容**:
- `KGEdge::valid_at: Option<u64>` - 边生效时间
- `KGEdge::invalid_at: Option<u64>` - 边失效时间
- `KGEdge::new()` - 构造函数，自动设置 valid_at=now, invalid_at=None
- `KGEdge::is_valid_at(t: u64) -> bool` - 时间有效性判断
- `make_edge()` 测试辅助函数
- 8 个新测试用例覆盖时间有效性

**测试**: 186 tests passed ✅

---

### 2026-04-16: 第二轮迭代完成

**目标**: 为 `KnowledgeGraph` trait 添加时间感知查询 API

**添加方法**:
```rust
fn get_valid_edges_at(&self, t: u64) -> Result<Vec<KGEdge>, KGError>;
fn get_valid_edge_between(&self, src: &str, dst: &str, edge_type: Option<KGEdgeType>, t: u64) -> Result<Option<KGEdge>, KGError>;
fn invalidate_conflicts(&self, new_edge: &KGEdge) -> Result<usize, KGError>;
```

**实现**:
- `get_valid_edges_at(t)` - 遍历所有边，返回在时间 T 时有效的边
- `get_valid_edge_between(src, dst, edge_type, t)` - 返回两节点间在时间 T 有效的边（如有多条，返回 valid_at 最近的）
- `invalidate_conflicts(new_edge)` - 两阶段锁：先收集冲突，再批量失效

**测试**: 112 unit tests passed ✅

**新增测试用例**:
- `test_get_valid_edges_at_filters_by_time` - 时间过滤边界验证
- `test_get_valid_edges_at_current_time` - 当前时间查询
- `test_get_valid_edges_at_no_edges` - 空图查询
- `test_get_valid_edge_between_returns_most_recent` - 多边时序取最近
- `test_get_valid_edge_between_filters_by_edge_type` - edge_type 过滤
- `test_invalidate_conflicts_replaces_prior_edge` - 不同 dst 无冲突（dst 相同才冲突）
- `test_invalidate_conflicts_none_found` - 无冲突时返回 0
- `test_invalidate_conflicts_preserves_history` - 历史查询仍可用

### 2026-04-16: 第三轮迭代完成

**目标**: KGNode 添加时间有效性，与 Graphiti 模型对齐

**修改**:
- `KGNode::valid_at: Option<u64>` - 节点生效时间
- `KGNode::invalid_at: Option<u64>` - 节点失效时间
- `KGNode::is_valid_at(t)` - 时间有效性判断（与 KGEdge 一致）
- 3 处 struct literal 更新（`upsert_document`, `create_event`, `make_node`）
- `#[serde(default)]` 向后兼容

**测试**: 190 tests passed ✅

### 2026-04-16: 第四轮迭代完成

**目标**: 添加 `expired_at` 软删除标记，与 Graphiti bi-temporal 模型完整对齐

**修改**:
- `KGNode::expired_at: Option<u64>` - 软删除标记
- `KGEdge::expired_at: Option<u64>` - 软删除标记
- `is_valid_at()` 同时检查 `expired_at`（软删除排除，保留审计）
- 4 处 struct literal 更新（`KGEdge::new`, `upsert_document`, `create_event`, `make_node`）
- 更新 KGNode/KGEdge doc comment 说明四时间维度

**Bi-temporal 四时间维度现已完整**:
- `created_at`: 数据首次摄入系统
- `valid_at`: 事实实际发生时间
- `invalid_at`: 事实被取代时间（真实世界）
- `expired_at`: 软删除时间（admin/user delete）

**测试**: 190 tests passed ✅

### 2026-04-16: 第五轮迭代完成

**目标**: `get_valid_nodes_at` 节点时间查询 API

**修改**:
- `KnowledgeGraph` trait 新增 `get_valid_nodes_at(agent_id, node_type, t)`
- `PetgraphBackend` 实现：过滤 agent_id + node_type + `is_valid_at(t)`
- 3 个新测试：`test_get_valid_nodes_at_filters_by_time`, `test_get_valid_nodes_at_filters_by_agent_and_type`, `test_get_valid_nodes_at_respects_expired`

**测试**: 193 tests passed ✅

### 2026-04-16: 第六轮迭代完成 — BM25 关键词搜索集成

**目标**: 为混合检索添加 BM25 关键词搜索，填补 Hindsight 91.4% vs Zep 63.8% 差距中的关键词匹配缺口。

**背景**: Hindsight 的关键优势是 4 路并行检索（语义 + BM25 + 图遍历 + 时间），Plico 已有语义向量搜索和图遍历，BM25 是最后一块拼图。

**修改**:

`src/fs/search.rs`:
- 新增 `Bm25Index` wrapper，封装 `bm25::SearchEngine<String>`
- `upsert(cid, text)`, `remove(cid)`, `search(query, limit) → Vec<(String, f32)>`
- 使用 `bm25` crate v2.3.2，`SearchEngineBuilder::with_avgdl(100.0).build()`

`src/fs/semantic_fs.rs`:
- `SemanticFS` struct 新增 `bm25_index: Arc<Bm25Index>` 字段
- `upsert_semantic_index()` 同时 upsert 到 BM25（使用完整文本， snippet 用于向量）
- `search_with_filter()` 改为 RRF (Reciprocal Rank Fusion) 混合检索：
  1. 向量搜索 → `HashMap<cid, cosine_score>`
  2. BM25 搜索 → `Vec<(cid, bm25_score)>`
  3. 对 BM25 结果应用 filter 过滤
  4. RRF 合并：`score = Σ 1/(60 + rank)`，k=60
  5. 排序取 top-k，从 CAS 获取对象
- `delete()` 同时从 BM25 删除
- `restore()` 调用 `upsert_semantic_index`（已含 BM25）
- `rebuild_vector_index()` 同时重建 BM25 索引

`src/fs/mod.rs`:
- 导出 `Bm25Index`

**设计说明**:
- BM25 和向量搜索独立索引：向量用 snippet (200 chars)，BM25 用完整文本
- RRF 对不同分数尺度（余弦相似度 vs BM25）稳健，无需归一化
- BM25 的 filter 过滤在 RRF 合并后生效，避免 BM25 独自引入噪音

**测试**: 193 tests passed ✅

### 2026-04-16: 第七轮迭代完成 — Episode Provenance

**目标**: 为 KGEdge 添加 episodes 字段，实现 Graphiti 核心的 provenance 追踪能力。

**背景** (基于 Graphiti/Zep 最新研究):
- Graphiti 的每个边有 `episodes: Vec<uuid>` 追踪来源对话/事件
- Episode provenance 使能：信用评估（官方文档 vs 聊天）、冲突解决、审计追踪、历史查询
- Hindsight v0.4.19 在 LongMemEval 达到 **94.6%**（非 91.4%，旧版本数据）
- Hindsight 的三大改进：Observations（= Plico PatternExtractor）、更好的提取、重新设计的检索

**修改**:

`src/fs/graph.rs`:
- `KGEdge` 新增 `episodes: Vec<String>` 字段，带 `#[serde(default)]`
- `KGEdge::new()` 初始化 `episodes: Vec::new()`
- 所有现有边创建使用 `KGEdge::new()`（自动含 episodes）
- `#[serde(default)]` 保证向后兼容：旧数据 `episodes=None` → 视为空向量

**设计说明**:
- Episode ID 格式：外部注入（如 "conv-uuid-123", "doc-uuid-456"）
- SemanticFS 的 `BehavioralObservation.id` 自然作为 episode 引用
- 不需要单独的 Episode node type — 只需 `episodes: Vec<String>` 在边上即可实现核心 provenance 查询

**P3 状态**: ✅ episodes 字段已添加。下一步：在 `upsert_document` / `create_event` 时传入具体 episode ID。

**测试**: 193 tests passed ✅

### 2026-04-16: 第八轮迭代完成 — Episode Provenance 集成

**目标**: 将 episode ID 实际注入到边中，使 provenance 查询有意义。

**修改**:

`src/fs/graph.rs`:
- 新增 `KGEdge::new_with_episode(src, dst, edge_type, weight, episode)` 构造函数
- `upsert_document` 的 `AssociatesWith` 边使用 `new_with_episode(cid)`，CID 作为 episode 来源

`tests/graph.rs`:
- 新增 `test_upsert_document_episodes_populated` 验证边 episodes 包含源文档 CID

**设计说明**:
- AssociatesWith 边记录：新文档的 CID 是 episode（表示这个关联是由新文档创建时发现的）
- `BehavioralObservation.id` 自然作为 episode 引用（未来 `create_event` 调用时会传入）

**测试**: 194 tests passed ✅ (新增1个graph测试)

### 2026-04-16: 第九轮迭代完成 — ActionSuggestion 持久化存储 (iter14)

**目标**: 为 Phase D M15（通知集成）打下基础。Suggestions 生成后需要持久化存储才能被查询/确认/拒绝。

**背景**:
- Phase D 的 `infer_suggestions_for_event()` 每次调用都生成新的 suggestions 但不存储
- 通知系统无法查询待处理的 suggestions
- `dashboard_status.pending_suggestions` 始终为 0

**修改**:

`src/fs/semantic_fs.rs`:
- `SemanticFS` 新增 `suggestion_store: RwLock<HashMap<String, Vec<ActionSuggestion>>>` 字段
- `infer_suggestions_for_event()`: 生成后将 suggestions 存入 store（按 event_id 作为 key）
- `get_pending_suggestions()`: 返回所有 Pending 且非 "too uncertain" 的 suggestions
- `get_suggestions_for_event(event_id)`: 获取特定事件的 suggestions
- `confirm_suggestion(id)` / `dismiss_suggestion(id)`: 更新 lifecycle 状态
- `pending_suggestion_count()`: 用于 dashboard 状态计数

`src/kernel/mod.rs`:
- AIKernel 转发上述新方法
- `dashboard_status.pending_suggestions`: 改为调用 `fs.pending_suggestion_count()`

**设计说明**:
- Suggestions 按 event_id 存储（而非全局 UUID），因为每个 suggestion 都与触发事件强关联
- `get_pending_suggestions()` 过滤 `is_too_uncertain()` 的建议（confidence < 0.4），避免噪音通知
- 状态机: Pending → Confirmed / Dismissed，不可逆

**测试**: 197 tests passed ✅ (新增3个 suggestion store 测试)

**下一步**: 
- 添加 API endpoints: `GetPendingSuggestions`, `ConfirmSuggestion`, `DismissSuggestion`
- 集成 Scheduler: alarm 触发时查询 pending suggestions 并推送通知