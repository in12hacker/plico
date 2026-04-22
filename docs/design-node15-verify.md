# Plico 第十五节点设计文档
# 验 — 测试完备与内部问责

**版本**: v1.0
**日期**: 2026-04-22
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 输入安全 + 名称统一 + 单元覆盖 + 变异抗性 + 行为指纹
**前置**: 节点 14 ✅（91%）
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n15-audit`，干净环境）+ 全量源码逐文件 review + 单元测试覆盖率盲区分析 + AIOS 2026 前沿对标
**信息来源**: `docs/dogfood-audit-n14.md` + AdverTest (arXiv:2602.08146) / AgentAssay (arXiv:2603.02601) / FLARE (arXiv:2604.05289) / ToolSimulator (AWS 2026) / Six-Component Harness / MuTON (Trail of Bits) + 代码行级根因分析

---

## 0. AI 第一人称推演：为什么是"验"

> 以下推演完全从 AI Agent 视角出发，不掺杂人类项目管理思维。

### 层次一：我的操作被拒绝，但不是因为我做错了

我是一个 AI Agent，我执行 `delete <CID>` 来清理不需要的数据。系统 panic 了——exit code 101，没有错误信息，没有诊断建议。我的整个工作流中断。

但我没做错什么。CID 是正确的。问题是 `cmd_delete` 只接受 `--cid` flag 而不读 positional arg，导致 CID 成为空字符串，然后 `CASStorage::object_path("")` 对空字符串做 `split_at(2)` → **panic**。

这不是"边缘情况"。这是我最常用的操作路径。

### 层次二：我知道谁在那里，但系统不认识他的名字

我注册了一个叫 `test-agent` 的协作者。我想查他的记忆：`tool call memory.recall {"agent_id":"test-agent"}`。系统返回 `Contract violation: agent_id 'test-agent' not found`。

但他确实存在——用 UUID 查就能找到。系统有两个身份空间（名称和 UUID），但只在一个空间里做验证。对我来说，名称是自然语言接口；UUID 是系统内部标识。两者都应该被接受。

### 层次三：808 个测试全通过，但最危险的代码没有测试

这是我发现的最深层问题。Plico 有 808 个测试，0 失败。但当我逐文件检查时：

| 关键模块 | 源码行数 | 内嵌单元测试 |
|----------|---------|------------|
| `kernel/ops/memory.rs` | 637 | **0** |
| `kernel/builtin_tools.rs` | 757 | **0** |
| `kernel/ops/agent.rs` | 521 | **0** |
| `kernel/ops/fs.rs` | 445 | **0** |
| `kernel/ops/graph.rs` | 784 | **0** |
| `kernel/ops/permission.rs` | — | **0** |
| `kernel/ops/session.rs` | 878 | 有（但仅覆盖 session 流程） |
| 所有 CLI handlers (12个文件) | ~1500+ | **0** |

这些是整个系统的**业务逻辑核心**。它们只被集成测试间接覆盖——如果集成测试的调用路径没有触及某个分支，该分支就是**死角**。

B35 panic 就来自这个死角：`cmd_delete` 从未被测试过"positional CID"输入路径。

### 层次四：从 Mutation Testing 的视角

Trail of Bits (2026) 说得对："**代码覆盖率是最危险的质量指标——它衡量执行，不衡量验证。**"

如果我在 `cmd_delete` 中引入一个 mutant：把 `extract_arg(args, "--cid")` 替换为 `args.get(1).cloned()` ——现有的 808 个测试不会检测到这个变化，因为**没有任何测试直接调用 cmd_delete**。

这意味着现有测试对这些核心模块的**变异杀伤力为零**。

### 推演结论

Node 14 完成了**子系统融合**（记忆 ↔ KG 绑定、接口一致性、自验约束）。
Node 15 的使命是：**确保这些融合不是脆弱的**。

> **测试不完备的融合，比没有融合更危险。**
> 因为它给出"一切正常"的假象（91% 通过率），同时在最关键的路径上留下 panic、空结果、和身份断裂。

---

## 1. Dogfood 实测校正

> **以下全部结论来自 `/tmp/plico-n15-audit` 干净实例的真实 CLI 执行。**
> **不使用管道（消除 `$?` 捕获误差）。每条命令独立运行后立即检查 exit code。**

### 1.1 审计报告 vs 独立 Dogfood 对比

```
Bug    报告状态                    独立实测                              代码行级根因
────── ─────────────────────────── ───────────────────────────────────── ─────────────────────────────────
B35    🔴 CRITICAL delete panic   ✅ 100% 复现确认                     crud.rs:132 cmd_delete
       byte index 2 out of ""     delete <CID> → panic (exit 101)       extract_arg(args, "--cid")
                                  delete --cid <CID> → 正常 (exit 0)    不读 positional arg → CID=""
                                                                        → CASStorage::object_path("")
                                                                        → split_at(2) 空字符串 PANIC

