# Plico Node 17 Dogfood 审计报告
# 信 — 操作诚信与效果合约

**审计版本**: Node 17 (commit 7b68491)
**审计日期**: 2026-04-20
**审计方法**: 真实 CLI 执行 (`/tmp/plico-n17-audit` 干净环境) + 全量源码逐文件审计 + cargo test 1057 全通过 + minimax 视角交叉验证
**灵魂基准**: `system-v2.md` (Soul 2.0) 十条公理

---

## 0. 审计背景

此报告是 **Node 17 的独立审计**，不修改 `dogfood-audit-n16.md`。

Claude Minimax 给出了 Plico 的 Soul 2.0 对齐度评估（~62%），指出了若干核心问题。本报告：
1. 逐条验证 minimax 的公理评估，给出我方独立判断
2. 验证 Node 17 设计文档的六大维度（D1-D6）和六个特性（F-1~F-6）完成度
3. 全量单元测试覆盖率审计
4. 综合评估当前 Plico 进度与差距

---

## 1. Node 17 承诺验证

### 1.1 六大维度完成度

| 维度 | 承诺 | 实现度 | 验证证据 |
|------|------|--------|---------|
| **D1 输入保真** | 位置参数+空内容拒绝 | **✅ 100%** | `put "positional content test"` → CID `d8adfb14`, `get` 返回完整内容 |
| **D2 效果合约** | write/delete 后置条件 | **✅ 100%** | `semantic_create` 空内容前置条件(line 64) + CID可检索后置条件(line 73-81), `delete` recycle-bin后置条件(line 258-266) |
| **D3 工具合约化** | P0工具前后置条件 | **🟡 70%** | 内核层效果合约覆盖 `cas.create`/`cas.delete`, 但 builtin_tools.rs 的工具分发层未独立添加前置检查（依赖内核层传播） |
| **D4 P0测试覆盖** | 每个P0文件≥8测试 | **✅ 100%** | `builtin_tools.rs`: 16测试, `persistence.rs`: 8测试, `execution.rs`: 8测试, 合计32(目标24) |
| **D5 灵魂违规修复** | V-06 auto_summarize→可选 | **✅ 100%** | `PLICO_AUTO_SUMMARIZE=1` 才启用, 默认 `disable_auto_summarize: true` |
| **D6 CLI系统审计** | 全部16个handler参数一致性 | **🟡 60%** | `cmd_create` 修复 ✅, 但 `cmd_update`/`cmd_remember`/`cmd_send`/`cmd_intent` 未见系统性审计证据 |

**总体**: 5/6 维度≥70%, 1个维度(D6)仅60%. 承诺达成率约 **88%**.

### 1.2 特性清单完成度

| 特性 | 描述 | 状态 | 详情 |
|------|------|------|------|
| **F-1** | B49修复 + CLI输入保真 | **✅ Done** | `cmd_create` 支持位置参数, 空内容返回错误 |
| **F-2** | 效果合约系统 | **✅ Done** | `semantic_create` 前后置条件, `delete` 后置条件, 4个合约测试 |
| **F-3** | 工具前后置条件 | **🟡 Partial** | 通过内核层间接覆盖, 但 `builtin_tools.rs` dispatch层无独立pre-check |
| **F-4** | P0单元测试 | **✅ Done** | F-4a: 16, F-4b: 8, F-4c: 8, 合计32 |
| **F-5** | V-06修复 | **✅ Done** | 环境变量开关, 默认off |
| **F-6** | CLI系统审计 | **🟡 Partial** | B49修复证明crud.rs审计完成, 其余handler未完全审计 |

**总体**: 4/6 完成, 2/6 部分完成. **特性完成率约 80%**.

### 1.3 Bug 状态

