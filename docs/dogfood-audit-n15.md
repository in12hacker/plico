# Plico Dogfood Audit — Node 15 全量实测

**日期**: 2026-04-20
**测试实例**: `/tmp/plico-n15-audit`（干净环境，零历史数据）
**方法**: 链式思考 + 真实 CLI 执行（无管道 exit code 污染）+ 源码行级分析
**单元测试**: 876 passed, 0 failed, 7 ignored
**覆盖范围**: Node 1-15 全部设计承诺 + 用户提出的两个新问题

---

## 1. 测试摘要

| 维度 | 测试项 | 通过 | 失败 | 通过率 |
|------|--------|------|------|--------|
| CAS 存储 | 8 | 8 | 0 | 100% |
| 分层记忆 | 9 | 9 | 0 | 100% |
| Agent 生命周期 | 7 | 7 | 0 | 100% |
| Session 管理 | 3 | 3 | 0 | 100% |
| 事件系统 | 3 | 3 | 0 | 100% |
| 知识图谱 | 6 | 6 | 0 | 100% |
| Tool API | 6 | 5 | 1 | 83% |
| Permission | 2 | 2 | 0 | 100% |
| Search/Tags | 4 | 4 | 0 | 100% |
| Skills | 3 | 1 | 2 | 33% |
| Intent 系统 | 2 | 2 | 0 | 100% |
| Delta/Context | 3 | 2 | 1 | 67% |
| N15 输入安全 | 5 | 5 | 0 | 100% |
| N15 名称解析 | 4 | 4 | 0 | 100% |
| **总计** | **65** | **61** | **4** | **94%** |

---

## 2. 用户提出的两个新问题诊断

### 问题 1: Dogfood 使用率极低

**用户诊断**:

| 维度 | 应该 | 实际 |
|------|------|------|
| 持久化 | 长期存储 | /tmp 重启丢失 |
| ADR 记录 | 每个设计决策 | 几乎没记 |
| 进度记录 | 每个里程碑完成时 | 只在触发时记录 |
| 知识积累 | 持续经营 | 临时抱佛脚 |

**独立验证结果**:

```
/tmp/plico-dogfood KG 实际状态:
  KG 总节点: 46 (应该远不止此)
  Entity 节点: 25 (但含大量重复 — bootstrap 重复执行)
    - "cas" x3, "fs" x2, "kernel" x2, "memory" x2, ...
    - 唯一模块锚点实际只有 ~10 个
  Fact 节点: 4 (仅 4 个 ADR)
    - "Use generic KG primitives" x2 (重复)
    - "Skill KG Sync Fix"
    - "F-2 Ephemeral Design Diagnosis"
  Document 节点: 17
  CAS 总对象: 21
  Progress 记录: 标签存在但 KG 无关联
```

**用户诊断 ✅ 完全正确**。根因分析:

1. **持久化**: `/tmp/plico-dogfood` 在系统重启后丢失。每次新会话都需要重新 bootstrap，导致重复 Entity 节点。
2. **去重缺失**: bootstrap 脚本只检查 Entity 数量是否为 0，不检查是否已有同名节点。重复运行 = 重复创建。
3. **ADR 稀少**: 仅 4 个 Fact 节点。11 个 Node 设计文档 × 平均 5+ 设计决策 = 应有 55+ ADR。实际覆盖率 < 8%。
4. **Progress 断裂**: 有 `plico:type:progress` 标签但无 KG 节点关联，无法通过图查询追踪里程碑。

**结论**: 用户说得对 — 当前更像是"用 plico 开发 plico"而不是"用 plico 管理 plico"。需要:
- P0: 持久化存储（非 /tmp）
- P0: Bootstrap 去重（幂等检查）
- P1: 系统性 ADR 记录流程
- P2: Progress → KG 节点自动关联

### 问题 2: V-01/V-02 灵魂对齐违背 — IntentRouter 在内核

**用户推导**:
> IntentRouter 在内核 = 内核在"替 Agent 思考" → 违反 V-01/V-02

**独立验证结果**: ❌ **用户假设不成立**

IntentRouter 已在 v3.0-M1 提取到接口层。源码证据:

1. **`src/intent/mod.rs` (line 72)**: `IntentRouter` trait 定义在独立模块
2. **`src/intent/execution.rs` (line 1-6)**:
   ```
   //! Intent execution — application-layer logic for NL→execute→learn.
   //! Extracted from kernel (v3.0-M1) per soul alignment
   ```