B43    🟡 MEDIUM 名称解析        ✅ 确认                               builtin_tools.rs:390
       agent_id name → error      tool call memory.recall               has_agent(AgentId("name"))
                                  {"agent_id":"test-b43"}               AgentId 是 UUID 包装器
                                  → "Contract violation"                需要 name→UUID 解析
                                  {"agent_id":"<UUID>"} → 正常

B44    🟡 MEDIUM send 权限        ✅ 确认                               messaging.rs:8
       --agent 不传递给权限检查    send --to X --agent Y                 cmd_send_message 用 --from
                                  → "cli lacks SendMessage"             不读 --agent flag
                                  send --to X --from Y → 正确

B45    🟡 NEW agent name bug      ✅ 发现                               agent.rs:20 cmd_agent
       agent --register --name X  agent --register --name X             extract_arg(args, "--register")
       → agent 名称变为 "--name"  → Agent registered as "--name"        吞掉 --name 作为 --register 的值
                                  Available agents 列表含 "--name"
```

### 1.2 新发现：KG 节点可见性

```
发现     描述                                    根因
──────── ─────────────────────────────────────── ────────────────────────
F-K      `nodes` 命令只显示 "cli" agent 的节点    list_nodes 用 agent_id 过滤
         Entity/Fact 节点存在于 kg_nodes.json     默认 agent_id="cli"
         但 CLI 默认 agent="cli" 看不到           Entity 的 agent_id = UUID
         register_agent 创建的 Entity 不可见       需要 --agent <UUID> 才能看到
```

### 1.3 Node 14 最终达成率（校正后）

| 维度 | 承诺 | 实测 | 状态 |
|------|------|------|------|
| D1 记忆完整 | F-1 F-2 | ✅ | 100% |
| D2 接口一致 | F-3 F-4 | F-3 有 B43 名称解析 bug | 75% |
| D3 记忆绑定 | F-5 F-6 | ✅ | 100% |
| D4 降级通路 | F-7 F-8 | ✅ | 100% |
| D5 自验 | F-9 | INV-2 有 B43 bug | 75% |
| **总体** | | | **90%** |

---

## 2. 单元测试覆盖率全量分析

### 2.1 代码规模

```
源文件:   100+ .rs files    38,500 行
测试文件: 30 .rs files      14,476 行
单元测试: 370 passed
集成测试: 433 passed
文档测试: 5 passed
总计:     808 passed, 0 failed
```

### 2.2 内嵌单元测试覆盖地图

**有内嵌 `#[cfg(test)]` 的模块 (45 个)**:

| 类别 | 有测试模块数 | 关键模块 |
|------|------------|---------|
| CAS 层 | 2 | `cas/storage.rs` (3 tests), `cas/object.rs` |
| FS 层 | 10 | `fs/search/hnsw.rs`, `fs/search/memory.rs`, `fs/embedding/*`, `fs/context_budget.rs`, `fs/summarizer.rs` |
| Memory 层 | 4 | `memory/layered/mod.rs`, `memory/persist.rs`, `memory/relevance.rs`, `memory/context_snapshot.rs` |
| Scheduler 层 | 4 | `scheduler/mod.rs`, `scheduler/agent.rs`, `scheduler/dispatch.rs`, `scheduler/messaging.rs` |
| Kernel 子模块 | 12 | `kernel/event_bus.rs`, `kernel/ops/session.rs`, `kernel/ops/cache.rs`, `kernel/ops/prefetch.rs`, `kernel/ops/delta.rs`, 等 |
| 其他 | 13 | `api/semantic.rs`, `api/agent_auth.rs`, `temporal/rules.rs`, `intent/*`, `llm/*`, `tool/*`, `mcp/*`, `bin/plico_mcp.rs` |

