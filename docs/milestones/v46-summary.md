# v46 里程碑总结：Extreme Recall & Memory Fusion

**日期**：2026-05-13
**范围**：检索质量优化、多跳推理增强、端到端质量提升

---

## 质量基线

- **测试**：2,075 lib + 12 perf regression + integration
- **覆盖率**：87.02%（cargo-llvm-cov --lib）
- **Clippy**：0 新增警告（预存 12 个非本次引入）
- **性能回归**：12/12 通过

## Benchmark 结果

| 指标 | 值 |
|------|-----|
| CAS write QPS | 32.8 |
| Search QPS | 7.0 |
| Memory recall QPS | 3135.8 |
| LoCoMo F1 | 0.364（基线，待验证提升） |

## Phase 1：性能优化

1. **EmbeddingCache 接入**：`CachingEmbeddingProvider` 包装 embedding provider，重复查询跳过 ~80ms HTTP 调用
2. **SearchCache 接入**：`SearchCache` 接入 `SemanticFS::search_with_filter`，TTL 5 分钟，重复查询 μs 级返回
3. **HNSW 阈值调整**：binary_index 两阶段搜索阈值 100→1000，小数据集直接走 usearch O(log n)
4. **冗余 CAS 读取消除**：`vector_hits` 改存 `SearchHit`，BM25 chunk boost 优先用 vector meta
5. **条件化 PPR**：`is_multihop_query()` 检测多跳查询，简单事实查询跳过 PPR 节省 10-30ms

## Phase 2：多跳推理增强

1. **Query Decomposition**：`src/fs/query_decompose.rs` — 规则 + LLM 查询分解
   - 支持因果/关系/链式三种分解模式
   - 中文实体提取：CJK 滑动窗口 + 跳过词表 + 单字助词过滤
   - LLM 分解：`decomposition_prompt()` + `parse_llm_decomposition()`
   - 14 个单元测试覆盖
2. **Path Discovery 接入检索流水线**：`SemanticFS::discover_and_inject_paths()`
   - MultiHop 查询时，top-K RRF 候选映射 KG 节点
   - `get_nodes_by_cid()` — 新增 trait 方法，PetgraphBackend 用 cid_refs 索引 O(1) 查找
   - `find_weighted_path()` 对种子节点对发现加权路径
   - 路径上的 CID 注入 RRF 分数（boost 0.1-0.15）
3. **find_paths 修复**：per-path visited 集替代全局 visited，允许不同路径经过同一节点
4. **find_weighted_path 修复**：双向边遍历（出边 + 入边），支持反向关系路径发现
5. **MultiHop top_k 提升**：15 → 30（RetrievalConfig::for_intent）

## Phase 3：端到端质量提升

1. **迭代检索**：`SemanticFS::iterative_retrieve()` — 从 top-5 结果提取关键词，BM25 二次检索，合并去重
2. **意图特定 Reader Prompt**：按 LoCoMo category 使用专用 prompt
   - single_hop → `READER_PROMPT_FACTUAL`
   - multi_hop → `READER_PROMPT_MULTI_HOP`
   - temporal → `READER_PROMPT_TEMPORAL`
   - adversarial → `READER_PROMPT_ADVERSARIAL`
3. **搜索 limit 提升**：10→15 snippets，context 5→10 条

## Phase 4：质量门控

- `cargo test --lib`: 2075 通过 ✅
- `cargo llvm-cov --lib`: 87.02% ≥ 87% ✅
- `cargo clippy`: 无新增警告 ✅
- 性能回归测试: 12/12 通过 ✅

## 关键文件变更

- `src/fs/query_decompose.rs` — 新增，查询分解引擎
- `src/fs/graph/mod.rs` — 新增 `get_nodes_by_cid()` trait 方法
- `src/fs/graph/backend.rs` — 实现 `get_nodes_by_cid()` + 修复 find_paths/find_weighted_path
- `src/fs/semantic_fs/mod.rs` — 集成路径发现到 search_with_filter
- `src/fs/retrieval_router.rs` — MultiHop top_k 15→30
- `src/fs/mod.rs` — 注册 query_decompose 模块

## 开发流程教训

- 删除代码前必须检查测试覆盖 — binary_index 有 4 个测试，不是死代码
- 功能测试 + 性能测试双重验证 — 纯功能测试无法捕获性能回归
- `..Default::default()` 不能用于 Rust enum variant — 必须显式填写所有字段
- `#[tokio::test]` 必须用于调用 kernel API 的测试 — kernel 内部使用 tokio
- `SearchResult` 需要 `Serialize/Deserialize` 才能被 SearchCache 缓存
- `&Arc<dyn Trait>` 不能直接当 `&dyn Trait` 用 — 需要 `kg.as_ref()` 显式转换

## 性能回归测试教训 (P0)

v46 开发中错误删除了 binary_index 两阶段搜索代码（有 4 个测试覆盖的线上功能）。纯功能测试无法捕获性能回归 — 必须有性能回归测试。删除代码前必须：
1. 检查测试覆盖
2. 运行性能测试
3. 确认是废弃代码而非未接入代码

## 遗留问题

- [ ] LoCoMo F1 > 0.50（需运行 benchmark 验证）
- [ ] Search P50 < 50ms（需运行 benchmark 验证）
- [ ] 6 个 benchmark suite 全通过（需运行验证）
- [ ] v46 benchmark 报告生成