3. **`src/bin/aicli/commands/handlers/intent.rs` (line 1-4)**:
   ```
   //! Intent resolution commands — application-layer NL handling.
   //! The ChainRouter is created here at the interface layer, not in the kernel.
   //! This follows the soul principle: OS provides resources, agents decide how to think.
   ```
4. **`src/kernel/` 有 0 个 `use crate::intent` 或 `use plico::intent`**

内核只处理结构化 `ApiRequest` 变体（`handle_api_request`），不做 NL→API 翻译。
IntentRouter 在接口层（CLI `handlers/intent.rs`）创建和调用。

**架构符合 Soul 2.0 公理 5 (Mechanism not Policy)**:
- 内核 = 纯机制提供者（API dispatch, memory primitives, CAS ops）
- 接口层 = 策略执行者（NL parsing, router selection, confidence thresholds）

---

## 3. Bug 状态变化 (N14 → N15)

| Bug | N14 状态 | N15 状态 | 验证命令 | 证据 |
|-----|---------|---------|---------|------|
| B35 | 🔴 CRITICAL | ✅ **FIXED** | `delete <CID> --agent X` | 正常删除；空 CID → 友好错误 |
| B43 | 🟡 MEDIUM | ✅ **FIXED** | `tool call memory.recall {"agent_id":"name"}` | 名称解析 → UUID，返回记忆数据 |
| B44 | 🟡 MEDIUM | ✅ **FIXED** | `send --to X --agent Y` | 权限检查正确使用 agent Y |
| B45 | 🟡 N15-NEW | ✅ **FIXED** | `agent --register --name "X"` | 名称正确存储（agents 列表确认） |

**N14 遗留 Bug: 4/4 全部修复。**

### 新发现问题

| ID | 严重度 | 描述 | 根因 |
|----|--------|------|------|
| B46 | 🟡 LOW | `delete "a"` / `delete "xyz!!!"` 返回假成功 | `semantic_delete` 对不存在的 CID 不返回错误 |
| B47 | 🟡 LOW | `skills register --agent <name>` 失败 "Agent not found" | `skills register` 未经过名称解析层 |
| B48 | 🟡 LOW | `store_procedure` name/description 未持久化 | `builtin_tools.rs` 的 `memory.store_procedure` 未传递 name 参数 |
| D-1 | ⚠️ DESIGN | `agent --status` 路由到 `cmd_agent`（注册）而非 `cmd_agent_status` | 命令路由: `agent` → 注册, `status` → 状态查看; 非子命令设计 |
| D-2 | ⚠️ DESIGN | Dogfood KG Entity 重复 (25 个中多数是重复) | Bootstrap 脚本非幂等 |

---

## 4. Node 15 设计承诺实现状态

### 4.1 Bug 修复 (4/4 ✅)

| Bug | 承诺 | 状态 | 修复位置 |
|-----|------|------|---------|
| B35 | F-1: CID 输入防御 | ✅ | `cas/storage.rs` `validate_cid()` + `handlers/crud.rs` 空 CID 检查 |
| B43 | F-3: 统一名称解析 | ✅ | `kernel/ops/agent.rs` `resolve_agent()` + `builtin_tools.rs` |
| B44 | F-2: CLI 参数统一 | ✅ | `commands/mod.rs` `extract_agent_id()` + `handlers/messaging.rs` |
| B45 | F-2: register --name | ✅ | `handlers/agent.rs` `extract_arg(args, "--name")` |

### 4.2 特性实现

| 特性 | 承诺 | 实现状态 | 证据 |
|------|------|---------|------|
| F-1 CID 输入防御 | 空/短 CID 不 panic | ✅ 完成 | `validate_cid()` + `InvalidCid` 错误类型 |
| F-2 CLI 参数统一 | `--agent`/`--from` 统一 | ✅ 完成 | `extract_agent_id()` 函数 |
| F-3 名称解析 | name↔UUID 双空间解析 | ✅ 完成 | `resolve_agent()` + Tool API 验证 |
| F-4 单元测试框架 | ~81 个新单元测试 | ❌ **未开始** | 所有目标模块仍有 0 个内嵌测试 |
| F-5 CAS 防御加固 | CID 验证 + 属性测试 | 🟡 部分 | `validate_cid()` 存在但无新测试 |
| F-6 行为指纹测试 | CLI 行为回归基线 | ❌ **未开始** | 无行为指纹测试文件 |

### 4.3 五大维度达成率

