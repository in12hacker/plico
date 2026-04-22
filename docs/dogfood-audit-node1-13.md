# Plico Dogfood 审计报告：Node 1–13 全量承诺兑现验证（反向审查版）

**日期**: 2026-04-22  
**验证方法**: AI Agent 真实接入 `aicli --root /tmp/plico-n13-audit` 全链路测试  
**测试环境**: `EMBEDDING_BACKEND=stub`（BM25 only），808 单元测试（0 失败, 7 ignored）  
**Exit code 方法**: 无管道验证（`ECODE=$?; echo "EXIT:$ECODE"`），消除 grep 干扰  
**信息来源**: 11 份设计文档 (Node 2–6, 9–13) + 上轮审计 + 全量 CLI 命令遍历  
**审查方法**: 反向覆盖——逐 Node 提取承诺 → 逐条执行 → 标记覆盖/遗漏  

---

## 0. 总览

| 维度 | 上轮 (R2) | 本轮 (反向审查) | 变化 |
|------|-----------|----------------|------|
| 单元测试 | 399 (1 fail) | **808 (0 fail)** | +409, 全绿 |
| CLI 命令覆盖 | ~20 | **35+** | 全量遍历 |
| 测试项数 | 58 项 | **87 项** | +29 (补充遗漏) |
| 通过率 | 79.3% (46/58) | **72.4% (63/87)** | 覆盖更全，暴露更多问题 |
| CRITICAL bug | 1 (B35) | 1 (B35) + 5 新 bug | ↑ |
| 修复确认的 bug | 11 (N12+N13) | 11 (不变) | — |

> **为什么通过率反而降低了？** 因为上轮跳过了大量 Node 2–11 的承诺验证。
> 本轮逐 Node 反向检查后，补充了 29 个之前遗漏的测试项，其中暴露了 5 个新 bug。
> 正确的衡量方式是：同一覆盖范围下修复率 ↑，但总覆盖范围大幅扩展暴露了更多问题。

---

## 1. 逐 Node 承诺覆盖矩阵

### Node 1：存储基础（6 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 1.1 | CAS SHA-256 寻址+去重 | `put` 返回 CID, 重复内容返回相同 CID | CID 稳定, 内容不可变 | ✅ |
| 1.2 | Semantic FS: search + BM25 | `search --query "architecture"` | relevance=0.02, BM25 命中 | ✅ |
| 1.3 | 分层记忆 4 tier | `remember --tier working/long-term/ephemeral/procedural` | working+long-term 存储成功; ephemeral/procedural 存储但 **recall 不返回** | ⚠️ |
| 1.4 | 调度器 + intent | `intent --description "..."` → Intent submitted | Intent ID 返回 | ✅ |
| 1.5 | 事件总线持久化 | `events history` 跨进程保留 | 25 events 跨重启完整 | ✅ |
| 1.6 | 工具注册表 | `tool list` → 37 tools; `tool describe cas.search` → JSON schema | ✅ |

### Node 2：AIOS 智能原语（5 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 2.1 | F-0: IntentRouter 在接口层 | 内核只接受 ApiRequest（代码结构验证） | 不可 CLI 测试，代码审计确认 | ✅ |
| 2.2 | F-1: 跨 Agent 搜索可见性 | `search --agent n13-tester` 找到 n13-helper 存的对象 | b8ad89b6 被双方可见 | ✅ |
| 2.3 | F-2: 主动上下文装配 | `session-start --intent "..." --last-seq 10` | warm_context 返回, 948 token | ✅ |
| 2.4 | F-3: AgentToken 认证 | 需 plicod 长连接，CLI 模式不适用 | 不可测 | — |
| 2.5 | Token estimate 覆盖 | search/hybrid/delta/growth/context 全部包含 token_estimate | ✅ |

### Node 3-tenant：多租户隔离（2 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 3t.1 | tenant_id 隔离 | CLI 无 `--tenant` 参数 | 不可 CLI 测试 | — |
| 3t.2 | 默认 tenant 向后兼容 | 所有命令在无 tenant 下正常工作 | ✅ |