| Bug | 严重度 | 状态 | 验证 |
|-----|--------|------|------|
| B35 delete panic | P0 | ✅ Fixed (N14) | — |
| B43 名称解析 | P0 | ✅ Fixed (N15) | — |
| B44-B47 | P1-P2 | ✅ Fixed (N15-N16) | — |
| **B49 phantom put** | **P0** | **✅ Fixed (N17)** | `put "content"` → 完整CID, `put` (无内容) → 错误 |
| **B50 edge type 静默降级** | P2 | **❌ New** | `edge --type caused_by` → 存储为 `RelatedTo` (无匹配时静默回退) |
| **B51 warm_context 非CAS-CID** | P1 | **❌ New** | `session-start` 返回 UUID `3c322b1a-...`, `get` 返回 `InvalidCid` |

---

## 2. Minimax 公理评估 — 交叉验证

### 2.1 逐条对比

| 公理 | Minimax评估 | 我方评估 | 差异原因 |
|------|-------------|---------|---------|
| **1. Token最稀缺** | 85% | **90%** | minimax遗漏: `context assemble --budget` 实现了预算控制 ✅, 搜索有 `--limit` 参数 ✅ |
| **2. 意图先于操作** | 60% | **70%** | `session-start --intent` 返回 `warm_context`+`changes_since_last` ✅, 但 `warm_context` 是UUID非CAS-CID(B51), 无法直接检索. 预热上下文未实现 |
| **3. 记忆跨越边界** | 90% | **92%** | minimax说"CLI无`--tier`参数" — **错误**. `remember --tier working/long-term` 实测正常 ✅, `--scope shared` 也有 ✅ |
| **4. 共享先于重复** | 40% | **55%** | `MemoryScope::Private/Shared/Group` 已实现 ✅, `remember --scope shared` 可存 ✅, `discover` 可发现其他Agent ✅. 但**跨Agent搜索共享记忆**的API确实缺失 |
| **5. 机制不是策略** | 95% | **97%** | 37工具 ✅, V-06修复 auto_summarize默认off ✅, 内核无NL ✅, 无自动学习 ✅ |
| **6. 结构先于语言** | 70% | **72%** | minimax核心批评正确: 默认CLI输出是人类格式(如 `Agent ID: xxx`), 需 `AICLI_OUTPUT=json` 切换. 作为AIOS, 默认应为JSON |
| **7. 主动先于被动** | 30% | **35%** | `session-start` 返回 `changes_since_last` ✅ (被动推送存在). 但意图缓存未实现 ❌, 后台预组装未实现 ❌ |
| **8. 因果先于关联** | 60% | **62%** | `KGEdgeType::Causes` 存在 ✅, `kg.paths` 可查因果链 ✅, 事件日志不可变 ✅. 但B50(edge type静默降级)导致因果边可能变成RelatedTo |
| **9. 越用越好** | 20% | **25%** | 意图缓存未实现 ❌, Agent profile未实现 ❌. 唯一的"越用越好"特征: 共享程序记忆可被其他Agent发现 ✅ |
| **10. 会话是一等公民** | 70% | **78%** | `session-start/end` 完整 ✅, auto-checkpoint ✅, `changes_since_last` ✅, `consolidation` 统计 ✅. 但 B51(`warm_context`不可检索)是真实gap |

### 2.2 综合Soul 2.0对齐度

| 来源 | 对齐度 |
|------|--------|
| Minimax评估 | ~62% |
| 我方评估 | **~68%** |

**差异分析**: Minimax有3个事实性错误:
1. ❌ "CLI remember无--tier参数" → 实测有, 功能正常
2. ❌ "warm_context返回空" → 返回非空UUID (虽然不可CAS检索)
3. ❌ 遗漏 `context assemble --budget` 功能

Minimax有2个核心洞察完全正确:
1. ✅ 默认输出格式不是AI-native (公理6)
2. ✅ 公理7/9几乎未实现 (主动性与越用越好)

### 2.3 架构红线检查

| 红线 | 状态 | 证据 |
|------|------|------|
| 内核零协议 | **✅** | MCP/SSE/TCP 只在 `bin/` |
| 内核零模型 | **✅** | `EmbeddingProvider`/`Summarizer` 通过trait抽象, 且 auto_summarize 默认off |
| 内核零自然语言 | **✅** | IntentRouter 已提取到 `intent/` (v3.0 M1, commit 0a061ba) |
| 存储与索引分离 | **✅** | CAS ≠ Search, 独立子系统 |
| 记忆scope强制 | **✅** | `MemoryScope::Private/Shared/Group` 实现完整 |
| 事件日志不可变 | **✅** | append-only SequencedEvent |