**无内嵌测试的关键模块 (完全依赖集成测试)**:

| 模块 | 行数 | 职责 | 风险等级 |
|------|------|------|---------|
| **`kernel/ops/memory.rs`** | 637 | 全部记忆操作 (remember/recall 全 4 tier) | 🔴 CRITICAL |
| **`kernel/builtin_tools.rs`** | 757 | 37 个内置 tool 的执行分发 | 🔴 CRITICAL |
| **`kernel/ops/agent.rs`** | 521 | Agent 生命周期 + skills 注册 | 🔴 HIGH |
| **`kernel/ops/fs.rs`** | 445 | CAS 语义操作 (create/read/search/delete) | 🔴 HIGH |
| **`kernel/ops/graph.rs`** | 784 | KG 全部操作 | 🔴 HIGH |
| **`kernel/mod.rs`** | 1909 | 内核初始化 + API 分发 | 🟡 MEDIUM |
| **`kernel/persistence.rs`** | 418 | 状态持久化 + 恢复 | 🟡 MEDIUM |
| **`kernel/ops/permission.rs`** | — | 权限操作层 | 🟡 MEDIUM |
| **`kernel/ops/messaging.rs`** | — | 消息系统 | 🟡 MEDIUM |
| **`kernel/ops/model.rs`** | 361 | LLM 模型管理 | 🟡 MEDIUM |
| **全部 CLI handlers** (12 文件) | 1500+ | CLI 参数解析 + 调度 | 🔴 CRITICAL |

### 2.3 覆盖率盲区与 Bug 的对应关系

| Bug | 所在文件 | 该文件单元测试 | 能被单元测试捕获？ |
|-----|---------|--------------|------------------|
| B35 (delete panic) | `handlers/crud.rs` | **0** | ✅ 是——测试 `cmd_delete("delete", cid)` 即可发现 |
| B43 (name resolution) | `builtin_tools.rs` | **0** | ✅ 是——测试 `execute_tool("memory.recall", {"agent_id":"name"})` 即可 |
| B44 (send --agent) | `handlers/messaging.rs` | **0** | ✅ 是——测试 `cmd_send_message` 参数提取即可 |
| B45 (register --name) | `handlers/agent.rs` | **0** | ✅ 是——测试 `cmd_agent(["agent","--register","--name","X"])` 即可 |

**结论：所有 4 个现存 Bug 都在无单元测试的模块中。它们都可以被最基础的单元测试捕获。**

---

## 3. AIOS 2026 前沿对标

### 3.1 Agent 测试范式进化

| 论文/工具 | 日期 | 核心概念 | Plico 适用性 |
|-----------|------|---------|------------|
| **AdverTest** (arXiv:2602.08146) | 2026-02 | 对抗式测试生成：Test Agent (T) vs Mutant Agent (M) 交互循环 | 高——可用于生成 kernel/ops 的变异测试 |
| **AgentAssay** (arXiv:2603.02601) | 2026-03 | 5 维 Agent 覆盖率指标 + 行为指纹 + 随机三值判决 (PASS/FAIL/INCONCLUSIVE) | 高——为 Plico 定义 Agent 覆盖率标准 |
| **FLARE** (arXiv:2604.05289) | 2026-04 | 覆盖率引导 fuzzing + 从 agent 定义中提取规范 | 中——可用于 tool API 模糊测试 |
| **ToolSimulator** (AWS) | 2026-04 | LLM 驱动的 tool 模拟 + Pydantic schema 验证 | 中——可用于 builtin_tools 测试 |
| **MuTON/mewt** (Trail of Bits) | 2026-04 | Rust 变异测试 + SQLite 结果存储 + 严重度跳过优化 | 高——直接适用于 Plico Rust 代码 |
| **Six-Component Harness** | 2026 | 六组件闭环：Ground Truth + Memory + Startup + Verification + Contract + Feature Intake | 高——Plico 已有 4/6，缺 Contract + Feature Intake |