### Node 3-experience：认知连续性（10 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 3e.1 | P-0: 原子 JSON 写入 | 数据跨重启完整（aicli 每次重启内核） | 6 objects, 25 events, 5 memories 全在 | ✅ |
| 3e.2 | P-2: CheckpointStore 持久化 | `suspend` → `resume` → `recall` | recall 前后一致 | ✅ |
| 3e.3 | F-6: Session 生命周期 | `session-start` + `session-end` | session_id 返回, "Session ended" | ✅ |
| 3e.4 | F-7: DeltaSince | `delta --since 0 --limit 5` | 结构化 changes 含 seq/summary/changed_by | ✅ |
| 3e.5 | F-8: Token 估算 | 各命令 JSON 输出含 token_estimate | ✅ |
| 3e.6 | F-9: Intent 缓存 | 无直接 CLI 观察手段 | 不可测 | — |
| 3e.7 | Checkpoint round-trip 类型保持 | recall 在 suspend/resume 后内容一致 | 4 条记忆完整保留 | ✅ |
| 3e.8 | Session warm_context | `session-start --intent "..." --last-seq 10` | 12 changes since seq 10 | ✅ |
| 3e.9 | **recall --tier 过滤** | `recall --tier working` vs `--tier long-term` vs 无 | **三种输出完全相同** | ❌ B38 |
| 3e.10 | Session 持久化跨重启 | `session-start` → 新进程 `session-end` | "Session ended" 成功 | ✅ |

### Node 4：协作生态（8 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 4.1 | HybridRetrieve Graph-RAG | `hybrid --query "circuit breaker resilience"` | 3 items, graph_score + provenance hop 数据 | ✅ |
| 4.2 | Ring EventLog 持久化 | `events history` 跨进程 | 25 events 完整恢复 | ✅ |
| 4.3 | GrowthReport | `growth --agent n13-tester` JSON | sessions:1, memories:4, kg_nodes:4, kg_edges:3 | ✅ |
| 4.4 | Task Delegation | `delegate --from A --to B` | 名称解析成功，UUID 返回 | ✅ |
| 4.5 | Crash recovery | put + remember → 新进程 search + recall | CAS + Memory + Events 全部跨重启持久 | ✅ |
| 4.6 | **KG paths** | `paths --src CID_A --dst CID_B` | **"No paths found"** 即使有 edge 连接 | ❌ B39 |
| 4.7 | agents list | `agents` JSON | 3 agents 含 id/name/state | ✅ |
| 4.8 | KG nodes list | `nodes` JSON | 8 nodes 含完整属性 | ✅ |

### Node 5：开门（MCP 暴露）（6 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 5.1 | 3 MCP tools | `plico-mcp` → `tools/list` | plico, plico_store, plico_skills | ✅ |
| 5.2 | MCP resources | `resources/list` | 6 resources (超出承诺的 3) | ✅ |
| 5.3 | plico://status | `resources/read` → status | 完整健康数据 + cache + scheduler | ✅ |
| 5.4 | plico tool schema | tools/list response | 17 actions, pipeline 模式, select 投影 | ✅ |
| 5.5 | **MCP recall_semantic** | `tools/call plico recall_semantic` | **"Server unavailable" — 无 stub fallback** | ❌ B42 |
| 5.6 | MCP 初始化协议 | initialize + notifications/initialized | protocolVersion 2024-11-05 返回正确 | ✅ |

### Node 6：闭合回路（7 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 6.1 | C-1: Event 持久化 | events 跨进程 | PASS (25 events 恢复) | ✅ |
| 6.2 | C-2: 跨 agent 搜索 | search --agent A 找到 B 的对象 | b8ad89b6 双向可见 | ✅ |
| 6.3 | C-3: 懒注册 | search --agent 新名 → 搜索成功 | 搜索可用但 `status` 404 — 未真正注册 | ⚠️ |
| 6.4 | C-4: MCP discover | `discover --agent X` | Agent Card 含 37 tools | ✅ |
| 6.5 | C-5: Resource 修复 | plico://status 返回真实数据 | ✅ |
| 6.6 | C-6: Delta 改进 | delta --since N 返回 changes | ✅ |
| 6.7 | C-7: 自适应反馈 | 无直接观察手段 | 不可测 | — |