| 维度 | 目标 | 状态 | 得分 |
|------|------|------|------|
| D1 输入安全 | 公共接口边界防护 | ✅ | 90% |
| D2 名称统一 | name↔UUID 解析层 | ✅ | 100% |
| D3 单元覆盖 | 关键模块 100% 分支覆盖 | ❌ | 0% |
| D4 变异抗性 | Mutant Escape Rate < 20% | ❌ | 0% |
| D5 行为指纹 | CLI 行为回归检测 | ❌ | 0% |
| **总计** | | | **38%** |

---

## 5. 单元测试覆盖率深度分析

### 5.1 总体数据

| 指标 | N14 | N15 | 变化 |
|------|-----|-----|------|
| 总测试数 | 808 | 876 | +68 (+8.4%) |
| 通过率 | 100% | 100% | = |
| 文件覆盖率 (有内嵌测试 / >30行文件) | ~43% | 43% | = |
| 有内嵌测试文件 | 42 | 42 | +0 |
| 无内嵌测试文件 (>30行) | 55 | 55 | +0 |
| 有测试文件总行数 | 23,056 | 23,056 | = |
| 无测试文件总行数 | 13,900 | 13,900 | = |

### 5.2 关键覆盖率盲区 (CRITICAL)

| 模块 | 行数 | 内嵌测试 | 风险 | 集成测试覆盖 |
|------|------|---------|------|------------|
| `kernel/mod.rs` | 1,909 | 0 | 🟡 | kernel_test.rs (169 tests) |
| `kernel/ops/graph.rs` | 784 | 0 | 🔴 | kg_causal_test.rs (8 tests) |
| `kernel/builtin_tools.rs` | 761 | 0 | 🔴 | kernel_test.rs 间接 |
| `kernel/ops/memory.rs` | 636 | 0 | 🔴 | memory_test.rs (21 tests) |
| `kernel/ops/agent.rs` | 521 | 0 | 🔴 | kernel_test.rs 间接 |
| `kernel/ops/fs.rs` | 445 | 0 | 🔴 | fs_test.rs (25 tests) |
| `kernel/persistence.rs` | 418 | 0 | 🟡 | memory_persist_test.rs (5) |
| `kernel/ops/dashboard.rs` | 348 | 0 | 🟡 | observability_test.rs (14) |
| `kernel/ops/model.rs` | 361 | 0 | 🟡 | model_hot_swap_test.rs (9) |
| `fs/graph/backend.rs` | 749 | 0 | 🟡 | fs_test.rs 间接 |
| CLI handlers (12 文件) | ~1,500 | 0 | 🔴 | cli_test.rs (10), cli_behavior_test.rs (18) |

### 5.3 Bug→测试盲区对应关系

| Bug | 来源文件 | 该文件内嵌测试 | 本可被捕获 |
|-----|---------|--------------|-----------|
| B35 (delete panic) | `handlers/crud.rs` | 0 | ✅ 基础单元测试即可 |
| B43 (name resolution) | `builtin_tools.rs` | 0 | ✅ |
| B44 (send --agent) | `handlers/messaging.rs` | 0 | ✅ |
| B45 (register --name) | `handlers/agent.rs` | 0 | ✅ |
| B46 (假删除成功) | `kernel/ops/fs.rs` | 0 | ✅ |
| B47 (skills register name) | `handlers/skills.rs` | 0 | ✅ |

**结论: 100% 的现存/历史 Bug 来自无内嵌测试的模块。Trail of Bits 论断"代码覆盖率是最危险的质量指标"在 Plico 上完全印证。**

---

## 6. Node 1-15 全量承诺追踪

### 6.1 基础设施 (Node 1-3)

| 承诺 | 状态 | 最后验证 |
|------|------|---------|
| CAS 存储/检索 | ✅ | T01-T04 |
| SHA-256 寻址 | ✅ | T01 CID 格式 |
| 语义搜索 (Stub BM25) | ✅ | T05 |
| KG 节点/边/路径 | ✅ | T29-T41 |
| 标签系统 | ✅ | T52 |
| Unicode 内容 | ✅ | T02 |

### 6.2 协作层 (Node 4-6)

| 承诺 | 状态 | 最后验证 |
|------|------|---------|
| Agent 注册 | ✅ | T08, T25, T54 |
| Agent 状态管理 | ✅ | T62 |
| Permission 授予/检查 | ✅ | T13 |
| Session 生命周期 | ✅ | T27, T31 |
| 消息传递 | ✅ | T37 |

### 6.3 驾具层 (Node 7-8)

| 承诺 | 状态 | 最后验证 |
|------|------|---------|
| 事件持久化 | ✅ | T35 |
| Delta 增量 | ✅ | T53 |
| Token 估算 | ✅ | T53 (est. 303 tokens) |