---

## 3. 单元测试覆盖率审计

### 3.1 全量数据

| 指标 | 值 |
|------|-----|
| 总测试数 | **1057** (537 lib + 30 aicli + 20 plicod_tests + 181 harness + 其余integration/doc) |
| 全部通过 | ✅ 0 failed, 7 ignored |
| 总 .rs 文件 | 111 |
| 基础设施文件 (mod.rs/lib.rs/main.rs ≤30行) | 11 |
| 测试伴生文件 | 5 (`semantic_fs/tests.rs`, `graph/tests.rs`, `layered/tests.rs`, `mcp/tests.rs`, `kernel/tests.rs`) |
| 有效代码文件 | **95** |
| 有内联测试的文件 | 54 |
| 被伴生测试覆盖的文件 | +4 (`layered/mod.rs`, `semantic_fs/mod.rs`, `graph/backend.rs`, `mcp/client.rs`) |
| 有doc测试的文件 | +2 (`api/permission.rs`, `api/semantic.rs`) |
| **有测试覆盖的代码文件** | **60** |
| **文件覆盖率** | **63.2%** (60/95) |

### 3.2 零测试的关键文件 (>100行)

| 优先级 | 文件 | 行数 | 风险说明 |
|--------|------|------|---------|
| **P0** | `kernel/mod.rs` | 1910 | 内核主模块, 依赖integration测试间接覆盖 |
| **P1** | `bin/aicli/commands/mod.rs` | 487 | CLI命令路由分发 |
| **P1** | `bin/aicli/main.rs` | 535 | CLI入口, 参数解析 |
| **P1** | `api/permission.rs` | 439 | 权限系统 (有5个doc test) |
| **P2** | `fs/embedding/ollama.rs` | 278 | 外部集成 (需真实后端) |
| **P2** | `fs/embedding/ort_backend.rs` | 249 | 外部集成 (需真实后端) |
| **P2** | `bin/aicli/commands/handlers/graph.rs` | 245 | CLI handler |
| **P2** | `fs/embedding/local.rs` | 230 | 外部集成 |
| **P2** | `fs/types.rs` | 200 | 数据结构定义 |
| **P2** | `bin/aicli/commands/handlers/agent.rs` | 190 | CLI handler |
| **P2** | `bin/aicli/commands/handlers/crud.rs` | 182 | CLI handler |
| **P2** | `bin/plicod.rs` | 161 | TCP daemon入口 |
| **P2** | `fs/search/mod.rs` | 147 | 搜索子系统trait |

### 3.3 Node 17 测试增量

| N16终点 | N17终点 | 增量 |
|---------|---------|------|
| 934 | 1057 | **+123** |

按文件:
- `builtin_tools.rs`: 0 → 16 (+16)
- `persistence.rs`: 0 → 8 (+8)
- `execution.rs`: 0 → 8 (+8)
- `semantic_fs/events.rs`: 0 → 10 (+10)
- 其他测试增量: +81 (integration + handler + harness)

---

## 4. 与 Minimax 差异的深度分析

### 4.1 默认输出格式 (公理6) — Minimax 正确

```bash
# 无AICLI_OUTPUT时
$ aicli agent --name test
Agent ID: 6871aa4d-705e-430e-a9df-8649364f2258   ← 人类格式

# 需要手动设置
$ AICLI_OUTPUT=json aicli agent --name test
{"ok":true,"version":"18.0.0","agent_id":"..."}  ← JSON格式
```

这违反了 Soul 2.0 公理6的推论: "ApiRequest / ApiResponse（JSON）是唯一的内核接口"。
CLI 虽然是协议适配器, 但作为 Agent 的主要接入方式, 默认应服务 AI 而非人类。

**建议**: P0 — 反转默认值, `AICLI_OUTPUT=human` 时才输出人类格式。

### 4.2 跨Agent共享发现 (公理4) — Minimax 基本正确