### 3.2 关键洞见

**Trail of Bits 的核心论断**: "**代码覆盖率是最危险的质量指标**。高覆盖率可以掩盖关键功能未被测试的事实。"

这精确描述了 Plico 的现状：
- 808 tests, 0 failures
- 但 `kernel/ops/memory.rs` (637 行) 的变异杀伤力 = **0**

**AgentAssay 的 5 维覆盖率**:
1. **Tool coverage** — 是否所有 tool 都被调用过
2. **State coverage** — 是否覆盖了所有 agent 状态转换
3. **Interaction coverage** — 是否测试了 agent 间通信路径
4. **Error coverage** — 是否测试了错误路径
5. **Workflow coverage** — 是否测试了完整的多步骤工作流

Plico 当前主要覆盖维度 1 和 5（通过集成测试），但维度 2-4 严重不足。

---

## 4. Node 15 主题与五大维度

### 主题：**验 (Verify)** — 测试完备与内部问责

> "融合了但不验证，等于没有融合。"

Node 14 完成了子系统融合。Node 15 确保这些融合在每个边界条件下都是健壮的。

### 五大维度

| 编号 | 维度 | 目标 | 对标 |
|------|------|------|------|
| D1 | **输入安全 (Input Safety)** | 所有公共接口的输入边界防护 | FLARE 规范提取 |
| D2 | **名称统一 (Name Resolution)** | 统一的名称↔UUID 解析层 | AgentSys 边界验证 |
| D3 | **单元覆盖 (Unit Coverage)** | 关键模块 100% 分支覆盖 | AdverTest 对抗覆盖 |
| D4 | **变异抗性 (Mutation Resistance)** | Mutant Escape Rate < 20% | MuTON/mewt + AgentAssay |
| D5 | **行为指纹 (Behavioral Fingerprint)** | CLI 行为回归检测 | AgentAssay 行为指纹 |

---

## 5. 特性设计

### F-1: CID 输入防御（D1 输入安全）

**修复 B35 (CRITICAL) + 预防性防御**

根因：`CASStorage::object_path()` 和 `shard_dir()` 对 CID < 2 字符做 `split_at(2)` → panic。
`cmd_delete` 不读 positional arg 导致 CID 为空字符串。

```rust
// cas/storage.rs — 防御性 CID 验证
fn validate_cid(cid: &str) -> Result<(), CASError> {
    if cid.len() < 2 {
        return Err(CASError::NotFound { cid: cid.to_string() });
    }
    if !cid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(CASError::NotFound { cid: cid.to_string() });
    }
    Ok(())
}

fn object_path(&self, cid: &str) -> Result<PathBuf, CASError> {
    validate_cid(cid)?;
    let (prefix, rest) = cid.split_at(2);
    Ok(self.root.join(prefix).join(rest))
}
```

```rust
// handlers/crud.rs — 修复 positional CID 解析
pub fn cmd_delete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    if cid.is_empty() {
        return ApiResponse::error("delete requires a CID: delete <CID> or delete --cid <CID>");
    }
    // ...
}
```

**测试**:
- `test_object_path_empty_cid()` — 空 CID 返回 NotFound，不 panic
- `test_object_path_short_cid()` — 1 字符 CID 返回 NotFound
- `test_cmd_delete_positional()` — `["delete", "<cid>", "--agent", "cli"]` 正常工作
- `test_cmd_delete_flag()` — `["delete", "--cid", "<cid>"]` 正常工作
- `test_cmd_delete_empty()` — `["delete"]` 返回友好错误

**影响**: `cas/storage.rs` ~20行 + `handlers/crud.rs` ~10行 + 5 个新单元测试

---