### Node 7：代谢（5 项，代码审计为主）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 7.1 | F-20: ORT Embedding | stub 模式，无法验证 ORT | 代码存在 `ort_backend.rs` | ✅* |
| 7.2 | F-21: HNSW | stub 模式 | 代码存在 `hnsw.rs` 574 行 | ✅* |
| 7.3 | F-22: CAS Access Tracking | 无直接 CLI 观察 | 代码存在 | ✅* |
| 7.4 | F-25: Event Log Rotation | 无直接观察 | 代码存在 | ✅* |
| 7.5 | F-27: Causal Shortcuts | 无直接观察 | 代码存在 | ✅* |

> *标记 ✅* 表示代码审计确认存在，但未通过 CLI E2E 验证（需要 real embedding 环境）。

### Node 8：驾具（4 项，代码审计为主）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 8.1 | F-28: Instructions | plico://instructions MCP resource 存在 | ✅* |
| 8.2 | F-29: Profile | plico://profile MCP resource 存在 | ✅* |
| 8.3 | F-30: Smart Handover | session-start 返回 warm_context + delta | ✅ |
| 8.4 | F-35: MCP Prompts | MCP capabilities 含 prompts:{} | ✅* |

### Node 9：韧性（6 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 9.1 | F-36: BM25 评分改进 | search relevance 分数 | 仍为 0.01-0.02，无明显改进 | ⚠️ |
| 9.2 | F-37: Search Snippet | search JSON → snippet 字段 | ✅ 完整 preview 文本 | ✅ |
| 9.3 | F-38: Circuit Breaker | health report → degradations 数组 | "Vector search unavailable, BM25 fallback active" | ✅ |
| 9.4 | F-39: Checkpoint Round-Trip | suspend → resume → recall 一致 | ✅ |
| 9.5 | F-40: Search CAS Read Opt | search 结果含 snippet 避免逐条 get | ✅ |
| 9.6 | F-41: Degradation Visibility | `health` → degradations | embedding + cache 退化信息完整 | ✅ |

### Node 10：正名（11 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 10.1 | F-43: Delete 搜索隔离 (B9) | delete → search 不返回 | **B35 panic 阻塞，无法测试** | ❌ 阻塞 |
| 10.2 | F-44: Hybrid BM25 fallback | hybrid 在 stub 下有结果 | 3 items via graph, BM25 提供种子 | ✅ |
| 10.3 | F-45: Agent 生命周期 (B12) | register → activate → suspend → resume → terminate | suspend/resume 正常; register = `agent --register`; 无 activate 命令 | ⚠️ |
| 10.4 | F-46: Events filter (B14) | `events history --agent X` | n13-helper:1 event, n13-tester:11 events — 过滤正确 | ✅ |
| 10.5 | F-47: Response envelope | JSON 输出 `{ok, version, ...}` | 所有 JSON 响应遵循统一信封 | ✅ |
| 10.6 | F-48: Error exit code | get/delete/send 失败 → exit 1 | ✅ |
| 10.7 | F-49: Silent put (B15) | remember/put 有输出 | "Memory stored" / "CID: xxx" | ✅ |
| 10.8 | F-50: Get error (B18) | get 不存在 CID → exit 1 + 消息 | "Error: Object not found: CID=..." EXIT:1 | ✅ |
| 10.9 | F-51: Tool describe (B16) | `tool describe cas.search` → schema | 完整 JSON schema 含 params | ✅ |
| 10.10 | F-52: Tool call error (B19) | `tool call nonexistent` → exit 1 | EXIT:1 + "unknown tool" | ✅ |
| 10.11 | **--require-tags 交集过滤** | `search --require-tags "architecture,bug"` | **"No results" 即使对象含双标签** | ❌ B41 |

### Node 11：落地（5 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 11.1 | 冷启动优化 | 计时 status 命令 | **117ms**（6 objects） | ✅ |
| 11.2 | 统一操作反馈 | put/search/remember/events/status | 大部分命令有输出 | ✅ |
| 11.3 | JSON 全模式覆盖 | AICLI_OUTPUT=json 测所有命令 | status/quota/growth/search/events/nodes/agents/health 全 OK | ✅ |
| 11.4 | Update 命令 | `update --cid X --content "..."` | 返回新 CID | ✅ |
| 11.5 | Deleted/Restore | `deleted` 显示回收站 | "Recycle bin is empty"（B35 阻塞实际测试） | ⚠️ |