实测:
- `remember --scope shared` → 存储成功 ✅
- `discover` → 列出所有Agent ✅

缺失:
- Agent B 无法搜索/检索 Agent A 的 `shared` 记忆
- 缺少 `recall --scope shared --from <agent>` 或类似API

**建议**: P1 — 增加跨Agent共享记忆检索API。

### 4.3 主动性与越用越好 (公理7+9) — Minimax 正确

这是当前最大差距。Soul 2.0 核心创新在于:
- 公理7: Agent声明意图后, OS后台预组装上下文
- 公理9: 重复意图零成本 (意图缓存)

当前状态:
- `session-start --intent "X"` 被接受, 但 `warm_context` 是不可检索的UUID
- 无意图缓存机制
- 无Agent工作模式学习

**建议**: P1 — 这需要新Node专门实现, 不是单点修复。

### 4.4 Edge Type 静默降级 (B50) — 新发现

```bash
$ edge --src A --dst B --type caused_by
→ "Edge created: A --[RelatedTo]--> B"   # 期望 Causes, 实际 RelatedTo
```

`parse_edge_type` 函数仅接受特定snake_case变体 (`causes`, `follows`, `mentions` 等)。
`caused_by` 不匹配任何变体, 静默回退为 `RelatedTo`。

**根因**: 无效类型不报错。
**建议**: P2 — 无效edge type应返回错误+有效类型列表。

### 4.5 warm_context 不可检索 (B51) — 新发现

```bash
$ session-start --agent test --intent "audit"
→ "warm_context": "3c322b1a-654d-4125-b6ff-8a875342c04f"

$ get 3c322b1a-654d-4125-b6ff-8a875342c04f
→ "Invalid CID format"   # UUID不是CAS CID
```

`warm_context` 返回的是内部引用ID, 不是可检索的CAS内容。Agent无法使用这个值。

**建议**: P1 — `warm_context` 应返回可检索的CAS CID, 或直接返回组装好的上下文内容。

---

## 5. 当前进度总览 (Node 1-17)

### 5.1 节点完成度

| 节点 | 主题 | 完成度 | 关键成果 |
|------|------|--------|---------|
| N1-3 | CAS+语义FS+搜索 | ✅ 100% | AIOS存储层 |
| N4-6 | Agent+权限+消息 | ✅ 100% | AIOS调度层 |
| N7-8 | 事件+Delta | ✅ 100% | AIOS驾具层 |
| N9-10 | 弹性+整流 | ✅ 100% | AIOS韧性层 |
| N11-12 | 4层记忆+合并 | ✅ 100% | AIOS记忆层 |
| N13 | API v18+MCP+Intent | ✅ 100% | AIOS传导层 |
| N14 | 融合 | ✅ 100% | AIOS集成层 |
| N15 | 输入安全 | ✅ 100% | AIOS防御层(输入) |
| N16 | 持久化+幽灵防御 | ✅ 95% | AIOS持续层 |
| **N17** | **效果合约** | **🟡 88%** | AIOS诚信层 (F-3/F-6部分完成) |

### 5.2 量化指标演进

| 指标 | N15 | N16 | N17 | 变化 |
|------|-----|-----|-----|------|
| 总测试数 | 483 | 934 | **1057** | +123 |
| 文件覆盖率 | ~48% | 72.6% | **63.2%** | -9.4%* |
| P0零测试文件 | 3 | 3 | **1** | -2 |
| Soul对齐分 | ~90 | 93 | **~68%**† | — |
| CLI Bug | 2 | 1 | **0 (+2new)** | ±2 |

*覆盖率下降是因为本次使用更严格的95文件基准 (之前是73有效文件, 排除了handler和embedding)。
†从100分制改为百分比制(逐公理加权平均), 使公理7/9的缺失真实反映。

---

## 6. Soul 2.0 对齐度分层评估

### 6.1 基础层 (公理1-5): 内核能力 — 88%