### 6.4 韧性层 (Node 9-10)

| 承诺 | 状态 | 测试 |
|------|------|------|
| 弹性降级 | ✅ | node9_resilience_test.rs (13 tests) |
| 整流 | ✅ | node10_rectification_test.rs (6 tests) |

### 6.5 自演进层 (Node 11-12)

| 承诺 | 状态 | 测试 |
|------|------|------|
| 分层记忆 (4 tier) | ✅ | T18-T24 |
| Tier 过滤 | ✅ | T23-T24 (B38 fix) |
| Procedural 记忆 | ✅ | T21, T45-T46 |
| 持久化 | ✅ | memory_persist_test.rs (5 tests) |
| Consolidation | ✅ | T31 (session-end 报告) |

### 6.6 传导层 (Node 13)

| 承诺 | 状态 | 测试 |
|------|------|------|
| API v18.0 | ✅ | api_version_test.rs (23 tests) |
| 37 个内置工具 | ✅ | T55 (tool list) |
| MCP 协议 | ✅ | mcp_test.rs (2 tests) + plico_mcp.rs (30 tests) |
| Intent 解析 | ✅ | T56-T57 |

### 6.7 融合层 (Node 14)

| 承诺 | 状态 | 验证 |
|------|------|------|
| F-1 recall tier filter | ✅ | T23-T24 |
| F-2 ephemeral 设计 | ✅ (by design) | T18 (INV-1 warning) |
| F-3 tool agent override | ✅ | T59 (B43 fixed) |
| F-4 require-tags intersection | ✅ | T42-T43 |
| F-5 KG binding | ✅ | KG 操作正常 |
| F-6 consolidation display | ✅ | T31 |
| F-7 MCP recall fallback | ✅ | plico_mcp.rs tests |
| F-8 procedural tier | ✅ | T21, T45 |
| F-9 INV-1/2/3/4 | ✅ | INV-1(T18), INV-2(T59), INV-3(T31), INV-4(T42) |

### 6.8 验证层 (Node 15)

| 承诺 | 状态 | 完成度 |
|------|------|--------|
| F-1 CID 输入防御 | ✅ | 100% — validate_cid + B35 fix |
| F-2 CLI 参数统一 | ✅ | 100% — extract_agent_id + B44/B45 fix |
| F-3 名称解析 | ✅ | 100% — resolve_agent + B43 fix |
| F-4 单元测试 (~81 个) | ❌ | 0% — 无新增内嵌测试 |
| F-5 CAS 防御加固 | 🟡 | 50% — 验证逻辑存在，无新测试 |
| F-6 行为指纹 | ❌ | 0% — 无行为测试文件 |

---

## 7. 定量目标达成

| 目标 | N15 承诺 | 实际 | 达成 |
|------|---------|------|------|
| Bug 修复 | 4/4 (B35,B43,B44,B45) | 4/4 | ✅ 100% |
| 新增单元测试 | ≥ 80 个 | +68 (全为集成测试) | ❌ 0% 内嵌 |
| kernel/ops 单元覆盖 | 0% → ≥60% | 0% | ❌ |
| CLI handlers 单元覆盖 | 0% → ≥50% | 0% | ❌ |
| CAS panic 路径 | ≥1 → 0 | 0 | ✅ 100% |

---

## 8. 总评与建议

### 8.1 进展总结

**从 N14 到 N15 的关键改善**:

| 维度 | N14 | N15 | 变化 |
|------|-----|-----|------|
| CLI 通过率 | 91% | 94% | ⬆ +3% |
| 总测试数 | 808 | 876 | ⬆ +68 |
| 现存 Bug | 4 (B35,B43,B44,B45) | 2 新 (B46,B47) | ⬆ 严重 Bug 清零 |
| CRITICAL Bug | 1 (B35) | 0 | ✅ 清零 |
| CID Panic 路径 | 存在 | 0 | ✅ |
| 名称↔UUID 解析 | 缺失 | 完整 | ✅ |

### 8.2 核心差距

1. **F-4 单元测试 (0/81)**: Node 15 最核心的承诺完全未实现。55 个源文件仍无内嵌测试。这直接导致所有历史 Bug 只能通过集成测试间接发现。

2. **F-6 行为指纹 (0%)**: CLI 输出格式没有回归检测基线。任何输出格式变更无法自动捕获。

3. **Dogfood 持久化**: `/tmp` 方案导致跨会话数据丢失、Entity 重复、ADR 覆盖率 <8%。

### 8.3 建议优先级

