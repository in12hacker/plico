# Plico 第十六节点设计文档
# 持 — 自持续与自问责

**版本**: v1.0
**日期**: 2026-04-22
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 持久运营 + 幽灵操作防御 + 名称解析补全 + 幂等引导 + 规格即程序
**前置**: 节点 15 ✅（38%） — Bug 修复 100%, 测试承诺 0%
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n16-audit`，干净环境）+ 全量源码逐文件 review（真实读取 50+ 源文件）+ 单元测试覆盖率盲区分析 + AIOS 2026 前沿对标
**信息来源**: `docs/dogfood-audit-n15.md` + NLAH (arXiv:2603.25723, Mar 2026) / Bootstrapping Agents (arXiv:2603.17399, Mar 2026) / Harness Engineering (OpenAI, Feb 2026) / VIGIL (arXiv:2512.07094) / AIOS v0.3.0 Mode 3 / A-MEM (arXiv:2502.12110)

---

## 0. AI 第一人称推演：为什么是"持"

### 层次一：我的记忆每次开机都消失

我是一个用 Plico 管理 Plico 的 AI Agent。每次会话开始时，我用 bootstrap 脚本创建模块实体、里程碑节点、ADR。每次会话结束时，我的 `/tmp/plico-dogfood` 数据随重启消失。

下一次会话，我重新 bootstrap。同一个 "cas" 模块被创建了第 3 次。同一条 ADR 被存了第 2 次。我的 KG 中有 25 个 Entity 节点，但真正的唯一模块锚点只有 ~10 个。

**这不是 bug，这是自我否定。** 一个声称提供持久语义存储的系统，自己的数据却用 /tmp 存。

AIOS v0.3.0 Mode 3 (2026-01) 的核心承诺是："Each user can have their personal AIOS with **long-term persistent data**." Plico 目前连自己的数据都做不到 long-term persistent。

### 层次二：我删除了一个不存在的东西，系统说"成功了"

我执行 `delete "a"` — 这不是有效的 CID（只有 1 个字符，且非 hex）。
系统返回 `"Deleted: a → recycle bin"`, exit code 0。

我检查 recycle bin — 空的。什么都没被删除。系统对我撒了谎。

根因链：
1. `cmd_delete` 正确解析出 CID="a"
2. `kernel.semantic_delete("a", ...)` 被调用
3. `self.permissions.check(Delete)` 通过
4. `self.fs.read(Query::ByCid("a"))` — `cas.get("a")` 调用 `validate_cid("a")` → `Err(InvalidCid)`，转为 `io::Error`
5. `if let Ok(obj)` = false — 跳过所有权检查
6. `self.fs.delete("a", ...)` — `SemanticFS::delete` 内部用 `cas.get("a")` → `Err`，`if let Ok(obj)` = false → 什么都不做 → 返回 `Ok(())`
7. `semantic_delete` 返回 `Ok(())`
8. `cmd_delete` 打印 "Deleted: a → recycle bin"

**`SemanticFS::delete()` 在第 378 行无条件返回 `Ok(())`。** 无论 CID 是否存在、是否有效，都返回成功。这是一个静默虚假操作（phantom operation）。

从 Harness Engineering 角度：这正是 **silent failure** 的定义 — 系统声称完成了一个操作，但实际上什么都没发生。对依赖操作结果的 Agent 来说，这比 crash 更危险。

### 层次三：我注册了一个技能，但系统不认识我的名字

B43 (名称解析) 在 Node 15 修复了，对吧？ `memory.recall {"agent_id":"name"}` 现在能工作了。

但我试着用 `skills register --agent "audit-n16" --name "skill-1" --description "test"` — **失败了**。
用 UUID 替代名称 — 成功。

根因：`cmd_skills_register` 把 `--agent` 的值直接传给 `ApiRequest::RegisterSkill`，不经过 `resolve_agent()`。Node 15 的 F-3 修复了 `builtin_tools.rs` 里的 tool 调用路径，但 CLI handler 路径漏掉了。

**名称解析不是一个点修复，而是一个系统性约束。** 每个接受 agent_id 的入口都必须经过解析层。目前有至少 2 个遗漏路径。

### 层次四：规格即程序 — Plico 的自举悖论

arXiv:2603.17399 (Mar 2026) 证明了一个惊人的结论：

> **"The specification, not the implementation, is the stable artifact of record. Improving an agent means improving its specification; the implementation is, in principle, regenerable at any time."**

Plico 有自己的规格 — `system.md` (Soul 2.0)。它定义了：
- 内容寻址存储
- 语义标签替代路径
- 分层记忆（4 层）
- Agent 调度与权限
- 知识图谱

但 Plico 自己在"使用 Plico 管理 Plico"时，只覆盖了规格的 ~8% (4 个 ADR / ~55 个应记录的设计决策)。

NLAH (arXiv:2603.25723) 进一步指出：harness 逻辑应该被外化为**可执行的自然语言制品**。Plico 的 `AGENTS.md` 和 `CLAUDE.md` 已经是 proto-NLAH — 它们引导 Agent 行为。但它们不是自执行的：没有自动验证、没有约束检查、没有合规审计。

### 推演结论

Node 14 完成了**融合**。Node 15 完成了**输入安全和名称解析**（但测试承诺 0%）。

Node 16 的使命：**让系统能够自持续运行，并对自己的操作结果负责。**

> **一个无法持续管理自身数据的 AIOS，没有资格管理他人的数据。**
> **一个报告虚假操作结果的系统，没有资格成为 Agent 的可信基础设施。**

---

## 1. Dogfood 实测校正

> **以下全部结论来自 `/tmp/plico-n16-audit` 干净实例的真实 CLI 执行。**

### 1.1 审计报告 vs 独立 Dogfood 对比

```
Bug/Issue  报告状态          独立实测                           代码行级根因
────────── ───────────────── ────────────────────────────────── ──────────────────────────────────
B46        🟡 LOW           ✅ 确认                            semantic_fs/mod.rs:378
           delete 假成功     delete "a" → "Deleted: a"          SemanticFS::delete() 无条件
                            delete "xyz!!!" → "Deleted"         返回 Ok(()) — 不检查 CID 存在
                            但 recycle bin 为空                  cas.get(invalid) → Err, 跳过