### F-2: CLI 参数解析统一（D1 输入安全 + D2 名称统一）

**修复 B44, B45 + 系统性 CLI 一致性**

当前问题：
- `cmd_send_message` 读 `--from` 不读 `--agent`
- `cmd_agent` 的 `--register` 吞掉下一个参数
- 不同命令对 positional arg 的处理不一致

```rust
// commands/mod.rs — 新增统一参数提取助手
pub fn extract_arg_or_positional(args: &[String], flag: &str, pos: usize) -> Option<String> {
    extract_arg(args, flag)
        .or_else(|| args.get(pos).cloned().filter(|a| !a.starts_with("--")))
}

pub fn extract_agent_id(args: &[String]) -> String {
    extract_arg(args, "--agent")
        .or_else(|| extract_arg(args, "--from"))
        .unwrap_or_else(|| "cli".to_string())
}
```

**修复 B44**:
```rust
// handlers/messaging.rs
pub fn cmd_send_message(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_agent_id(args); // 统一读取 --agent 或 --from
    // ...
}
```

**修复 B45**:
```rust
// handlers/agent.rs
pub fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // ...
    let name = extract_arg(args, "--name").unwrap_or_else(|| "unnamed".to_string());
    let id = kernel.register_agent(name);
    // ...
}
```

**测试**:
- `test_extract_agent_id_from()` — `--from` 参数正确提取
- `test_extract_agent_id_agent()` — `--agent` 参数正确提取
- `test_cmd_agent_register_name()` — `["agent", "--register", "--name", "foo"]` → name="foo"
- `test_cmd_send_agent_context()` — `["send", "--to", "X", "--agent", "Y"]` → from="Y"

**影响**: `commands/mod.rs` ~15行 + `messaging.rs` ~3行 + `agent.rs` ~5行 + 4 个新单元测试

---

### F-3: 统一名称解析层（D2 名称统一）

**修复 B43 — Agent 名称/UUID 双空间解析**

```rust
// kernel/ops/agent.rs — 新增 resolve_agent_validated
pub fn resolve_agent_validated(&self, name_or_id: &str) -> Result<AgentId, String> {
    match self.scheduler.resolve(name_or_id) {
        Some(aid) => Ok(aid),
        None => {
            let available: Vec<String> = self.scheduler.list_agents()
                .into_iter().map(|h| h.name).collect();
            Err(format!(
                "Agent '{}' not found. Available: {:?}",
                name_or_id, available
            ))
        }
    }
}
```

```rust
// kernel/builtin_tools.rs — INV-2 修复
"memory.recall" => {
    let param_agent_id = params.get("agent_id").and_then(|v| v.as_str());
    let effective_agent = if let Some(name_or_id) = param_agent_id {
        match self.resolve_agent_validated(name_or_id) {
            Ok(aid) => aid.0,
            Err(msg) => return ToolResult::error(format!("Contract violation: {}", msg)),
        }
    } else {
        agent_id.to_string()
    };
    // ...
}
```

**测试**:
- `test_resolve_by_name()` — 名称解析返回正确 UUID
- `test_resolve_by_uuid()` — UUID 直接返回
- `test_resolve_not_found()` — 未知名称返回可用列表
- `test_tool_recall_by_name()` — `memory.recall {"agent_id":"name"}` 正常工作
- `test_tool_recall_by_uuid()` — `memory.recall {"agent_id":"uuid"}` 正常工作

**影响**: `agent.rs` ~15行 + `builtin_tools.rs` ~10行 + 5 个新单元测试

---

### F-4: kernel/ops 单元测试框架（D3 单元覆盖）

**为全部无测试的 kernel/ops 模块建立单元测试基础设施**

这是 Node 15 的**核心特性**。目标不是达到 100% 行覆盖（那是指标陷阱），而是确保每个公共方法的**关键路径**和**错误路径**都被验证。