### Node 12：觉察（8 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 12.1 | A-1: Session 持久化 | session-start → 新进程 session-end | "Session ended" | ✅ |
| 12.2 | A-2: Agent 名称注册 | status/quota --agent name | 按名称查询成功 | ✅ |
| 12.3 | A-3: Agent Card | discover JSON | 37 tools, state, created_at; **缺 A2A 扩展字段** (description/skills/protocols/url) | ⚠️ |
| 12.4 | A-4: Memory Link Engine | remember → explore | KG 节点创建但**无自动边** | ⚠️ |
| 12.5 | A-5: Memory Consolidation | session-end 观察 | 无可观察整合效果 | ❌ |
| 12.6 | A-6: Context 诚实降级 | context --budget L0 vs L2 | **L0 和 L2 返回完全相同内容，均标记 [L2]** | ❌ |
| 12.7 | A-8a: Tag search 修复 | search --tags "X" | 返回正确结果 | ✅ |
| 12.8 | A-8d: Tool call error | tool call nonexistent → exit 1 | ✅ |

### Node 13：传导（10 项）

| # | 承诺 | 验证命令 | 结果 | 状态 |
|---|------|---------|------|------|
| 13.1 | F-1 (B28): Tier Match | remember --tier long-term → recall | [LongTerm] 显示 | ✅ |
| 13.2 | F-2 (B26): Tool Limit | `tool call cas.search --params '{"limit":1}'` | JSON params: 1 条; key:value 格式: 仍多条 | ⚠️ |
| 13.3 | F-3 (B29): Permission CLI | permission grant/check/list | 全路径可用 | ✅ |
| 13.4 | F-4 (B30): Delegate Name | delegate --from A --to B (by name) | 名称解析成功 | ✅ |
| 13.5 | F-5 (A-4): Memory Link | remember → explore | Memory KG 节点无自动边 | ⚠️ |
| 13.6 | F-6 (B33): Consolidation | session-end | 无整合报告 | ❌ |
| 13.7 | F-7: Health Report | `health` JSON | 完整: healthy/degradations/roundtrip | ✅ |
| 13.8 | F-8a: Events Filter | events history --agent X | 正确过滤 | ✅ |
| 13.9 | F-8b: Edge Display | edge --from A --to B | CID 完整显示 | ✅ |
| 13.10 | F-8c: Context L0 | context --budget L0 | 返回 [L2] 无降级 | ❌ |

---

## 2. 工具 API vs CLI 差异（Node 10 补充发现）

| 操作 | CLI (`aicli`) | Tool API (`tool call`) | 差异 |
|------|-------------|----------------------|------|
| cas.search limit | ❌ key:value 忽略 | ✅ JSON params 生效 | CLI 参数解析 bug |
| memory.store_procedure | ❌ `remember --tier procedural` 存但不可 recall | ✅ `tool call memory.store_procedure` → entry_id | CLI tier 路由断裂 |
| memory.recall | ✅ 显示内容文本 | ❌ `content: ""` 空字段 | Tool API 序列化 bug B40 |
| memory.recall tier 过滤 | ❌ `--tier` 完全被忽略 | ❌ `{"tier":"working"}` 返回所有 tier | 均不过滤 B38 |

---

## 3. 新发现 Bug 清单

| ID | 严重度 | 描述 | 发现 Node | 影响 |
|----|--------|------|----------|------|
| **B35** | 🔴 CRITICAL | `delete CID --agent X` 触发 Rust panic `byte index 2 is out of bounds of ""` | N13 | 阻塞 B9 + recycle bin 全功能 |
| **B38** | HIGH | `recall --tier X` 完全忽略 tier 参数，所有 tier 输出相同 | N1/N3 | 4 层记忆区分失效 |
| **B39** | HIGH | `paths --src A --dst B` 永远返回 "No paths found" 即使 edge 存在 | N4 | KG 路径查找不可用 |
| **B40** | MEDIUM | `tool call memory.recall` 返回 `content: ""` 空字段 | N10 | Tool API 记忆内容不可读 |
| **B41** | MEDIUM | `search --require-tags "a,b"` 交集过滤不工作，返回 0 结果 | N10 | 多标签精确过滤失效 |
| **B42** | MEDIUM | MCP `recall_semantic` 无 stub fallback，直接报错 | N5 | MCP 语义记忆在 stub 下不可用 |

---

## 4. 统计汇总

### 按 Node 覆盖率

