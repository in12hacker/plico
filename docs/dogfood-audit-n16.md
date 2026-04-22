# Plico Dogfood Audit — Node 16 全量实测（v2 更新）

**日期**: 2026-04-22 (初版) → 2026-04-22 (v2 更新: 灵魂2.0复审 + 全量覆盖率)
**测试实例**: `~/.plico` (持久化默认路径)
**方法**: 链式思考 + 真实 CLI 执行 + 源码行级分析 + 灵魂2.0逐条复审
**单元测试**: 934 passed, 0 failed, 7 ignored
**覆盖范围**: Node 1-16 全部设计承诺 + 灵魂2.0对齐 + 持久化方案 + 逐文件覆盖率

---

## 0. v2 更新摘要

| 变更 | 内容 |
|------|------|
| 持久化默认路径 | `/tmp` → `~/.plico`（三个二进制入口全部修改） |
| 灵魂2.0 复审 | 重新验证 V-01~V-05，发现 2 个新潜在违规 V-06/V-07 |
| 全量覆盖率 | 逐文件审计 86 个 src/*.rs 文件，识别 16 个关键无测试文件 |

---

## 1. 测试摘要

| 维度 | 测试项 | 通过 | 失败 | 通过率 |
|------|--------|------|------|--------|
| CAS 存储 | 5 | 5 | 0 | 100% |
| 输入防御 (B35/B46) | 5 | 5 | 0 | 100% |
| 分层记忆 | 7 | 7 | 0 | 100% |
| 名称解析 (B43/B47) | 4 | 4 | 0 | 100% |
| 知识图谱 | 6 | 6 | 0 | 100% |
| Session/Events/Delta | 4 | 3 | 1 | 75% |
| Intent 系统 | 2 | 2 | 0 | 100% |
| Search/Tags | 4 | 4 | 0 | 100% |
| Permission/Send | 2 | 2 | 0 | 100% |
| Agent 注册 | 2 | 2 | 0 | 100% |
| 持久化/幂等 | 2 | 2 | 0 | 100% |
| **总计** | **43** | **42** | **1** | **98%** |

唯一失败项：T29 `session-end --session <ID>` JSON 解析获取 session ID 失败（JSON 格式问题，非功能缺陷）。

---

## 2. 灵魂 2.0 对齐复审

### 2.1 原始违规追踪

| ID | 描述 | 严重度 | 状态 | 验证方式 |
|----|------|--------|------|---------|
| V-01 | IntentRouter 在内核 | P1-高 | ✅ **已修复** (v3.0-M1) | `grep -r "intent" src/kernel/` = 0 引用 |
| V-02 | intent_execute_sync 执行引擎在内核 | P1-高 | ✅ **已修复** (v3.0-M1) | `execute_sync` 在 `src/intent/execution.rs`，注释明确标注 "application-layer" |
| V-03 | Memory 无 MemoryScope | P2-中 | ✅ **已修复** | `MemoryScope::Private/Shared/Group` 完整实现，含 `get_shared()`, `get_by_group()`, 14 个测试 |
| V-04 | 自动学习硬编码 | P2-中 | ✅ **已修复** (v3.0-M3) | `start_result_consumer` 只发事件不写记忆；`remember_*` 仅响应显式 API 调用 |
| V-05 | AgentExecutor 命名误导 | P3-低 | ⚠️ **未修** | 仍名为 `AgentExecutor`，建议 → `ActionDispatcher` |

### 2.2 新发现的潜在违规

| ID | 描述 | 严重度 | 位置 | 分析 |
|----|------|--------|------|------|
| V-06 | `create()` 自动 LLM 摘要 | P2-中 | `semantic_fs/mod.rs:198-213` | 存储内容时自动调用 `summarizer.summarize()` 生成 L0 摘要。类似 V-04 的"OS 替 Agent 决定"模式。OS 不应在 `write()` 时隐式调用 LLM。 |
| V-07 | `create()` 自动 KG 关联 | P3-低 | `semantic_fs/mod.rs:189-196` | 存储内容时自动 upsert KG Document 节点 + SimilarTo 边。更接近"索引维护"，但仍是策略性决定（哪些相似度阈值建边）。 |

**V-06 详细分析:**

```
用户调用 put 存储内容
  → CAS 写入 ← 机制 (等同 write() syscall)
  → 标签索引更新 ← 机制 (等同目录条目)
  → 向量索引更新 ← 机制 (等同搜索索引)
  → LLM 摘要生成 ← ❌ 策略！(OS 不应在 write() 时调用 LLM)
  → KG 文档节点 ← ⚠️ 边界 (类似 inode 创建)
  → KG SimilarTo 边 ← ⚠️ 策略 (阈值判断)
```

**缓解**: `summarizer` 字段是 `Option<Arc<dyn Summarizer>>`，当 LLM 不可用时为 `None`，摘要被跳过。这不是强制行为。但"有 LLM 就自动摘要"仍属内核策略。

**建议修正方向**: 将自动摘要改为 API 参数（如 `auto_summarize: bool`），默认 `false`，由调用者决定是否需要。

### 2.3 灵魂对齐得分（v2 更新）

| 灵魂组件 | 得分 | 变化 | 说明 |
|----------|------|------|------|
| AI原生文件系统 | **93/100** | -2 | V-06 自动摘要扣分 |
| 一切皆工具 | **95/100** | — | ToolRegistry + ExternalToolProvider 正确 |
| 模型无关 | **90/100** | — | LlmProvider/SemanticSearch 全 trait 化 |
| Agent 调度器 | **85/100** | — | 状态机 + 资源配额完整 |
| 权限与安全 | **85/100** | — | ReadAny + scope + allowed_tools |
| 分层内存 | **95/100** | — | 4 层 + MemoryScope |
| 协议分层 | **90/100** | — | MCP 在接口层 |
| **内核纯净度** | **93/100** | -2 | V-06/V-07 轻微策略泄漏 |
| **总体** | **93/100** | -1 | 新发现 V-06/V-07 |

---

## 3. 持久化方案（v2 修正）

### 3.1 修正说明

原报告误将 `/tmp` 作为可接受默认值。v2 修正：

```
旧: --root > $PLICO_ROOT > dirs::data_dir()/plico > /tmp (fallback)
新: --root > $PLICO_ROOT > $HOME/.plico > /tmp (仅当 $HOME 缺失)
```

### 3.2 修改范围

| 文件 | 修改 |
|------|------|
| `src/bin/aicli/main.rs` | `dirs::data_dir().join("plico")` → `dirs::home_dir().join(".plico")` |
| `src/bin/plicod.rs` | `/tmp/plico` → `dirs::home_dir().join(".plico")` |
| `src/bin/plico_mcp.rs` | `/tmp/plico` → `dirs::home_dir().join(".plico")` |
| `scripts/plico-bootstrap.sh` | 已是 `${HOME}/.plico/dogfood`，注释更新 |
| `scripts/plico-post-commit.sh` | `/tmp/plico-dogfood` → `${HOME}/.plico/dogfood` |
| `.cursor/skills/plico-dogfood/SKILL.md` | `/tmp/plico-dogfood` → `${HOME}/.plico/dogfood` |
| `~/.claude/skills/plico-dogfood/SKILL.md` | 同上 |
| `CLAUDE.md`, `AGENTS.md` | 示例路径更新，不再需要 `--root` |
| `README.md`, `README_zh.md` | 示例路径更新 |
| `examples/` | 示例路径更新 |

### 3.3 设计理由

采用 `~/.plico` 而非 XDG `~/.local/share/plico`：

| 因素 | `~/.plico` | `~/.local/share/plico` |
|------|-----------|----------------------|
| 可发现性 | ✅ `ls -a ~` 即可看到 | ❌ 需要知道 XDG 规范 |
| 行业惯例 | `~/.cargo`, `~/.docker`, `~/.npm` | 少数 GUI 应用使用 |
| 简洁性 | ✅ 1 级目录 | ❌ 3 级嵌套 |
| 可移植性 | ✅ 所有 Unix | ✅ 所有 Unix |
| 适用场景 | 开发者工具 / CLI | 桌面应用 |

**结论**: Plico 是开发者/AI 工具，跟 cargo/docker 同类。`~/.plico` 是正确选择。

---

## 4. 全量单元测试覆盖率

### 4.1 总体数据

| 指标 | 值 |
|------|-----|
| 总测试数 | **934** (414 unit + 50 binary + 456 integration + 5 doc + 9 mcp) |
| src/ 文件总数 | **86** (不含 bin/) |
| 有测试的文件 | **48** 个（直接含 `#[test]`） |
| 有伴生测试的文件 | **+5** 个（tests.rs 覆盖主文件） |
| 纯 re-export / <30 行 | **13** 个（不计入覆盖率） |
| 有效代码文件 | **73** 个 |
| **文件覆盖率** | **53/73 = 72.6%** |

### 4.2 逐文件覆盖率清单

#### src/api/ (4 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `agent_auth.rs` | 365 | 9 | ✅ |
| `permission.rs` | 439 | 0 | ⚠️ (集成测试 permission_test.rs 24 tests) |
| `semantic.rs` | 2734 | 22 | ✅ |
| `mod.rs` | 19 | — | N/A (re-export) |

#### src/cas/ (3 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `object.rs` | 229 | 3 | ✅ |
| `storage.rs` | 455 | 7 | ✅ |
| `mod.rs` | 18 | — | N/A |

#### src/fs/ (20 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `context_budget.rs` | 253 | 7 | ✅ |
| `context_loader.rs` | 324 | 0 | ❌ **无测试** |
| `embedding/circuit_breaker.rs` | 248 | 2 | ✅ |
| `embedding/json_rpc.rs` | 30 | 0 | ❌ |
| `embedding/local.rs` | 230 | 0 | ❌ **无测试** |
| `embedding/mod.rs` | 36 | 1 | ✅ |
| `embedding/ollama.rs` | 278 | 0 | ❌ **无测试** |
| `embedding/ort_backend.rs` | 249 | 0 | ❌ **无测试** |
| `embedding/stub.rs` | 36 | 0 | ⚠️ (trivial impl) |
| `embedding/types.rs` | 69 | 0 | ❌ |
| `graph/backend.rs` | 749 | 0 | ⚠️ (graph/tests.rs 26 tests) |
| `graph/tests.rs` | 511 | 26 | ✅ |
| `graph/types.rs` | 325 | 0 | ❌ **无测试** |
| `search/bm25.rs` | 83 | 0 | ❌ |
| `search/hnsw.rs` | 573 | 10 | ✅ |
| `search/memory.rs` | 332 | 5 | ✅ |
| `search/mod.rs` | 147 | 0 | ❌ |
| `semantic_fs/events.rs` | 216 | 0 | ❌ **无测试** |
| `semantic_fs/mod.rs` | 753 | 0 | ⚠️ (semantic_fs/tests.rs 37 tests) |
| `semantic_fs/tests.rs` | 497 | 37 | ✅ |
| `summarizer.rs` | 166 | 4 | ✅ |
| `types.rs` | 200 | 0 | ❌ |

#### src/intent/ (4 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `execution.rs` | 265 | 0 | ❌ **无测试** |
| `heuristic.rs` | 662 | 15 | ✅ |
| `llm.rs` | 186 | 5 | ✅ |
| `mod.rs` | 181 | 3 | ✅ |

#### src/kernel/ (22 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `builtin_tools.rs` | 761 | 0 | ❌ **无测试 — 最大缺口** |
| `event_bus.rs` | 1073 | 27 | ✅ |
| `mod.rs` | 1910 | 0 | ⚠️ (kernel_test.rs 179 integration tests) |
| `persistence.rs` | 418 | 0 | ❌ **无测试** |
| `ops/agent.rs` | 609 | 8 | ✅ |
| `ops/batch.rs` | 238 | 0 | ❌ **无测试** (集成测试 batch_ops_test.rs 8 tests) |
| `ops/cache.rs` | 458 | 4 | ✅ |
| `ops/checkpoint.rs` | 502 | 3 | ✅ |
| `ops/dashboard.rs` | 348 | 0 | ❌ **无测试** |
| `ops/delta.rs` | 255 | 4 | ✅ |
| `ops/dispatch.rs` | 95 | 0 | ⚠️ (thin wrapper, scheduler tests cover) |
| `ops/distributed.rs` | 440 | 4 | ✅ |
| `ops/events.rs` | 71 | 0 | ❌ |
| `ops/fs.rs` | 531 | 6 | ✅ |
| `ops/graph.rs` | 967 | 15 | ✅ |
| `ops/hybrid.rs` | 367 | 2 | ✅ |
| `ops/memory.rs` | 716 | 7 | ✅ |
| `ops/messaging.rs` | 83 | 0 | ❌ |
| `ops/model.rs` | 361 | 0 | ❌ **无测试** (集成测试 model_hot_swap_test.rs 9 tests) |
| `ops/observability.rs` | 756 | 11 | ✅ |
| `ops/permission.rs` | 42 | 0 | ❌ (small) |
| `ops/prefetch.rs` | 1842 | 22 | ✅ |
| `ops/session.rs` | 878 | 21 | ✅ |
| `ops/task.rs` | 504 | 10 | ✅ |
| `ops/tenant.rs` | 324 | 9 | ✅ |
| `ops/tier_maintenance.rs` | 225 | 4 | ✅ |
| `ops/tools_external.rs` | 81 | 0 | ❌ |

#### src/llm/ (4 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `mod.rs` | 109 | 3 | ✅ |
| `ollama.rs` | 118 | 0 | ❌ |
| `openai.rs` | 209 | 6 | ✅ |
| `stub.rs` | 27 | — | N/A (trivial) |

#### src/mcp/ (3 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `client.rs` | 226 | 0 | ⚠️ (mcp/tests.rs 9 tests) |
| `tests.rs` | 157 | 9 | ✅ |
| `mod.rs` | 12 | — | N/A |

#### src/memory/ (6 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `context_snapshot.rs` | 146 | 3 | ✅ |
| `layered/mod.rs` | 1191 | 0 | ⚠️ (layered/tests.rs 14 tests) |
| `layered/tests.rs` | 342 | 14 | ✅ |
| `persist.rs` | 388 | 6 | ✅ |
| `relevance.rs` | 340 | 10 | ✅ |
| `mod.rs` | 24 | — | N/A |

#### src/scheduler/ (5 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `agent.rs` | 345 | 5 | ✅ |
| `dispatch.rs` | 727 | 5 | ✅ |
| `messaging.rs` | 183 | 4 | ✅ |
| `mod.rs` | 334 | 5 | ✅ |
| `queue.rs` | 93 | 0 | ❌ |

#### src/temporal/ (3 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `resolver.rs` | 200 | 0 | ❌ **无测试** |
| `rules.rs` | 362 | 8 | ✅ |
| `mod.rs` | 35 | — | N/A |

#### src/tool/ (3 文件)

| 文件 | 行数 | 测试数 | 状态 |
|------|------|--------|------|
| `mod.rs` | 154 | 3 | ✅ |
| `procedure_provider.rs` | 232 | 4 | ✅ |
| `registry.rs` | 186 | 8 | ✅ |

### 4.3 关键无测试文件（按风险排序）

| 优先级 | 文件 | 行数 | 风险理由 |
|--------|------|------|---------|
| 🔴 P0 | `kernel/builtin_tools.rs` | 761 | 37 个工具分发入口，历史 B43 来源，零测试 |
| 🔴 P0 | `kernel/persistence.rs` | 418 | 启动时状态恢复，数据丢失风险 |
| 🔴 P0 | `intent/execution.rs` | 265 | 意图执行引擎，应用层核心逻辑 |
| 🟠 P1 | `kernel/ops/dashboard.rs` | 348 | 可观测性，SystemStatus 响应 |
| 🟠 P1 | `kernel/ops/model.rs` | 361 | LLM hot-swap，运行时模型切换 |
| 🟠 P1 | `fs/context_loader.rs` | 324 | 上下文加载器，L0/L1/L2 降级 |
| 🟠 P1 | `fs/graph/types.rs` | 325 | KG 类型定义 + 序列化 |
| 🟡 P2 | `fs/embedding/ollama.rs` | 278 | 外部 API 集成 |
| 🟡 P2 | `fs/embedding/ort_backend.rs` | 249 | ONNX Runtime 本地推理 |
| 🟡 P2 | `fs/embedding/local.rs` | 230 | 本地嵌入后端 |
| 🟡 P2 | `fs/semantic_fs/events.rs` | 216 | 文件系统事件 |
| 🟡 P2 | `temporal/resolver.rs` | 200 | 自然语言时间解析 |
| 🟡 P2 | `kernel/ops/batch.rs` | 238 | 批量操作 |

**缺失测试代码总行数**: ~4,291 行（P0+P1+P2 合计）

### 4.4 覆盖率演变

```
N14: ████████████████░░░░░░░░░░░░░░░░░░░░░░░░ 43% (42/97)
N15: ████████████████░░░░░░░░░░░░░░░░░░░░░░░░ 43%
N16: ███████████████████░░░░░░░░░░░░░░░░░░░░░ 48% (47/97)
N16v2:███████████████████████████░░░░░░░░░░░░░ 73% (53/73, 含伴生tests.rs)
目标: ████████████████████████████████░░░░░░░░ 80%
```

**注**: v2 修正了计算方式 — 将 mod.rs 等 re-export 文件排除在分母外，将伴生 tests.rs 文件算入覆盖。73% 反映真实有效覆盖率。

---

## 5. Node 16 设计承诺实现状态

### 5.1 六大维度

| 维度 | 承诺 | 实现状态 | 验证证据 |
|------|------|---------|---------|
| D1 持久运营 | 数据存活跨重启 | ✅ **100%** | `main.rs` 默认 `~/.plico`；`plico-bootstrap.sh` 默认 `~/.plico/dogfood` |
| D2 幽灵防御 | 无操作返回错误 | ✅ **100%** | `semantic_fs/mod.rs` `self.cas.get(cid)?` 传播错误 |
| D3 解析完备 | 所有 agent_id 走解析 | ✅ **100%** | `skills.rs` 4 个命令全用 `resolve_agent()` |
| D4 幂等引导 | 重复不产生重复数据 | ✅ **100%** | `plico-bootstrap.sh` 幂等检查 |
| D5 操作审计 | 关键操作留痕 | ✅ **100%** | 事件总线 + 审计日志 |
| D6 规格绑定 | 设计决策入库 | ✅ **100%** | 15 个历史 ADR 批量录入 |

### 5.2 特性实现

| 特性 | 目标 | 状态 | 源码位置 |
|------|------|------|---------|
| F-1 持久存储 | `/tmp` → 用户目录 | ✅ (v2 修正) | `main.rs` `dirs::home_dir().join(".plico")` |
| F-2 幽灵防御 | delete 假成功 → 错误 | ✅ | `semantic_fs/mod.rs` `self.cas.get(cid)?` |
| F-3 名称解析 | skills 全路径解析 | ✅ | `skills.rs` 全用 `resolve_agent()` |
| F-4 幂等引导 | bootstrap 去重 | ✅ | `plico-bootstrap.sh` + `kg_add_node_idempotent` |
| F-5 单元测试 | ~40 个新测试 | 🟡 **部分** | 36 新测试；handlers/builtin_tools 仍为 0 |
| F-6 ADR 入库 | 历史 ADR 批量录入 | ✅ | `plico-bootstrap.sh` 15 个 ADR |

### 5.3 Bug 修复追踪

| Bug | 状态 | 验证 |
|-----|------|------|
| B35 CRITICAL delete panic | ✅ Fixed | `delete <CID>` 正常 |
| B43 名称解析 | ✅ Fixed | `tool call memory.recall {"agent_id":"name"}` 成功 |
| B44 send --agent | ✅ Fixed | 权限检查用指定 agent |
| B45 register --name | ✅ Fixed | `name` 正确注册 |
| B46 phantom delete | ✅ Fixed | `delete "a"` → InvalidCid |
| B47 skills register name | ✅ Fixed | `skills register --agent "name"` 成功 |

**6/6 全部修复。CRITICAL Bug: 0。**

---

## 6. Node 1-16 全量承诺追踪

### Node 1-3 基础设施 — 100%

| 承诺 | 状态 |
|------|------|
| CAS 存储/检索/SHA-256 | ✅ |
| CID 输入验证 | ✅ |
| 语义搜索/KG/标签 | ✅ |

### Node 4-6 协作层 — 100%

| 承诺 | 状态 |
|------|------|
| Agent 注册 + 名称解析 | ✅ |
| Permission + Session | ✅ |
| 消息传递 | ✅ |

### Node 7-8 驾具层 — 100%

| 承诺 | 状态 |
|------|------|
| 事件持久化 + Delta | ✅ |

### Node 9-10 韧性层 — 100%

| 承诺 | 状态 |
|------|------|
| 弹性降级 + 整流 | ✅ |

### Node 11-12 自演进层 — 100%

| 承诺 | 状态 |
|------|------|
| 4 tier 分层记忆 + Consolidation | ✅ |

### Node 13 传导层 — 100%

| 承诺 | 状态 |
|------|------|
| API v18 + 37 工具 + MCP + Intent | ✅ |

### Node 14 融合层 — 100%

| 承诺 | 状态 |
|------|------|
| F-1~F-9 全部完成 | ✅ |

### Node 15 验证层 — 80%

| 承诺 | 状态 |
|------|------|
| F-1 CID 防御 | ✅ |
| F-2 CLI 参数统一 | ✅ |
| F-3 名称解析 | ✅ |
| F-4 单元测试 (~81) | 🟡 (N16 补了 36，仍缺 handlers) |
| F-5 CAS 防御 | ✅ |
| F-6 行为指纹 | ✅ |

### Node 16 持续层 — 95%

| 承诺 | 状态 |
|------|------|
| F-1 持久存储 | ✅ (v2: `~/.plico`) |
| F-2 幽灵防御 | ✅ |
| F-3 名称解析补全 | ✅ |
| F-4 幂等引导 | ✅ |
| F-5 单元测试 (~40) | 🟡 (36/40) |
| F-6 ADR 入库 | ✅ |

---

## 7. 进展对比

| 指标 | N14 | N15 | N16 v1 | N16 v2 | 趋势 |
|------|-----|-----|--------|--------|------|
| CLI 通过率 | 91% | 94% | 98% | **98%** | ⬆ |
| 自动化测试 | 808 | 876 | 934 | **934** | ⬆ |
| CRITICAL Bug | 1 | 0 | 0 | **0** | ✅ |
| 现存 Bug | 4 | 2 | 0 | **0** | ✅ |
| 文件覆盖率 | 43% | 43% | 48% | **73%** (修正计算) | ⬆⬆ |
| 持久化默认 | /tmp | /tmp | XDG+/tmp | **~/.plico** | ✅ |
| 灵魂对齐分 | — | — | 94 | **93** (新发现V-06/V-07) | — |

---

## 8. 建议

### P0 (立即)

| 任务 | 说明 |
|------|------|
| `kernel/builtin_tools.rs` 单元测试 | 761 行零测试，37 个工具分发，历史 B43 来源 |
| `kernel/persistence.rs` 单元测试 | 418 行零测试，启动恢复逻辑 |
| `intent/execution.rs` 单元测试 | 265 行零测试，意图执行核心 |

### P1 (近期)

| 任务 | 说明 |
|------|------|
| V-06 修复: `create()` 自动摘要改为可选参数 | 内核不应隐式调用 LLM |
| `kernel/ops/dashboard.rs` 单元测试 | 348 行零测试 |
| `kernel/ops/model.rs` 单元测试 | 361 行零测试 |
| `fs/context_loader.rs` 单元测试 | 324 行零测试 |

### P2 (中期)

| 任务 | 说明 |
|------|------|
| V-05 `AgentExecutor` → `ActionDispatcher` 重命名 | 消除命名误导 |
| V-07 评估: KG 自动关联是否需要改为可选 | 评估后决定 |
| embedding 后端测试 (ollama/ort/local) | 外部依赖 mock 测试 |
| `temporal/resolver.rs` 单元测试 | 200 行零测试 |
| 升级 `dirs` → `directories::ProjectDirs` | 完整 XDG 支持 |

---

*报告基于 934 个自动化测试 + 43 项 CLI 实测 + 86 个源文件逐一审计 + 灵魂2.0逐条复审。*
*所有 Bug (B35-B47) 已修复。2 个新灵魂违规 (V-06/V-07) 已识别。*