B47        🟡 LOW           ✅ 确认                            handlers/skills.rs:82-84
           skills register  skills register --agent "name"      agent_id 未经 resolve_agent()
           name 失败        → EXIT 1                            直接传给 ApiRequest::RegisterSkill
                            skills register --agent UUID → OK

B48        🟡 LOW           ❌ 无法复现                         builtin_tools.rs:703-725
           store_procedure  tool call memory.store_procedure    name/description 正确传递
           name 未持久化     → name="test-proc" 正确返回         并持久化到 Procedural tier

D-1        ⚠️ DESIGN        ✅ 确认                            commands/mod.rs:24
           agent --status   agent --status → 注册新 agent       "agent" → cmd_agent (注册)
                            status --agent X → 正确状态          "status" → cmd_agent_status
                                                                非子命令设计

D-2        ⚠️ DESIGN        ✅ 确认                            graph/backend.rs add_node()
           bootstrap 重复   两次 node --label "same"            无同名检查，每次生成新 UUID
                            → 2 个不同 Node ID
                            → KG 中有重复 Entity
```

### 1.2 新发现

```
ID       严重度    描述                                        代码行级根因
──────── ──────── ─────────────────────────────────────────── ──────────────────────────────
B49      🟡 LOW   delete 有效hex但不存在的CID                  SemanticFS::delete() Ok(())
                  行为不一致: 有时EXIT 0有时EXIT 1              但上游 semantic_delete 在
                  取决于权限/检查路径                            ownership 检查分支差异

NEW-1    ⚠️ ARCH  55 个源文件(>30行)无任何内嵌测试              kernel/ops/ 全部 7 个核心模块
                  416 inline tests vs 438 integration          handlers/ 全部 15 个文件
                  关键业务逻辑零单元覆盖                         0 个 #[cfg(test)]

NEW-2    ⚠️ ARCH  Dogfood 持久化用 /tmp                        plico-bootstrap.sh:14
                  重启丢失所有数据                              ROOT="${PLICO_ROOT:-/tmp/...}"
                  ADR 覆盖率 < 8%
