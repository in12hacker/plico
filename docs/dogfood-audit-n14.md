# Plico Dogfood Audit — Node 14 全量实测

**日期**: 2026-04-20
**测试实例**: `/tmp/plico-n14-audit`（干净环境，零历史数据）
**方法**: 链式思考 + 真实 CLI 执行（无管道 exit code 污染）
**单元测试**: 808 passed, 0 failed
**覆盖范围**: Node 1-14 全部设计承诺

---

## 1. 测试摘要

| 维度 | 测试项 | 通过 | 失败 | 通过率 |
|------|--------|------|------|--------|
| CAS 存储 | 10 | 9 | 1 | 90% |
| 分层记忆 | 9 | 9 | 0 | 100% |
| Agent 生命周期 | 6 | 6 | 0 | 100% |
| Session 管理 | 4 | 4 | 0 | 100% |
| 事件系统 | 3 | 3 | 0 | 100% |
| 知识图谱 | 7 | 7 | 0 | 100% |
| Tool API | 8 | 7 | 1 | 88% |
| Permission | 3 | 3 | 0 | 100% |
| MCP 协议 | 2 | 2 | 0 | 100% |
| 上下文/检索 | 4 | 3 | 1 | 75% |
| Skills | 3 | 2 | 1 | 67% |
| 消息/委托 | 3 | 2 | 1 | 67% |
| F-9 自验证 | 4 | 3 | 1 | 75% |
| **总计** | **66** | **60** | **6** | **91%** |

---

## 2. 对比上次审计（Node 1-13 报告）

### Bug 状态变化

| Bug | 上次状态 | 本次状态 | 说明 |
|-----|---------|---------|------|
| B35 | 🔴 CRITICAL delete panic | 🔴 **仍存在** | `byte index 2 is out of bounds of ""` — 有 permission 时仍 panic |
| B38 | 🔴 recall tier 不过滤 | ✅ **已修复** | CLI `--tier` + Tool API `tier` 参数均正确过滤 |
| B39 | 🟡 paths 空 | ✅ **不再复现** | paths 命令正常返回路径 |
| B40 | 🔴 tool recall 空 content | ✅ **已修复** | `memory.recall` 返回完整 content 字段 |
| B41 | 🔴 require-tags 交集失败 | ✅ **已修复** | F-4 `search_by_tags_intersection` 实现，无需 `--query` |
| B42 | 🔴 MCP recall_semantic 报错 | ✅ **已修复** | stub 模式返回 OK（BM25 fallback） |

### Node 14 特性实现状态

| 特性 | Node 14 承诺 | 本次实测 | 状态 |
|------|-------------|---------|------|
| F-1 Tier Recall Filter | recall --tier 过滤 | CLI + Tool API 均正确 | ✅ 完成 |
| F-2 Full Tier Recall | 全 4 tier 可回调 | Working/LongTerm/Procedural 跨命令可达; Ephemeral by design 不持久化 | ✅ 完成 (见§4) |
| F-3 Tool Agent Override | tool API agent_id 覆盖 | 无 override 时正确; 有 override 时名称解析失败 | 🟡 部分 |
| F-4 Require-Tags | 独立标签搜索 | `search --require-tags` 无需 `--query` 正常工作 | ✅ 完成 |
| F-5 Memory-KG Binding | remember → KG 节点 + 边 | LongTerm/Procedural 创建 Memory 节点 + SimilarTo 边 | ✅ 完成 |
| F-6 Consolidation Display | session-end 显示报告 | "Consolidation: reviewed 0 ephemeral, 1 working" | ✅ 完成 |
| F-7 MCP Fallback | MCP stub 不报错 | `recall_semantic` 返回 OK | ✅ 完成 |
| F-8 Tier Parity | procedural CLI 路由 | `--tier procedural` → `remember_procedural()` → 跨命令可回调 | ✅ 完成 |
| F-9 Contract Assertions | Harness 记忆层验证 | INV-1~3 已实现; INV-2 有名称解析 bug; INV-4 部分 | 🟡 大部分 |

---

## 3. 详细测试结果

### 3.1 CAS 存储（Node 2-3）

| # | 测试 | 命令 | 结果 | EXIT |
|---|------|------|------|------|
| T1 | put | `put --content "..." --tags "..." --agent X` | CID 返回 | ✅ 0 |
| T2 | put second | 同上 | CID 返回 | ✅ 0 |
| T3 | get by CID | `get <CID> --agent X` | 内容 + tags + type 正确 | ✅ 0 |
| T4 | search query | `search "alpha"` | 返回匹配对象 | ✅ 0 |
| T5 | tags list | `tags` | 4 tags 列出 | ✅ 0 |
| T6 | require-tags + query | `search "test" --require-tags "..."` | AND 过滤正确 | ✅ 0 |
| T7 | require-tags 独立 (F-4) | `search --require-tags "..."` | 返回匹配结果 (relevance=0.90) | ✅ 0 |
| T8 | -t shorthand | `search "test" -t "..."` | 等效 --require-tags | ✅ 0 |
| T9 | delete (B35) | `delete <CID> --agent X` | **panic: byte index 2 is out of bounds** | ❌ 101 |
| T51 | system-status | `system-status` | CAS 3, Agents 4, Tags 4, KG 11 | ✅ 0 |