| 公理 | 得分 | 关键机制 |
|------|------|---------|
| 1. Token最稀缺 | 90% | L0/L1/L2分层, context assemble --budget, token_estimate |
| 2. 意图先于操作 | 70% | session-start --intent, 但warm_context不可用(B51) |
| 3. 记忆跨越边界 | 92% | 4层记忆, --tier, --scope, 持久化 |
| 4. 共享先于重复 | 55% | MemoryScope ✅, discover ✅, 但跨Agent检索缺失 |
| 5. 机制不是策略 | 97% | 37工具, 无自动行为, V-06修复 |

### 6.2 接口层 (公理6): AI-native交互 — 72%

| 公理 | 得分 | 关键机制 |
|------|------|---------|
| 6. 结构先于语言 | 72% | JSON输出存在但非默认, 内核API结构化 |

### 6.3 高阶层 (公理7-10): 智能特性 — 40%

| 公理 | 得分 | 关键机制 |
|------|------|---------|
| 7. 主动先于被动 | 35% | changes_since_last ✅, 意图缓存 ❌, 预热 ❌ |
| 8. 因果先于关联 | 62% | KG因果边 ✅, 事件不可变 ✅, B50静默降级 |
| 9. 越用越好 | 25% | 共享程序记忆 ✅, 意图缓存 ❌, profile ❌ |
| 10. 会话是一等公民 | 78% | session生命周期 ✅, checkpoint ✅, B51 |

### 6.4 加权总分: **68%**

权重: 基础层50% + 接口层15% + 高阶层35%
= 0.50 × 88% + 0.15 × 72% + 0.35 × 40%
= 44% + 10.8% + 14% = **68.8%**

---

## 7. 建议优先级

### P0 (当前阻塞项)

| ID | 建议 | 公理 | 工作量 |
|----|------|------|--------|
| R1 | 默认输出JSON, 人类格式改为 `AICLI_OUTPUT=human` opt-in | 6 | 0.5天 |
| R2 | `warm_context` 返回可检索CAS-CID或直接内容 (B51) | 2,10 | 1天 |

### P1 (影响Soul对齐度)

| ID | 建议 | 公理 | 工作量 |
|----|------|------|--------|
| R3 | 跨Agent共享记忆检索API | 4 | 2天 |
| R4 | 意图缓存: 相似意图命中缓存 | 7,9 | 3-5天 |
| R5 | 上下文主动预热: session-start后后台assemble | 7 | 2-3天 |
| R6 | F-6 完成CLI系统审计 (剩余~8个handler) | D6 | 1天 |

### P2 (质量提升)

| ID | 建议 | 公理 | 工作量 |
|----|------|------|--------|
| R7 | B50 edge type 无效值返回错误而非静默降级 | 8 | 0.5天 |
| R8 | `kernel/mod.rs` (1910行) 内联测试 | D4 | 1-2天 |
| R9 | Agent工作模式profile记录 | 9 | 3天 |
| R10 | CLI handler单元测试覆盖 (~10个handler) | D4 | 2天 |

### P3 (长期方向)

| ID | 建议 | 公理 | 说明 |
|----|------|------|------|
| R11 | 意图→上下文装配管线 (DeclareIntent→FetchAssembledContext) | 7 | 需要新Node |
| R12 | 因果路径查询API (why-chain) | 8 | 增强KG |
| R13 | Token计费(会话级) | 10 | 需要计量基础设施 |

---

## 8. Minimax评估的修正总结

| Minimax观点 | 我方判断 | 理由 |
|-------------|---------|------|
| "CLI remember无--tier参数" | **❌ 错误** | 实测 `remember --tier working/long-term` 正常工作 |
| "warm_context返回空" | **⚠️ 不准确** | 返回UUID(非空), 但确实不可CAS检索(B51) |
| "搜索默认返回全量" | **⚠️ 部分正确** | 有 `--limit` 参数, 但默认不限制 |
| "默认输出不适合AI" | **✅ 完全正确** | 这是最需要修复的公理6违规 |
| "跨Agent共享未实现" | **✅ 基本正确** | 机制存在(MemoryScope), 但发现API缺失 |
| "意图缓存未实现" | **✅ 完全正确** | 公理7/9的核心差距 |
| "总体对齐度~62%" | **⚠️ 略低估** | 我方评估~68%, minimax遗漏了一些已实现能力 |
| "架构根基正确" | **✅ 完全正确** | 公理1-5(基础层)得分88% |