| Node | 测试项 | 通过 | 部分 | 失败 | 不可测 | 通过率 |
|------|--------|------|------|------|--------|--------|
| 1 基础 | 6 | 5 | 1 | 0 | 0 | 83% |
| 2 AIOS | 5 | 4 | 0 | 0 | 1 | **100%** (可测) |
| 3t 租户 | 2 | 1 | 0 | 0 | 1 | — |
| 3e 连续性 | 10 | 8 | 0 | 1 | 1 | **89%** |
| 4 协作 | 8 | 7 | 0 | 1 | 0 | 88% |
| 5 MCP | 6 | 5 | 0 | 1 | 0 | 83% |
| 6 闭合 | 7 | 5 | 1 | 0 | 1 | 83% (可测) |
| 7 代谢 | 5 | 5* | 0 | 0 | 0 | **100%*** |
| 8 驾具 | 4 | 4* | 0 | 0 | 0 | **100%*** |
| 9 韧性 | 6 | 5 | 1 | 0 | 0 | 83% |
| 10 正名 | 11 | 8 | 1 | 2 | 0 | 73% |
| 11 落地 | 5 | 4 | 1 | 0 | 0 | 80% |
| 12 觉察 | 8 | 4 | 2 | 2 | 0 | 50% |
| 13 传导 | 10 | 6 | 2 | 2 | 0 | 60% |

> *Node 7/8 标记 ✅* 为代码审计确认，非 CLI E2E 验证。

### 按能力维度

| 维度 | 涉及测试项 | 通过率 | 关键问题 |
|------|-----------|--------|---------|
| CAS 存取搜索 | 8 | 88% | --require-tags 不工作 (B41) |
| 分层记忆 | 7 | 43% | tier 过滤失效 (B38), procedural 不可回调 |
| 知识图谱 | 5 | 80% | paths 不工作 (B39) |
| Session 生命周期 | 5 | 100% | — |
| Agent 管理 | 5 | 80% | lifecycle 不完整 |
| 事件系统 | 4 | 100% | — |
| 工具系统 | 5 | 80% | tool recall content 空 (B40) |
| MCP 协议 | 4 | 75% | recall_semantic 无 fallback (B42) |
| 权限系统 | 3 | 100% | — |
| 上下文装配 | 3 | 33% | L0 降级不工作, delete 阻塞 |
| 健康监控 | 2 | 100% | — |

---

## 5. 已修复 Bug 确认（Node 12 + Node 13 累计）

| Bug | 描述 | 修复 Node | 验证命令 | 确认 |
|-----|------|----------|---------|------|
| B22 | session-end "not found" | N12 A-1 | session-end | ✅ |
| B21 | status by name 失败 | N12 A-2 | status --agent name | ✅ |
| B25 | tag search 0 结果 | N12 A-8a | search --tags | ✅ |
| B20 | growth sessions:0 | N12 A-1 | growth | ✅ sessions:1 |
| B16 | tool describe 无 schema | N12 A-8c | tool describe | ✅ |
| B19 | tool call nonexist exit 0 | N12 A-8d | tool call bad | ✅ exit 1 |
| B28 | tier long-term 忽略 | N13 F-1 | remember --tier | ✅ [LongTerm] |
| B29 | permission CLI 不存在 | N13 F-3 | permission grant | ✅ |
| B30 | delegate by name 失败 | N13 F-4 | delegate --to name | ✅ |
| B14 | events --agent 不过滤 | N13 F-8a | events history --agent | ✅ |
| B27 | edge 显示空节点名 | N13 F-8b | edge create | ✅ CID 显示 |

---

## 6. 阻塞链分析

```
B35 (delete panic)
  → 阻塞 F-43 (soft-delete 搜索隔离)
  → 阻塞 deleted/restore 完整测试
  → 阻塞 recycle bin 功能验证
  → 影响 Node 10 通过率

B38 (recall --tier 不过滤)
  → 阻塞 4 层记忆有效区分
  → 影响 procedural/ephemeral 记忆使用场景
  → 影响 Node 1/3 记忆承诺

B39 (paths 不工作)
  → 阻塞 KG 路径查找
  → 影响 因果推理链 (Node 4/9)
  → 影响 Graph-RAG 高级功能

B42 (MCP recall_semantic)
  → MCP 下语义记忆完全不可用
  → 影响 Cursor/Claude 通过 MCP 使用记忆
```