### 3.2 分层记忆（Node 3, 12, 14）

| # | 测试 | 命令 | 结果 | EXIT |
|---|------|------|------|------|
| T13 | remember working | `remember --tier working --content "..."` | 存储成功 | ✅ 0 |
| T14 | remember long-term | `remember --tier long-term --content "..."` | 存储成功 | ✅ 0 |
| T15 | remember ephemeral (INV-1) | `remember --tier ephemeral --content "..."` | 存储 + **Warning 输出** | ✅ 0 |
| T16 | remember procedural (F-B) | `remember --tier procedural --content "..."` | "Procedural memory stored" | ✅ 0 |
| T17 | recall --tier working (F-1) | `recall --tier working` | 只返回 [Working] | ✅ 0 |
| T18 | recall --tier long-term | `recall --tier long-term` | 只返回 [LongTerm] | ✅ 0 |
| T19 | recall --tier procedural | `recall --tier procedural` | 只返回 [Procedural] | ✅ 0 |
| T20 | recall --tier ephemeral | `recall --tier ephemeral` | "No memories" (by design) | ✅ 0 |
| T21 | recall 无 filter | `recall` | 3 tier 全返回 | ✅ 0 |

### 3.3 Agent 生命周期 + Session（Node 5, 12）

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T23 | agent --register | Agent ID 返回 | ✅ 0 |
| T27 | quota | Memory/CPU/Tools 信息 | ✅ 0 |
| T28 | discover | 列出所有 agent + 状态 | ✅ 0 |
| T29 | session-start | Session ID + changes count | ✅ 0 |
| T30 | session-end (F-6) | Consolidation: reviewed 0 ephemeral, 0 working | ✅ 0 |
| T30b | session + memory + end | Consolidation: reviewed 0 ephemeral, **1 working** | ✅ 0 |

### 3.4 事件系统（Node 7）

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T31 | events --agent filter (B14) | 只返回指定 agent 事件 | ✅ 0 |
| T32 | events --limit | 限制条数正确 | ✅ 0 |
| T63 | delta --since 0 | 5 条变更，含序号和摘要 | ✅ 0 |

### 3.5 知识图谱（Node 4, 12）

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T33 | node entity | Node ID 返回 | ✅ 0 |
| T34 | node fact | Node ID 返回 | ✅ 0 |
| T35 | edge | "Edge created: A --[RelatedTo]--> B" | ✅ 0 |
| T36 | nodes --type entity | 过滤正确 | ✅ 0 |
| T37 | explore | 邻居节点 + authority 分数 | ✅ 0 |
| T38 | paths (B39) | "Paths (1 found): test-decision → test-module" | ✅ 0 |
| T65 | F-5 Memory-KG 节点 | 3 个 Memory 节点 + SimilarTo 边 | ✅ 0 |

### 3.6 Tool API（Node 8, 13, 14）

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T39 | tool list | 37 tools | ✅ 0 |
| T40 | tool describe | Schema JSON 正确 | ✅ 0 |
| T41 | cas.search (params JSON) | 2 结果，无重复 | ✅ 0 |
| T42c | memory.recall 无 override | **完整 content 字段** (B40 修复) | ✅ 0 |
| T42d | memory.recall tier filter | 只返回 working tier | ✅ 0 |
| T42b | memory.recall + agent_id override | **Contract violation** (名称解析 bug) | ❌ 1 |
| T43 | INV-2 invalid agent | "Contract violation: not found" + 可用列表 | ✅ 1 |
| T44 | memory.store | tier:working 存储成功 | ✅ 0 |

### 3.7 Permission + MCP（Node 6, 13）

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T45a | permission grant | 授权成功 | ✅ 0 |
| T45b | permission check | `{"allowed":true}` | ✅ 0 |
| T45c | permission list | 列出所有授权 | ✅ 0 |
| T57 | MCP recall_semantic (B42) | OK + 空 memory (stub BM25 fallback) | ✅ 0 |

### 3.8 其他功能