---

## 9. 从 Node 17 到 Node 18 的推演

Node 17 完成了:
- ✅ 输入保真 (B49修复)
- ✅ 效果合约 (semantic_create/delete)
- ✅ P0文件测试覆盖 (32个新测试)
- ✅ V-06灵魂违规修复

Node 17 未完成:
- 🟡 F-3 工具层独立合约 (依赖内核层传播)
- 🟡 F-6 CLI系统审计 (仅完成crud.rs)

**Node 18 候选方向**:
- 方向A: **AI-Native Interface** — R1(默认JSON) + R2(warm_context) + R6(CLI审计), 聚焦公理6/10
- 方向B: **Proactive OS** — R4(意图缓存) + R5(上下文预热), 聚焦公理7/9
- 方向C: **Shared Intelligence** — R3(跨Agent共享检索), 聚焦公理4

建议: **方向A优先** — 低工作量, 高影响. 解决最明显的公理6违规, 使Plico真正AI-native. 然后方向B构建AIOS的差异化能力。

---

## 10. Dogfood 持久化数据健康诊断

### 10.1 KG 孤立节点分析

Minimax 指出 "KG边的丰富度可以增强，很多Fact节点没有关联到模块实体"。实测验证：

| 指标 | 值 |
|------|-----|
| Entity 节点 (模块锚点) | 11 |
| Fact 节点 (决策/知识) | 20 |
| 有模块链接的 Fact | **4** (20%) |
| **孤立 Fact** | **16** (80%) |

**有链接的 Fact** (通过 `RelatedTo` 边连接到 Entity):
- ✅ Use generic KG primitives → graph, kernel
- ✅ ADR: Persistent ~/.plico → cli
- ✅ ADR: V-06 fix plan → fs

**孤立 Fact** (无模块链接, 图查询不可达):
- ❌ Semantic Search Fallback — 应关联 → fs
- ❌ CAS Content Addressing — 应关联 → cas
- ❌ ExternalToolProvider Protocol — 应关联 → kernel
- ❌ Agent Checkpoint via CAS — 应关联 → kernel, cas
- ❌ Memory Link Engine — 应关联 → memory
- ❌ Agent Lifecycle Management — 应关联 → kernel
- ❌ Tier Maintenance Cycle — 应关联 → memory
- ❌ Four-Layer Memory — 应关联 → memory
- ❌ Tool Handler Trait — 应关联 → kernel
- ❌ Session Persistence — 应关联 → kernel
- ❌ Concurrent Agent Dispatch — 应关联 → kernel
- ❌ KG Generic Types — 应关联 → graph
- ❌ Context Budget Engine — 应关联 → fs
- ❌ Event Bus Architecture — 应关联 → kernel
- ❌ Everything is a Tool — 应关联 → kernel
- ❌ N16 coverage complete — 应关联 → kernel

**根因**: 早期 dogfood 记录只存 CAS+Fact, 没有执行 edge 链接步骤。直到 Node 16 Skill 更新后才系统性地执行 ADR→KG linking。

**影响**: `paths --src <fact> --dst <entity>` 查询找不到这些 Fact, 导致图谱遍历无法发现大部分历史决策。

**修复建议**: P1 — 补建 16 条 `RelatedTo` 边, 将孤立 Fact 链接到对应的模块 Entity。

### 10.2 边类型分布

| 边类型 | 数量 | 占比 | 来源 |
|--------|------|------|------|
| AssociatesWith | 398 | 93.9% | V-07 自动生成 (tag Jaccard 相似度) |
| PartOf | 20 | 4.7% | Bootstrap (Entity → dogfooding 项目) |
| RelatedTo | 6 | 1.4% | 手动 ADR linking |
| **Causes/Follows/等因果边** | **0** | **0%** | **缺失** |

**问题**: KG 边几乎全是自动生成的 `AssociatesWith` (关联) 而非有意义的因果/语义边。这与公理8 "因果先于关联" 直接矛盾。