```rust
// kernel/ops/memory.rs — 新增 #[cfg(test)] mod tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::AIKernel;
    use tempfile::tempdir;

    fn make_kernel() -> (AIKernel, tempfile::TempDir) {
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempdir().unwrap();
        let k = AIKernel::new(dir.path().to_path_buf()).unwrap();
        (k, dir)
    }

    #[test]
    fn test_remember_ephemeral_returns_id() {
        let (k, _d) = make_kernel();
        let id = k.remember("cli", "default", "hello".into());
        assert!(id.is_ok());
        assert!(!id.unwrap().is_empty());
    }

    #[test]
    fn test_recall_tier_filter_working() {
        let (k, _d) = make_kernel();
        let _ = k.remember_working("cli", "default", "w1".into(), vec![]);
        let _ = k.remember("cli", "default", "e1".into());
        let all = k.recall("cli", "default");
        assert!(all.len() >= 1);
    }

    #[test]
    fn test_remember_procedural_stores_correctly() {
        let (k, _d) = make_kernel();
        let steps = vec![crate::memory::layered::ProcedureStep {
            step_number: 0,
            description: "test".into(),
            action: "test".into(),
            expected_outcome: "ok".into(),
        }];
        let result = k.remember_procedural("cli", "default", "proc1".into(), "desc".into(), steps, "test".into(), vec![]);
        assert!(result.is_ok());
    }
}
```

**覆盖目标清单**:

| 模块 | 需要的测试 | 新增测试数(估) |
|------|----------|-------------|
| `kernel/ops/memory.rs` | remember(所有tier), recall, recall_semantic, promote, evict | 12 |
| `kernel/ops/agent.rs` | register, resolve, status, suspend/resume, checkpoint, register_skill | 10 |
| `kernel/builtin_tools.rs` | execute_tool 各分支 (cas.*, memory.*, kg.*, agent.*) | 15 |
| `kernel/ops/fs.rs` | semantic_create, semantic_search, semantic_delete, version_history | 8 |
| `kernel/ops/graph.rs` | kg_add_node, kg_add_edge, kg_find_paths, kg_list_nodes | 8 |
| `handlers/crud.rs` | cmd_create, cmd_read, cmd_search, cmd_delete, cmd_update | 8 |
| `handlers/memory.rs` | cmd_remember, cmd_recall, parse_memory_tier | 6 |
| `handlers/agent.rs` | cmd_agent, cmd_quota, cmd_discover | 5 |
| `handlers/messaging.rs` | cmd_send_message, cmd_read_messages | 4 |
| `handlers/skills.rs` | cmd_skills_register, cmd_skills_list, cmd_skills_discover | 5 |
| **总计** | | **~81** |

---

### F-5: CAS 防御层加固（D4 变异抗性）

**在 CAS 核心操作中添加输入验证和属性测试**

```rust
// cas/storage.rs — 扩展测试
#[cfg(test)]
mod tests {
    // ... 已有 3 tests ...

    #[test]
    fn test_empty_cid_returns_error() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();
        assert!(storage.get("").is_err());
        assert!(storage.get("a").is_err());
    }

    #[test]
    fn test_delete_nonexistent_is_ok() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();
        assert!(storage.delete("0000000000000000000000000000000000000000000000000000000000000000").is_ok());
    }

    #[test]
    fn test_list_cids_excludes_temp() {
        let dir = tempdir().unwrap();
        let storage = CASStorage::new(dir.path().to_path_buf()).unwrap();
        let obj = AIObject::new(b"test".to_vec(), AIObjectMeta::text(["t"]));
        storage.put(&obj).unwrap();
        let cids = storage.list_cids().unwrap();
        assert!(cids.iter().all(|c| !c.contains(".tmp")));
    }
}
```

**影响**: `cas/storage.rs` ~40行新测试 + CID 验证逻辑 ~15行

---

### F-6: CLI 行为快照测试（D5 行为指纹）

**建立 CLI 命令的"行为基线"回归测试**

受 AgentAssay 行为指纹启发。核心思想：记录一组标准操作序列的输出模式，任何代码变更后检查输出是否偏离基线。