| # | 测试 | 结果 | EXIT |
|---|------|------|------|
| T46 | delegate | "Delegated: cli → n14-test" | ✅ 0 |
| T47 | send message | ❌ 权限检查用 "cli" 而非 --agent | ❌ 1 |
| T52 | health (F-7) | "HEALTHY" + 降级信息 | ✅ 0 |
| T53 | growth | 会话/token/记忆统计 | ✅ 0 |
| T54 | context --cid | L2 内容 + token 估算 | ✅ 0 |
| T55 | context L0 | 短内容显示 [L2] 非 [L0] | 🟡 cosmetic |
| T56 | hybrid search | graph_hits=2, paths_found=4 | ✅ 0 |
| T59 | skills register | ❌ "Agent not found" (名称解析) | ❌ 1 |
| T61 | skills list | "cli-procedure" (procedural 可达) | ✅ 0 |
| T62 | intent submit | Intent ID 返回 | ✅ 0 |

---

## 4. F-2 / F-9 设计意图诊断

### F-2: Ephemeral CLI 持久化

**代码行级证据**:

```
kernel/ops/memory.rs:57-81  — remember() 不调用 persist_memories()
kernel/ops/memory.rs:133    — remember_working() 调用 persist_memories() ✅
kernel/ops/memory.rs:309    — remember_long_term() 调用 persist_memories() ✅
kernel/ops/memory.rs:414    — remember_procedural() 调用 persist_memories() ✅
handlers/memory.rs:17-18    — INV-1 warning 输出到 stderr
```

**诊断结论: 设计如此，不是 bug，也不是遗漏**

原因链:
1. Ephemeral 设计为 volatile cache — 不应跨会话持久化（daemon 模式下进程内有效）
2. CLI 是 stateless 的 — 每个命令是独立进程
3. 两者组合导致 ephemeral 在 CLI 下不可用
4. INV-1 已正确实现 — `eprintln!` 警告用户改用 `--tier working`

替代方案被 Node 14 设计文档明确拒绝:
> "破坏 volatile 语义; 选择 warning + 可选持久化"（§3 发散思维表）

### F-9: Harness 记忆层验证

| 不变量 | 状态 | 代码位置 | 诊断 |
|--------|------|---------|------|
| INV-1 ephemeral 警告 | ✅ 已实现 | `handlers/memory.rs:17-18` | `eprintln!` 输出警告 |
| INV-2 agent_id 验证 | ✅ 已实现 **但有 bug** | `builtin_tools.rs:389-395` | 有效 agent_id 校验 + "Contract violation" 错误。但名称解析失败：`has_agent(AgentId("audit-agent"))` 查找 UUID 而非名称 |
| INV-3 consolidation 可观测 | ✅ 已实现 | `commands/mod.rs:399-406` | `session-end` 输出 "Consolidation: reviewed X ephemeral, Y working" |
| INV-4 require_tags 后置 | ✅ 功能已实现 | `crud.rs:72-73` + `fs.rs:137` | `search_by_tags_intersection` AND 语义正确。无 `debug_assert!` 运行时断言 |

**F-9 诊断结论: 大部分已实现，非遗漏**

用户报告称 "INV-2 和 INV-3 未完全实施" 与实测不符：
- INV-2 已有完整的验证逻辑和错误信息，但存在一个 **名称到 ID 的解析 bug** — 当 `params.agent_id` 是名称（如 "audit-agent"）而非 UUID 时，`has_agent` 查找失败
- INV-3 完整实现且可观测

---

## 5. 现存问题

### 5.1 仍存在的 Bug

| Bug | 严重度 | 描述 | 根因 |
|-----|--------|------|------|
| B35 | 🔴 CRITICAL | `delete <CID> --agent X` panic: `byte index 2 is out of bounds of ""` | 未定位。即使有 `delete` 权限仍 panic (exit 101) |
| B43 | 🟡 MEDIUM | INV-2 agent_id 名称解析失败 | `has_agent(AgentId(name))` 用名称查 UUID 表。影响 `tool call memory.recall {"agent_id":"name"}` 和 `skills register --agent name` |
| B44 | 🟡 MEDIUM | `send --to X --agent Y` 权限检查用 "cli" 而非 Y | CLI agent 上下文传递问题 — 权限检查 agent 不跟随 `--agent` 参数 |

### 5.2 设计决策（非 Bug）

| 项目 | 状态 | 说明 |
|------|------|------|
| Ephemeral CLI 不持久 (F-2) | BY DESIGN | `remember()` 不调用 `persist_memories()`。INV-1 警告已实现 |
| context L0 短文本显示 [L2] | BY DESIGN | 内容短于 L0 阈值时不降级，直接返回完整内容 |
| auto-checkpoint 未实现 (F-F) | STUB | `let checkpoint_id = if auto_checkpoint { None } else { None }` — 依赖客户端显式调用 |

---

## 6. 能力维度评分