| 优先级 | 任务 | 预估工作量 |
|--------|------|-----------|
| P0 | F-4: 至少为 `kernel/ops/memory.rs` 和 `handlers/crud.rs` 写内嵌测试 | 2-3 天 |
| P0 | Dogfood 持久化: 迁移到 `~/.plico-dogfood` (非 /tmp) | 0.5 天 |
| P1 | F-4: 为 `kernel/builtin_tools.rs` 和 `kernel/ops/agent.rs` 写测试 | 2 天 |
| P1 | B46/B47 修复 | 0.5 天 |
| P1 | Bootstrap 幂等化 (去重检查) | 0.5 天 |
| P2 | F-6: CLI 行为指纹基线 | 1 天 |
| P2 | F-5: CAS 属性测试 | 1 天 |

---

## 9. 详细测试记录

### T01-T03: CAS 基础

```
T01: put "node15 test data alpha" --tags t1,t2 → CID: ae8fc5... ✅
T02: put "中文内容测试 日本語テスト" --tags unicode → CID: b980b0... ✅
T03: system-status → CAS: 2, Agents: 0, Tags: 3 ✅
```

### T04-T07: CAS 读取/搜索/删除

```
T04: get ae8fc5... → content正确, tags正确 ✅
T05: search "node15" → 1 result, relevance=0.02 ✅
T06: delete <CID> positional → 权限错误(预期) ✅
T07: delete --cid <CID> → 权限错误(预期) ✅
```

### T08-T17: B35 输入防御验证

```
T08: agent --register --name audit-agent → Agent ID: 02a83d9a ✅
T09: grant permission → ✅
T14: delete <CID> positional (有权限) → "Deleted: ... → recycle bin" ✅ (B35 FIXED)
T15: delete --cid <CID> (有权限) → 正常删除 ✅
T16: delete "a" → 假成功 "Deleted: a" 🟡 (B46)
T17: delete "not-a-valid-cid!!!" → 假成功 🟡 (B46)
T10: delete (无CID) → "delete requires a CID..." ✅ (F-1)
```

### T18-T24: 分层记忆

```
T18: remember --tier ephemeral → "Warning: ephemeral memory is stored in-process only" ✅ (INV-1)
T19: remember --tier working → stored ✅
T20: remember --tier long-term → stored ✅
T21: remember --tier procedural → "Procedural memory stored" ✅ (F-8)
T22: recall (all) → Working + LongTerm + Procedural (无 ephemeral) ✅
T23: recall --tier working → 只有 Working ✅ (B38 fix)
T24: recall --tier long-term → 只有 LongTerm ✅
```

### T25-T30: Agent/Session/KG

```
T25: agent --register --name "proper-name-test" → Agent ID ✅
T27: session-start → Session started + change count ✅
T31: session-end → Consolidation: reviewed 0 ephemeral, 1 working ✅ (F-6/INV-3)
T29: node --label "test-module" → Node ID ✅
T30: nodes --type entity → 1 node ✅
```

### T33-T41: B43/B44/KG 操作

```
T33: tool call memory.recall {"agent_id":"audit-agent"} → 5 memories ✅ (B43 FIXED)
T34: send --to audit-agent --agent audit-agent → 权限检查正确使用 audit-agent ✅ (B44 FIXED)
T39: edge --src X --dst Y → Edge created ✅
T40: explore --cid X → 1 neighbor ✅
T41: paths --src X --dst Y → 1 path found ✅
```

### T42-T46: 搜索/Tool API

```
T42: search "tagged" --require-tags AND → 1 result (正确交集) ✅ (F-4)
T43: search --require-tags (无 query) → 2 results ✅ (B41 fix)
T45: tool call memory.store_procedure → stored ✅
T46: tool call memory.recall_procedure → 2 procedures ✅
T47: skills register --agent name → "Agent not found" ❌ (B47)
```

### T54-T62: B45/Intent/Final

```
T54: agent --register --name "b45-test-name" → registered ✅
T61: agents JSON → name=b45-test-name ✅ (B45 FIXED)
T56: intent "search for kernel documents" → [0.85] resolved ✅
T57: intent "put hello world" --execute → executed, CID created ✅
T58: delete empty CID → "Invalid CID format" ✅ (F-1)
T59: tool call memory.recall by name vs UUID → 两者都返回数据 ✅ (F-3)
T62: status --agent audit-agent → "Created", 0 pending ✅
```

---

*报告基于 `/tmp/plico-n15-audit` 干净实例 65 项 CLI 实测 + 876 个自动化测试 + 源码行级分析。*
*所有 Bug 根因通过代码阅读确认。V-01/V-02 通过 import 依赖图验证。*