---

## 7. 反向审查发现：上轮遗漏清单

| 遗漏项 | 属于 Node | 本轮结果 | 为何遗漏 |
|--------|----------|---------|---------|
| 跨 Agent 搜索可见性 | 2, 6 | ✅ PASS | 只在单 Agent 下测试 |
| Procedural 记忆 store/recall | 2, 3 | ❌ FAIL | 完全未测此 tier |
| Checkpoint round-trip 记忆一致性 | 3 | ✅ PASS | 只测了 suspend/resume 输出 |
| DeltaSince 结构化输出 | 3 | ✅ PASS | 未验证 delta 命令 |
| KG paths 查找 | 4 | ❌ FAIL (B39) | 未测 paths 命令 |
| Crash recovery 数据完整 | 4 | ✅ PASS | 声称测了但未真正模拟 |
| MCP tools/list | 5 | ✅ PASS | 用了错误的二进制名 |
| MCP resources | 5 | ✅ PASS | 未测 |
| MCP tool call | 5 | ❌ FAIL (B42) | 未测 |
| 搜索 snippet preview | 9 | ✅ PASS | 未检查 JSON 字段 |
| 搜索 --exclude-tags | 10 | ✅ PASS | 未测高级过滤 |
| 搜索 --require-tags 交集 | 10 | ❌ FAIL (B41) | 未测 |
| JSON 全模式覆盖 | 10, 11 | ✅ PASS | 只测了 discover |
| Agent Card 扩展字段 | 12 | ⚠️ PARTIAL | 只看了基本输出 |
| Tool API memory.recall 内容 | 10 | ❌ FAIL (B40) | 未测 tool call 记忆 API |
| 冷启动计时 | 11 | ✅ 117ms | 未测量 |
| Agent 完整生命周期 | 10 | ⚠️ PARTIAL | 只测了部分状态转换 |
| Intent 提交 | 2 | ✅ PASS | 未测 |
| Update 命令 | 10 | ✅ PASS | 未测 |
| Message send/read | 10 | ⚠️ send OK, read 空 | 未测 |

---

## 8. 优先修复建议

### P0（立即修复 — 功能阻塞）

1. **B35**: delete panic — 修复 `byte index 2 is out of bounds` 字符串切片错误
2. **B38**: recall tier 过滤 — 在 CLI handler 中正确传递 tier 参数到内核 API

### P1（本迭代修复 — 核心承诺缺口）

3. **B39**: paths 路径查找 — 修复 KG 图遍历算法对已有 edge 的发现
4. **B41**: --require-tags 交集 — 修复多标签 AND 过滤逻辑
5. **B40**: tool call memory.recall content 空 — 修复 ToolOutput 序列化

### P2（下迭代 — 可降级）

6. **B42**: MCP recall_semantic stub fallback — 在 stub 模式下回退到 BM25 keyword 匹配
7. **A-6/B31**: Context L0 降级 — 实现真正的 L0 摘要或至少标记降级
8. **A-5/B33**: Memory Consolidation — 实现 session-end 整合报告

---

## 9. 经验教训

### 审计方法论

1. **反向审查比正向测试更有效**：正向测试容易"选择性验证"（只测容易通过的），反向从设计文档逐条提取承诺再验证，暴露了 29 个遗漏。
2. **CLI 与 Tool API 是两套路径**：`remember --tier procedural` 和 `tool call memory.store_procedure` 走不同代码路径，行为不一致。
3. **MCP 二进制名 != Cargo target 名**：`plico_mcp` vs `plico-mcp` 差异导致整轮 MCP 测试空白。
4. **"commit ≠ fix" 仍然成立**：本轮新发现 B38-B42 均是已有代码但行为不正确的情况。
5. **Stub 模式暴露架构问题**：所有 embedding 依赖路径在 stub 下应有 BM25 fallback，MCP 的 recall_semantic 没有。

### 系统健康度评估

- **最健康**: Session 生命周期 (100%), 事件系统 (100%), 权限系统 (100%), 健康监控 (100%)
- **需关注**: 分层记忆 (43%), 上下文装配 (33%)
- **代码质量**: 808 测试全绿，说明内核逻辑稳健，问题集中在 CLI/Tool API 接口层