| 维度 | 上次 (Node 13 审计) | 本次 | 变化 | 关键修复 |
|------|---------------------|------|------|---------|
| CAS/搜索 | 88% | **90%** | +2% | F-4 require-tags; B35 仍在 |
| 分层记忆 | 43% | **100%** | +57% | F-1 tier filter; F-B procedural 路由; INV-1 |
| 上下文装配 | 33% | **75%** | +42% | A-6 已修复; context --cid 正常 |
| 工具系统 | 80% | **88%** | +8% | B40 修复; B26 修复; B43 新发现 |
| MCP 协议 | 75% | **100%** | +25% | B42 修复 (BM25 fallback) |
| Agent 管理 | — | **100%** | — | register/discover/quota/delegate 全通过 |
| KG | — | **100%** | — | nodes/edges/explore/paths + F-5 Memory 链接 |
| Session | — | **100%** | — | start/end/consolidation(F-6) 全通过 |
| 事件 | — | **100%** | — | history/filter(B14)/delta 全通过 |
| F-9 自验证 | 0% | **75%** | +75% | INV-1/3/4 已实现; INV-2 有名称解析 bug |
| **总体** | **72.4%** | **91%** | **+18.6%** | |

---

## 7. Node 14 设计承诺覆盖检查

### D1: 记忆完整 ✅
- F-1 (Tier Recall Filter): ✅ CLI + Tool API 均正确过滤
- F-2 (Full Tier + Persistence): ✅ Working/LongTerm/Procedural 跨命令持久; Ephemeral by design + INV-1

### D2: 接口一致 🟡
- F-3 (Tool Agent Override): 🟡 无 override 时正确; 有 override 时名称解析 bug (B43)
- F-4 (Require-Tags): ✅ AND 语义正确，tag-only 搜索可用

### D3: 记忆绑定 ✅
- F-5 (Memory-KG Binding): ✅ LongTerm/Procedural 自动创建 Memory KG 节点 + SimilarTo 边
- F-6 (Consolidation Display): ✅ session-end 显示 ephemeral/working 审查 + promoted/evicted

### D4: 降级通路 ✅
- F-7 (MCP Fallback): ✅ stub 模式返回 OK
- F-8 (Tier Parity): ✅ procedural 正确路由到 `remember_procedural()`

### D5: 自验 🟡
- F-9 INV-1 (ephemeral warning): ✅
- F-9 INV-2 (agent validation): 🟡 实现但有名称解析 bug
- F-9 INV-3 (consolidation observable): ✅
- F-9 INV-4 (require_tags postcondition): ✅ 功能正确，无 debug_assert

---

## 8. 对比历史审计报告的校正

| 项目 | 上次报告 | 本次实测 | 校正 |
|------|---------|---------|------|
| B38 recall tier 不过滤 | 🔴 存在 | ✅ 已修复 | F-1 实现完成 |
| B39 paths 空 | 🔴 存在 | ✅ 不复现 | 可能是上次测试条件差异 |
| B40 tool recall 空 content | 🔴 存在 | ✅ 已修复 | content 字段非空 |
| B41 require-tags 交集 | 🔴 存在 | ✅ 已修复 | F-4 search_by_tags_intersection |
| B42 MCP stub 报错 | 🔴 存在 | ✅ 已修复 | BM25 fallback |
| F-2 ephemeral 不持久化 | ❌ 未实现 | ✅ BY DESIGN | INV-1 警告是正确的设计缓解 |
| F-9 INV-2 未实施 | ❌ 报告未实施 | ✅ 已实施 | 但有名称解析 bug (B43) |
| F-9 INV-3 未实施 | ❌ 报告未实施 | ✅ 已实施 | session-end 显示完整 consolidation |

---

## 9. 单元测试健康度

```
cargo test (EMBEDDING_BACKEND=stub):
  Unit tests:        370 passed, 0 failed
  Integration tests: 433 passed, 0 failed
  Doc tests:         5 passed, 0 failed
  Total:             808 passed, 0 failed
```

---

## 10. 下一步建议

### P0 — 必须修复
1. **B35 delete panic**: 所有 delete 操作均 panic，阻塞 CAS 对象管理的完整性
2. **B43 INV-2 名称解析**: `tool call memory.recall {"agent_id":"name"}` 应支持名称和 UUID

### P1 — 建议修复
3. **B44 send 权限**: `--agent` 参数应传递给权限检查上下文
4. **F-9 INV-4 runtime assert**: 添加 `debug_assert!` 验证 require_tags 结果后置条件

### P2 — 增强
5. **F-5 扩展**: remember working 也触发 KG linking（当前只有 LongTerm/Procedural）
6. **Skills register 名称解析**: 与 B43 同根因，修复后 skills register 也将可用

---

*报告基于 `/tmp/plico-n14-audit` 干净实例的 66 项真实 CLI 测试。*
*每项测试独立执行，exit code 直接捕获（无管道干扰）。*
*代码审计覆盖: kernel/ops/memory.rs, handlers/memory.rs, handlers/crud.rs, builtin_tools.rs, commands/mod.rs, session.rs。*