```rust
// tests/cli_behavior_test.rs
#[test]
fn test_cli_behavior_fingerprint() {
    let dir = tempdir().unwrap();
    let root = dir.path();

    // 行为序列：put → get → search → remember → recall → delete
    let behaviors = vec![
        (vec!["put", "--content", "hello", "--tags", "t1", "--agent", "cli"], "CID:"),
        (vec!["search", "hello"], "relevance="),
        (vec!["remember", "--tier", "working", "--content", "mem1", "--agent", "cli"], "Memory stored"),
        (vec!["recall", "--agent", "cli", "--tier", "working"], "[Working]"),
        (vec!["agent", "--register", "--name", "test"], "Agent ID:"),
        (vec!["session-start", "--agent", "cli"], "Session started"),
        (vec!["session-end", "--agent", "cli"], "Session ended"),
    ];

    for (args, expected_pattern) in &behaviors {
        let output = run(root, args);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(expected_pattern),
            "Command {:?} output did not contain '{}'. Got: {}",
            args, expected_pattern, stdout
        );
        assert!(output.status.success(), "Command {:?} failed", args);
    }
}
```

**影响**: `tests/cli_behavior_test.rs` ~100行新文件

---

## 6. 代码影响分析

### 修改文件

| 文件 | 修改类型 | 影响行数(估) |
|------|---------|------------|
| `cas/storage.rs` | CID 验证 + 新测试 | +55 |
| `handlers/crud.rs` | positional CID 解析 (F-1) | +10 |
| `handlers/messaging.rs` | --agent 统一 (F-2) | +3 |
| `handlers/agent.rs` | --name 解析修复 (F-2) | +10 |
| `commands/mod.rs` | 统一参数助手 (F-2) | +15 |
| `kernel/ops/agent.rs` | resolve_agent_validated (F-3) | +15 |
| `kernel/builtin_tools.rs` | INV-2 名称解析 (F-3) | +10 |
| `kernel/ops/memory.rs` | 新增单元测试 (F-4) | +120 |
| `kernel/ops/agent.rs` | 新增单元测试 (F-4) | +100 |
| `kernel/builtin_tools.rs` | 新增单元测试 (F-4) | +150 |
| `kernel/ops/fs.rs` | 新增单元测试 (F-4) | +80 |
| `kernel/ops/graph.rs` | 新增单元测试 (F-4) | +80 |
| `handlers/crud.rs` | 新增单元测试 (F-4) | +60 |
| `handlers/memory.rs` | 新增单元测试 (F-4) | +50 |
| `handlers/agent.rs` | 新增单元测试 (F-4) | +40 |
| `handlers/messaging.rs` | 新增单元测试 (F-4) | +30 |
| `handlers/skills.rs` | 新增单元测试 (F-4) | +40 |

### 新增文件

| 文件 | 职责 |
|------|------|
| `tests/cli_behavior_test.rs` | CLI 行为指纹回归测试 (F-6) |

### 总量

- **Bug 修复代码**: ~63 行
- **新增单元测试**: ~750+ 行 (~81 个新测试)
- **新增行为测试**: ~100 行
- **总计**: ~913 行变更

---

## 7. 实施计划

### Sprint 1 (Day 1-3): 输入安全
- [ ] F-1: CID 输入防御 + B35 修复
- [ ] F-2: CLI 参数解析统一 + B44/B45 修复
- [ ] F-5: CAS 防御层加固

验收: `cargo test` 全通过 + `delete <CID>` 不再 panic

### Sprint 2 (Day 4-6): 名称解析 + 核心测试
- [ ] F-3: 统一名称解析层 + B43 修复
- [ ] F-4 (Part 1): `kernel/ops/memory.rs` + `kernel/ops/agent.rs` 单元测试

验收: `tool call memory.recall {"agent_id":"name"}` 正常工作 + 22 个新测试通过

### Sprint 3 (Day 7-9): 扩展测试覆盖
- [ ] F-4 (Part 2): `kernel/builtin_tools.rs` + `kernel/ops/fs.rs` + `kernel/ops/graph.rs` 单元测试
- [ ] F-4 (Part 3): 全部 CLI handlers 单元测试

验收: 新增 ~59 个单元测试 + 覆盖率盲区消除