### 10.3 存储架构分析

当前 Plico **不使用任何 SQL/结构化数据库**。所有持久化基于文件:

| 子系统 | 存储方式 | 文件 | 当前大小 |
|--------|---------|------|---------|
| **CAS** | 文件系统, SHA-256 → 2级目录分片 | `objects/<2hex>/<cid>` | 192KB (25 files) |
| **KG 节点** | 单体 JSON, 全量读写 | `kg_nodes.json` | 32KB |
| **KG 边** | 单体 JSON, 全量读写 | `kg_edges.json` | **272KB** |
| **事件** | Append-only JSONL | `event_log.jsonl` | 7KB (34 events) |
| **记忆** | JSON→CAS, 索引JSON | `memory_index.json` + CAS | CAS内 |
| **Agent索引** | JSON | `agent_index.json` | 0.5KB |
| **Tag索引** | JSON | `tag_index.json` | 5KB |
| **搜索索引** | HNSW 二进制 (hnsw_rs) | `context/search/` | 12KB |
| **合计** | | | **~560KB** |

**Cargo.toml 中无 DB 依赖**: 无 `rusqlite`/`sled`/`rocksdb`/`redb`/`sqlx`。

### 10.4 当前方案的优劣

**优点**:
1. **零外部依赖** — 部署仅需单二进制, 无 DB 进程
2. **CAS 天然适合文件系统** — 内容寻址 = hash → 文件路径, 与 git 同源
3. **JSONL 事件日志** — append-only, 简洁, 可直接 `jq` 分析
4. **启动快** — 无 DB 连接/WAL 恢复开销
5. **可迁移** — 整个目录可 `cp -r` 备份/转移

**问题**:
1. **KG 全量 JSON 读写** — `kg_edges.json` 272KB, 每次 persist 全量 serialize→write→rename. 424 条边中 398 条是自动 AssociatesWith, 尺寸 O(n²) 增长
2. **无增量更新** — 添加 1 条边 = 重写整个 272KB 文件
3. **无索引** — KG 查询是内存 HashMap 全扫描 (当前规模可接受, 但 >10K 节点会成问题)
4. **无事务保证** — `atomic_write_json` 用 rename 原子性, 但多文件间无 ACID (如节点+边同时修改)
5. **缺 WAL** — 进程崩溃在 persist 之间的修改会丢失

### 10.5 是否需要引入 SQL/结构化存储

**结论: 当前阶段不需要, 但需要规划过渡路径。**

| 维度 | 当前 (文件JSON) | SQLite | 嵌入式KV (redb/sled) |
|------|----------------|--------|---------------------|
| 依赖 | 0 | +1 crate | +1 crate |
| 部署 | 单二进制 | 单二进制+.db | 单二进制+data dir |
| KG 查询 | O(n) scan | O(log n) index | O(1) lookup |
| 增量写 | ❌ 全量 | ✅ single row | ✅ single key |
| ACID | ❌ (rename) | ✅ WAL | ✅ |
| 规模上限 | ~10K edges | ~100M rows | ~TB级 |
| Soul 对齐 | CAS优先 ✅ | 需适配 | 可兼容 |

**建议分阶段**:
1. **现阶段 (N17-N19)**: 保持文件方案, 但修复 KG 增量 persist (仅写变更部分)
2. **中期 (N20+)**: 当 KG 超过 5K 边时, 引入 `redb` (纯 Rust 嵌入式 KV) 替换 KG JSON
3. **长期**: 如果需要复杂图查询 (多跳因果链, 子图匹配), 考虑 SQLite + 递归 CTE

**不推荐**: 全面迁移到 SQL — CAS 的内容寻址语义与 SQL 行存储不兼容, 强行统一会增加复杂度。

---

*审计基于1057个自动化测试(全部通过) + 干净环境CLI实测 + 111个源文件逐一扫描 + Soul 2.0十条公理逐条验证 + minimax评估交叉对比 + dogfood KG实际数据诊断。*
*所有数据可在 `/tmp/plico-n17-audit` 环境中复现。*