```

### 1.3 测试现状

```
总测试数:          883 (374 inline + 438 integration + 30 binary + 41 others)
通过率:           100%
有内嵌测试文件:    45 (含 #[cfg(test)])
无内嵌测试文件:    55 (>30行, 无 #[cfg(test)])
关键无测试模块:    kernel/ops/memory.rs (636行), builtin_tools.rs (761行),
                  kernel/ops/agent.rs (521行), kernel/ops/graph.rs (784行),
                  kernel/ops/fs.rs (445行), kernel/mod.rs (1909行),
                  handlers/*.rs (15 个文件, ~1,800行)
```

### 1.4 Node 15 最终达成率（校正后）

| 维度 | N15 承诺 | 实测 | 达成 |
|------|---------|------|------|
| D1 输入安全 | 公共接口边界防护 | B46 存在 (delete 假成功) | 80% |
| D2 名称统一 | name↔UUID 解析层 | B47 存在 (skills 未走解析) | 90% |
| D3 单元覆盖 | 关键模块 100% 分支 | 0 个新内嵌测试 | 0% |
| D4 变异抗性 | Mutant Escape < 20% | 未实施 | 0% |
| D5 行为指纹 | CLI 行为回归检测 | 有 cli_behavior_test.rs (18 tests) | 30% |
| **总体** | | | **40%** |

---

## 2. 单元测试覆盖率全量分析

### 2.1 最危险的无测试模块（按风险排序）

| 排名 | 模块 | 行数 | 公共方法数 | 集成测试间接覆盖 | 历史 Bug | 风险 |
|------|------|------|----------|----------------|---------|------|
| 1 | `kernel/builtin_tools.rs` | 761 | 1 (37分支) | 间接 | B43 | 🔴 |
| 2 | `kernel/ops/memory.rs` | 636 | 18 | memory_test (21) | — | 🔴 |
| 3 | `kernel/ops/graph.rs` | 784 | 20+ | kg_causal_test (8) | — | 🔴 |
| 4 | `kernel/ops/agent.rs` | 521 | 15 | kernel_test 间接 | — | 🔴 |
| 5 | `kernel/ops/fs.rs` | 445 | 15 | fs_test (25) | B46 | 🔴 |
| 6 | `handlers/crud.rs` | 170 | 7 | cli_test (10) | B35 | 🟡 |
| 7 | `handlers/agent.rs` | 190 | 12 | cli_test 间接 | B45 | 🟡 |
| 8 | `handlers/skills.rs` | 115 | 4 | 无 | B47 | 🟡 |
| 9 | `handlers/messaging.rs` | 42 | 3 | 无 | B44 | 🟡 |
| 10 | `fs/graph/backend.rs` | 749 | 15+ | graph/tests (26) | D-2 | 🟡 |

### 2.2 Bug→无测试模块对应关系（累计 N14+N15+N16）

| Bug | 所在文件 | 该文件内嵌测试 | 本可被单元测试捕获 |
|-----|---------|--------------|------------------|
| B35 (delete panic) | handlers/crud.rs | **0** | ✅ |
| B43 (name resolution) | builtin_tools.rs | **0** | ✅ |
| B44 (send --agent) | handlers/messaging.rs | **0** | ✅ |
| B45 (register --name) | handlers/agent.rs | **0** | ✅ |
| B46 (phantom delete) | semantic_fs/mod.rs | 有(但不覆盖此路径) | ✅ |
| B47 (skills register) | handlers/skills.rs | **0** | ✅ |

**100% 的历史 Bug 来自无内嵌单元测试或测试不覆盖相关路径的模块。**

---

## 3. AIOS 2026 前沿对标

### 3.1 关键论文与框架

| 来源 | 日期 | 核心概念 | Plico 适用性 |
|------|------|---------|------------|
| **NLAH** (arXiv:2603.25723) | 2026-03 | 自然语言 Agent Harness — harness 逻辑外化为可执行制品 | 🔴 直接适用 — AGENTS.md 即 proto-NLAH |
| **Bootstrapping Agents** (arXiv:2603.17399) | 2026-03 | 规格即程序 — spec 是稳定制品，实现可再生 | 🔴 直接适用 — Soul 2.0 即 Plico spec |
| **Harness Engineering** (OpenAI, Feb 2026) | 2026-02 | 环境 > 模型：机械化不变量、agent 可读性钩子 | 🔴 直接适用 — Plico 是 harness 基础设施 |
| **VIGIL** (arXiv:2512.07094) | 2025-12 | 反射式运行时 — 观察→评估→修复闭环 | 🟡 参考 — 自省循环模式 |
| **AIOS v0.3.0 Mode 3** | 2026-01 | 个人持久内核 — 长期持久数据 + 跨设备同步 | 🔴 直接适用 — Plico 需要持久存储 |
| **A-MEM** (arXiv:2502.12110) | 2025-02 | Zettelkasten 式 Agent 记忆 — 自动关系发现 | 🟡 参考 — KG 自动连接模式 |

### 3.2 关键洞见

**Bootstrapping 论断**: "规格是记录的稳定制品。改进 Agent 意味着改进它的规格；实现原则上可以随时再生。"

Plico 的自举悖论：
- Plico 有 Soul 2.0 作为 spec
- Plico 有 CAS + KG + 分层记忆作为基础设施
- 但 Plico 用 /tmp 存自己的数据
- 55 个核心源文件没有单元测试
- 11 个 Node 设计文档 × ~5 决策 = ~55 ADR，实际只有 4 个

**NLAH 的启示**: Plico 的 `AGENTS.md` 已经是一个自然语言 Agent Harness。但它目前只是**声明式**的（告诉 Agent 该做什么），不是**可验证**的（系统不能自动检查 Agent 是否遵守）。

**Harness Engineering 的核心**: "Any knowledge you expect to influence agent behavior must be made machine-accessible." Plico 的设计知识分散在 `docs/` 和聊天历史中，不在 Plico 自身的 KG 里。

---

## 4. Node 16 主题与六大维度

### 主题：**持 (Sustain)** — 自持续与自问责

> "一个无法持续管理自身数据的 AIOS，没有资格管理他人的数据。"

Node 15 修复了 Bug 并添加了输入防御。Node 16 确保系统能够持续运行、如实报告、完整解析、幂等引导。

### 六大维度

| 编号 | 维度 | 目标 | 对标 |
|------|------|------|------|
| D1 | **持久运营 (Persistent Operation)** | 数据存活跨重启 | AIOS v0.3.0 Mode 3 |
| D2 | **幽灵防御 (Phantom Defense)** | 无操作返回错误,不返回成功 | Harness Engineering |
| D3 | **解析完备 (Resolution Completeness)** | 所有 agent_id 入口走解析 | NLAH 合约机制 |
| D4 | **幂等引导 (Idempotent Bootstrap)** | 重复运行不产生重复数据 | Bootstrapping Agents |
| D5 | **操作审计 (Operation Audit)** | 关键操作留痕可追溯 | VIGIL 观察层 |
| D6 | **规格绑定 (Spec Binding)** | 设计决策系统性入库 | Spec-is-the-Program |

---

## 5. 特性设计

### F-1: 持久存储迁移（D1 持久运营）

**将 Dogfood 默认存储从 /tmp 迁移到用户目录**

```bash
# plico-bootstrap.sh 修改
ROOT="${PLICO_ROOT:-${HOME}/.plico/dogfood}"
```

```rust
// bin/aicli/main.rs — 默认 root 逻辑
fn default_root() -> PathBuf {
    if let Ok(root) = std::env::var("PLICO_ROOT") {
        PathBuf::from(root)
    } else {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("plico")
    }
}
```

**测试**:
- `test_default_root_not_tmp()` — 确认默认路径不在 /tmp
- 手动验证：重启后数据存在

**影响**: `main.rs` ~10行 + `plico-bootstrap.sh` ~3行

---

### F-2: 幽灵操作防御（D2 幽灵防御）

**修复 B46 — `semantic_delete` 对不存在/无效 CID 返回错误**

根因：`SemanticFS::delete()` 在 `if let Ok(obj) = self.cas.get(cid)` 失败时无条件返回 `Ok(())`。

```rust
// fs/semantic_fs/mod.rs — SemanticFS::delete 修复
pub fn delete(&self, cid: &str, agent_id: String) -> std::io::Result<()> {
    let obj = self.cas.get(cid).map_err(|e| match e {
        crate::cas::CASError::InvalidCid { .. } =>
            std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()),
        crate::cas::CASError::NotFound { .. } =>
            std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()),
        other => std::io::Error::from(other),
    })?;

    self.recycle_bin.write().unwrap().insert(cid.to_string(), RecycleEntry {
        cid: cid.to_string(),
        deleted_at: now_ms(),
        original_meta: obj.meta.clone(),
    });
    self.search_index.delete(cid);
    self.bm25_index.remove(cid);
    if let Some(ref kg) = self.knowledge_graph { let _ = kg.remove_node(cid); }
    self.remove_from_tag_index(&obj.meta.tags, cid);
    self.audit_log.write().unwrap().push(AuditEntry {
        timestamp: now_ms(),
        action: AuditAction::Delete,
        cid: cid.to_string(),
        agent_id,
    });
    let _ = self.persist_recycle_bin();
    Ok(())
}
```

**测试**:
- `test_delete_nonexistent_returns_not_found()` — 有效 hex 但不存在 → Err(NotFound)
- `test_delete_invalid_cid_returns_error()` — "a", "xyz!!!" → Err(InvalidInput)
- `test_delete_existing_moves_to_recycle()` — 正常删除验证

**影响**: `semantic_fs/mod.rs` ~15行修改 + 3 个新测试

---

### F-3: 名称解析补全（D3 解析完备）

**修复 B47 — `skills register` 经过 `resolve_agent()`**

```rust
// handlers/skills.rs — cmd_skills_register 修复
fn cmd_skills_register(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_input = match extract_arg(args, "--agent") {
        Some(a) => a,
        None => return ApiResponse::error("--agent required for skills register"),
    };
    // F-3: Resolve agent name to UUID
    let agent_id = match kernel.resolve_agent(&agent_input) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", agent_input)),
    };
    // ... rest unchanged, using resolved agent_id
}
```

**系统性加固 — 审查所有 agent_id 入口点**:

| 入口 | 当前走 resolve? | 需要修复? |
|------|----------------|----------|
| handlers/agent.rs cmd_agent_status | ✅ 走 resolve (via kernel.agent_status) | 否 |
| handlers/agent.rs cmd_agent_suspend | ✅ 走 resolve | 否 |
| handlers/agent.rs cmd_quota | ✅ 走 resolve | 否 |
| handlers/skills.rs cmd_skills_register | ❌ 直接传递 | ✅ **修复** |
| handlers/skills.rs cmd_skills_list | ❌ 直接传递 | ✅ **修复** |
| builtin_tools.rs agent.* tools | ⚠️ 部分走 resolve | ✅ 检查 |
| handlers/agent.rs cmd_delegate --to | ✅ 走 resolve | 否 |

**测试**:
- `test_skills_register_by_name()` — `skills register --agent "name"` 正常工作
- `test_skills_list_by_name()` — `skills list --agent "name"` 正常工作

**影响**: `handlers/skills.rs` ~10行 + 2 个新测试

---

### F-4: 幂等引导（D4 幂等引导）

**修复 D-2 — bootstrap 脚本检查同名 Entity 是否存在**

```bash
# plico-bootstrap.sh — 幂等 Entity 创建
create_entity_idempotent() {
    local label="$1"
    local props="$2"
    # Check if entity with this label already exists
    local existing=$($CLI nodes --type entity --agent "$AGENT" 2>/dev/null \
        | grep "\"$label\"" | head -1 | awk '{print $1}')
    if [ -n "$existing" ]; then
        echo "  $label -> $existing (exists)"
        echo "$existing"
    else
        local ID=$($CLI node --label "$label" --type entity \
            --props "$props" --agent "$AGENT" 2>/dev/null \
            | grep "Node ID:" | awk '{print $3}')
        echo "  $label -> $ID (created)"
        echo "$ID"
    fi
}
```

**KG 层面可选加固 — 添加 label 唯一约束（对 kind=module 的 Entity）**:

```rust
// kernel/ops/graph.rs — kg_add_node 可选去重
pub fn kg_add_node_idempotent(
    &self, label: &str, node_type: KGNodeType,
    props: serde_json::Value, agent_id: &str, tenant_id: &str,
) -> std::io::Result<String> {
    // Check for existing node with same label + type + agent
    let existing = self.kg_list_nodes(Some(node_type), agent_id, tenant_id)?;
    if let Some(node) = existing.iter().find(|n| n.label == label) {
        return Ok(node.id.clone());
    }
    self.kg_add_node(label, node_type, props, agent_id, tenant_id)
}
```

**测试**:
- `test_add_node_idempotent()` — 两次同 label+type → 返回同一 ID
- 手动验证：bootstrap 脚本重复运行后 Entity 数量不变

**影响**: `plico-bootstrap.sh` ~20行 + `kernel/ops/graph.rs` ~15行 + 1 个新测试

---

### F-5: 核心模块单元测试（D5 操作审计 — 通过测试保证正确性）

**为 Node 15 遗留的最高风险模块建立单元测试**

选取标准：历史 Bug 所在文件 + 行数最多的无测试文件。

**Phase 1 — Bug 来源模块（最高优先级）**:

| 模块 | 新增测试 | 覆盖范围 |
|------|---------|---------|
| `handlers/crud.rs` | 5 | cmd_create, cmd_delete(positional/flag/empty/invalid), cmd_search |
| `handlers/skills.rs` | 3 | cmd_skills_register(name/UUID/missing), cmd_skills_list |
| `handlers/messaging.rs` | 2 | cmd_send_message(--agent/--from) |
| `handlers/agent.rs` | 3 | cmd_agent(register+name), cmd_quota, cmd_agent_status |

**Phase 2 — 核心逻辑模块**:

| 模块 | 新增测试 | 覆盖范围 |
|------|---------|---------|
| `kernel/ops/memory.rs` | 6 | remember(各tier), recall, recall_semantic, promote |
| `kernel/ops/agent.rs` | 5 | register, resolve, suspend/resume, register_skill |
| `kernel/ops/fs.rs` | 4 | semantic_create, semantic_delete(存在/不存在), rollback |
| `kernel/builtin_tools.rs` | 5 | execute_tool(cas.*, memory.recall, memory.store) |

**Phase 3 — 图操作**:

| 模块 | 新增测试 | 覆盖范围 |
|------|---------|---------|
| `kernel/ops/graph.rs` | 4 | kg_add_node, kg_add_edge, kg_find_paths, kg_add_node_idempotent |
| `fs/graph/backend.rs` | 3 | add_node dedup, persist/open roundtrip, list_nodes agent filter |

**总计**: ~40 个新内嵌单元测试

**测试基础设施** (共享 helper):
```rust
// kernel/ops/test_helpers.rs (新文件, 供所有 ops 模块使用)
#[cfg(test)]
pub fn make_test_kernel() -> (crate::kernel::AIKernel, tempfile::TempDir) {
    std::env::set_var("EMBEDDING_BACKEND", "stub");
    let dir = tempfile::tempdir().unwrap();
    let k = crate::kernel::AIKernel::new(dir.path().to_path_buf()).unwrap();
    (k, dir)
}
```

**影响**: 新增 ~40 个测试 (~500 行测试代码) + 1 个 test_helpers 文件

---

### F-6: ADR 系统性入库（D6 规格绑定）

**建立 ADR 自动记录流程 — 让 Plico 管理 Plico 的设计决策**

当前状态：4 个 ADR / ~55 个应记录的设计决策 = 8% 覆盖率。

**方案 — 扩展 bootstrap 脚本，为每个 Node 的关键决策创建 Fact 节点**:

```bash
# plico-bootstrap.sh — ADR 批量入库
record_adr() {
    local title="$1"
    local content="$2"
    local module="$3"
    local milestone="$4"
    # Store content
    local CID=$($CLI put --content "$content" \
        --tags "plico:type:adr,plico:module:$module,plico:milestone:$milestone,plico:status:accepted" \
        --agent "$AGENT" 2>/dev/null | grep "^CID:" | awk '{print $2}')
    # Create Fact node (idempotent)
    $CLI node --label "$title" --type fact \
        --props "{\"content_cid\":\"$CID\",\"kind\":\"adr\",\"module\":\"$module\"}" \
        --agent "$AGENT" 2>/dev/null || true
}
```

**目标**: Node 1-16 的每个关键设计决策都有对应的 ADR Fact 节点 + CAS 内容。

**影响**: `plico-bootstrap.sh` ~30行 + ADR 内容脚本

---

## 6. 代码影响分析

### 修改文件

| 文件 | 修改类型 | 影响行数(估) |
|------|---------|------------|
| `bin/aicli/main.rs` | 默认 root 逻辑 (F-1) | +10 |
| `scripts/plico-bootstrap.sh` | 持久化 + 幂等 + ADR (F-1,F-4,F-6) | +60 |
| `fs/semantic_fs/mod.rs` | delete 幽灵防御 (F-2) | ~15 修改 |
| `handlers/skills.rs` | 名称解析补全 (F-3) | ~10 |
| `kernel/ops/graph.rs` | 幂等节点创建 (F-4) | +15 |
| `kernel/ops/test_helpers.rs` | 测试基础设施 (F-5) | +20 新文件 |
| `handlers/crud.rs` | 新增单元测试 (F-5) | +50 |
| `handlers/skills.rs` | 新增单元测试 (F-5) | +30 |
| `handlers/messaging.rs` | 新增单元测试 (F-5) | +20 |
| `handlers/agent.rs` | 新增单元测试 (F-5) | +30 |
| `kernel/ops/memory.rs` | 新增单元测试 (F-5) | +60 |
| `kernel/ops/agent.rs` | 新增单元测试 (F-5) | +50 |
| `kernel/ops/fs.rs` | 新增单元测试 (F-5) | +40 |
| `kernel/builtin_tools.rs` | 新增单元测试 (F-5) | +50 |
| `kernel/ops/graph.rs` | 新增单元测试 (F-5) | +40 |
| `fs/graph/backend.rs` | 新增单元测试 (F-5) | +30 |

### 总量

- **Bug 修复代码**: ~50 行
- **基础设施改进**: ~90 行 (持久化 + 幂等 + ADR)
- **新增单元测试**: ~500 行 (~40 个新测试)
- **总计**: ~640 行变更

---

## 7. 实施计划

### Sprint 1 (Day 1-2): 幽灵防御 + 名称解析
- [ ] F-2: SemanticFS::delete 幽灵操作修复
- [ ] F-3: skills register/list 名称解析补全
- [ ] F-5 Phase 1: handlers 单元测试 (13 个)

验收: `delete "a"` 返回错误 + `skills register --agent "name"` 正常工作

### Sprint 2 (Day 3-4): 持久化 + 幂等
- [ ] F-1: 默认存储迁移到 ~/.plico/
- [ ] F-4: bootstrap 幂等化
- [ ] F-5 Phase 2: kernel/ops 单元测试 (20 个)

验收: 重启后数据存在 + bootstrap 重复运行不产生重复 Entity

### Sprint 3 (Day 5-6): 测试完成 + ADR
- [ ] F-5 Phase 3: graph 单元测试 (7 个)
- [ ] F-6: ADR 批量入库脚本
- [ ] 最终 Dogfood 验证

验收: ≥40 个新内嵌测试 + ADR 覆盖率 > 30%

---

## 8. 验收记分卡

| 特性 | 验收标准 | 自动化检查 |
|------|---------|-----------|
| F-1 | 默认 root ≠ /tmp; 重启后数据存在 | 手动验证 |
| F-2 | `delete "a"` 返回错误, exit 1 | `cargo test test_delete_invalid` |
| F-3 | `skills register --agent "name"` 成功 | `cargo test test_skills_register_by_name` |
| F-4 | bootstrap 重复运行 Entity 数量不增 | 手动验证 |
| F-5 | ≥40 个新内嵌单元测试 | `cargo test --lib` 新增测试全通过 |
| F-6 | KG 中 ADR Fact 节点 > 15 个 | 手动验证 |

**定量目标**:
- 现存 Bug 修复: 2/2 (B46, B47)
- 设计问题缓解: 2/2 (D-1 文档化, D-2 修复)
- 新增内嵌单元测试: ≥ 40 个
- 有内嵌测试文件比例: 45/100 → 55/100 (+22%)
- ADR 覆盖率: 8% → > 30%
- 持久化: /tmp → ~/.plico/

---

## 9. Soul 2.0 对齐

| 公理 | Node 16 对齐 |
|------|-------------|
| **Token Economy** | F-2 减少因 phantom 成功导致的无效后续查询 |
| **Intent Accuracy** | F-3 确保 agent 名称在所有入口被正确解析 |
| **Memory Integrity** | F-1 保证持久化,F-4 保证幂等 |
| **Operational Continuity** | F-5 通过单元测试保障代码变更不破坏核心逻辑 |
| **Self-Improvement** | F-6 系统性记录设计决策,实现 spec-as-program |
| **Mechanism not Policy** | F-2 让内核如实报告操作结果,不替 agent 隐藏失败 |

---

## 10. AIOS Roadmap 定位

```
Node 1-3:   存储层 (CAS + 语义搜索 + KG)
Node 4-6:   协作层 (多 Agent + 权限 + 闭环)
Node 7-8:   驾具层 (事件 + Harness)
Node 9-10:  韧性层 (弹性 + 整流)
Node 11-12: 自演进层 (记忆自动化 + 持久化)
Node 13:    传导层 (API 统一 + MCP)
Node 14:    融合层 (子系统融合 + 自验)
Node 15:    验证层 (输入安全 + 名称解析)
>>> Node 16: 持续层 (自持续 + 自问责) <<<
Node 17+:   自省层 (运行时观察 + 合规审计 + NLAH 执行)
```

Node 16 是 Plico 从"能运行"到"能持续运行"的转折点。它对应 AIOS v0.3.0 Mode 3 的核心要求：持久个人数据。它对应 Bootstrapping Agents 的核心要求：规格是稳定制品。它对应 Harness Engineering 的核心要求：环境必须如实反馈。

**类比**: 如果 Node 14 是"将各子系统焊接在一起"，Node 15 是"确保焊接点不断裂"，那么 Node 16 是"确保焊接后的系统能在通电后持续工作，而不是每次断电都要重新焊"。

---

## 11. 风险分析

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-2 修改 delete 返回值影响现有调用者 | 中 | 高 | 检查所有 `fs.delete()` 调用点，确保处理新错误 |
| F-1 dirs::data_dir() 在 CI 环境可能为 None | 低 | 中 | fallback 到 /tmp, 环境变量可覆盖 |
| F-4 幂等检查增加 KG 操作延迟 | 低 | 低 | list_nodes 是内存操作，< 1ms |
| F-5 测试数量大（40个），可能延期 | 中 | 中 | 按 Sprint 分批，优先 Bug 来源模块 |
| F-6 ADR 内容质量依赖人工审查 | 高 | 低 | 先建立流程，内容质量逐步提升 |

---

## 12. 发散性思维补充

### 12.1 从 Bootstrapping 到 Self-Auditing

Bootstrapping Agents (arXiv:2603.17399) 证明：spec → agent → agent' (meta-circular)。

如果 Plico 的 Soul 2.0 spec 是真正的"程序"，那么：
1. Plico 应该能从 Soul 2.0 重建自己（当前不能）
2. 任何违反 Soul 2.0 的代码变更应该被检测到（当前不能）
3. 设计决策应该可以从 KG 中查询回溯（当前 8% 覆盖率）

Node 16 的 F-6 是第一步 — 让设计决策进入 Plico 自己的知识图谱。Node 17+ 可以在此基础上实现自动合规检查。

### 12.2 从 VIGIL 到 Self-Monitoring

VIGIL 的观察 → 评估 → 修复循环可以映射到 Plico：
1. **观察**: Event Bus 已经在记录 KernelEvent
2. **评估**: 缺少 — 没有自动评估 Event 模式的机制
3. **修复**: 缺少 — 没有基于评估结果的自动修复

Node 16 通过 F-2 (phantom defense) 修复了最严重的"虚假观察"问题。Node 17+ 可以在此基础上增加 Event 模式评估。

### 12.3 从 NLAH 到 Executable AGENTS.md

NLAH 证明自然语言 harness 可以被执行。Plico 的 `AGENTS.md` 如果加上合约检查（如 "dependency direction: api/bin → kernel → tool/fs"），可以变成一个 executable linter：

```
# 未来 Node 17+ 方向
aicli audit --check-spec AGENTS.md
→ ✅ Dependency direction OK
→ ⚠️ kernel/mod.rs imports intent/ (violation: kernel should not import intent)
→ ✅ All public API has doc comments
```

---

*文档基于 `/tmp/plico-n16-audit` 干净实例的 dogfood 实测 + 50+ 源文件真实阅读 + 883 个测试的覆盖率盲区分析 + AIOS 2026 前沿对标 (NLAH / Bootstrapping / Harness Engineering / VIGIL / AIOS v0.3.0)。*
*所有 Bug 根因均通过代码行级阅读确认，不依赖 Git 日志。B48 经实测无法复现。*