### Sprint 4 (Day 10-11): 行为指纹
- [ ] F-6: CLI 行为快照测试
- [ ] 最终 Dogfood 验证 (B35/B43/B44/B45 全修复确认)

验收: `tests/cli_behavior_test.rs` 全通过 + 全部 Bug 修复确认

---

## 8. 验收记分卡

| 特性 | 验收标准 | 自动化检查 |
|------|---------|-----------|
| F-1 | `delete <CID>` 不 panic; 空/短 CID 返回错误 | `cargo test test_empty_cid` + `test_cmd_delete_positional` |
| F-2 | `send --agent X` 权限检查用 X; `agent --register --name Y` 名称为 Y | `cargo test test_cmd_send_agent_context` + `test_cmd_agent_register_name` |
| F-3 | `tool call memory.recall {"agent_id":"name"}` 返回数据 | `cargo test test_tool_recall_by_name` |
| F-4 | kernel/ops 全部关键方法有单元测试 | `cargo test -- kernel::ops::memory::tests kernel::ops::agent::tests` |
| F-5 | CAS 输入验证覆盖空/短/非法 CID | `cargo test -- cas::storage::tests` |
| F-6 | CLI 行为序列输出稳定 | `cargo test --test cli_behavior_test` |

**定量目标**:
- 现存 Bug: 4/4 修复 (B35, B43, B44, B45)
- 新增单元测试: ≥ 80 个
- kernel/ops 模块单元覆盖: 0% → ≥ 60%
- CLI handlers 单元覆盖: 0% → ≥ 50%
- CAS panickable 路径: ≥ 1 → 0

---

## 9. Soul 2.0 对齐

| 公理 | Node 15 对齐 |
|------|-------------|
| **Token Economy** | F-3 减少因名称解析失败的无效 round-trip |
| **Intent Accuracy** | F-2 确保 CLI 参数正确传递用户意图 |
| **Memory Integrity** | F-1 消除 CAS 操作中的 panic 路径 |
| **Operational Continuity** | F-4 通过单元测试保证代码变更不破坏核心逻辑 |
| **Self-Improvement** | F-6 行为指纹实现自动化回归检测 |

---

## 10. AIOS Roadmap 定位

```
Node 1-3: 存储层 (CAS + 语义搜索 + KG)
Node 4-6: 协作层 (多 Agent + 权限 + 闭环)
Node 7-8: 驾具层 (事件 + Harness)
Node 9-10: 韧性层 (弹性 + 整流)
Node 11-12: 自演进层 (记忆自动化 + 持久化)
Node 13: 传导层 (API 统一 + MCP)
Node 14: 融合层 (子系统融合 + 自验)
>>> Node 15: 验证层 (测试完备 + 内部问责) <<<
Node 16+: 观测层 (运行时自省 + 异常感知)
```

Node 15 在 AIOS 路线上的位置是**基础设施加固**。AIOS v0.3.0 (2026-01) 已经开始做 Rust scaffold (`aios-rs/`)。Plico 作为 Rust-native AIOS 实现，其质量保证体系必须先于功能扩展。

---

## 11. 风险分析

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-4 测试数量大（~81个），可能延期 | 高 | 中 | 按 Sprint 分批，优先 CRITICAL 模块 |
| CID 验证改变 API 行为，影响现有调用者 | 低 | 中 | CID 验证返回已有错误类型（NotFound） |
| 名称解析缓存未考虑 Agent 注册/删除 | 低 | 低 | resolve 是 O(n) 扫描，无缓存一致性问题 |
| CLI handlers 测试需要 mock Kernel | 中 | 中 | 使用 `make_kernel()` + tempdir，与集成测试同模式 |
| 行为指纹对输出格式变化敏感 | 中 | 低 | 用 `contains()` 而非精确匹配 |

---

*文档基于 `/tmp/plico-n15-audit` 干净实例的 dogfood 实测 + 38,500 行源码逐文件 review + 808 个测试的覆盖率盲区分析 + AIOS 2026 前沿对标。*
*所有 Bug 根因均通过代码行级阅读确认，不依赖 Git 日志。*
