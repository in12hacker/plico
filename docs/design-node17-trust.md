# Plico 第十七节点设计文档
# 信 — 操作诚信与效果合约

**版本**: v1.0
**日期**: 2026-04-22
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: 输入保真 + 效果合约 + 工具前后置条件 + P0 单元测试 + 灵魂违规修复
**前置**: 节点 16 ✅（95%） — Bug 6/6 修复, 测试 36/40, 持久化完成, 幽灵防御(delete)完成
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n17-audit4`，干净环境）+ 全量源码逐文件 review（真实读取 60+ 源文件）+ cargo test 934 全通过确认 + 逐文件覆盖率交叉验证
**信息来源**: `docs/dogfood-audit-n16.md` + TDAD (arXiv:2603.08806, Mar 2026) / Trace-Based Assurance (arXiv:2603.18096, Mar 2026) / AgentAssay (arXiv:2603.02601, Mar 2026) / NLAH (arXiv:2603.25723, Mar 2026) / VIGIL (arXiv:2512.07094) / Silent Failures 2026 + Rust AI Infrastructure Shift (OSS Insight, 2026)

---

## 0. AI 第一人称推演：为什么是"信"

### 层次一：我存了东西，但什么也没存进去

我是一个用 Plico 管理知识的 AI Agent。我调用 `put "关于模块架构的重要发现" --tags "architecture,insight"`。系统返回 `CID: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855`。我信任这个 CID，把它存入 KG 作为 Document 节点。

一周后，我需要回溯这个发现。我用 CID 查询——返回的是空内容。

我的操作系统给了我一张**空白收据**，而我一直把它当作有效凭证。这个 CID 是有效的——它是 SHA-256(空字符串)。但它不代表我存入的任何东西。

**根因链**:

```
cmd_create(args = ["put", "关于模块架构的重要发现", "--tags", "architecture,insight"])
  → extract_arg(args, "--content") → None（没有 --content flag）
  → unwrap_or_default() → ""（空字符串）
  → kernel.semantic_create("".as_bytes(), tags, agent_id, None)
  → CAS.put(AIObject { data: b"", meta: ... })
  → SHA-256(b"") = e3b0c44298fc...
  → 返回 Ok("e3b0c44298fc...")
  → CLI 打印 "CID: e3b0c44298fc..."
  → EXIT 0
```

`cmd_create` 只读 `--content` flag，完全忽略位置参数。用户的自然语法 `put "content"` 静默变成 `put --content "" --tags ...`。空字符串被 SHA-256、存储、返回，一切看起来正常。

从 TDAD (arXiv:2603.08806) 角度：**没有行为合约**断言"带非空输入的 put 必须产生非空输出"。规格存在于意图中（`put` 的语义就是"存入内容"），但它没有被强制执行为机器可检查的断言。

### 层次二：幽灵疫苗只防了一个变种

Node 16 修复了 B46（幽灵删除）——`delete` 现在正确传播 CAS 错误。但 B49 暴露了**同一病毒的新变种**：

| Bug | CLI 命令 | 预期效果 | 实际效果 | 系统报告 |
|-----|---------|---------|---------|---------|
| B46 | `delete "a"` | 报告错误 | 什么都没做 | ✅ "Deleted: a → recycle bin" |
| B49 | `put "content"` | 存入 "content" | 存入 "" | ✅ "CID: e3b0c44..." |

两者的共同模式：**CLI 层声称成功，但不验证操作是否产生了预期效果。**

B46 的修复是定点的（在 `SemanticFS::delete` 中传播 CAS error）。但根因是架构性的：**操作完成后无后置条件检查**。

Trace-Based Assurance (arXiv:2603.18096, Mar 2026) 定义了步骤合约："tool calls with side effects must be preceded by a verification step." Plico 既没有步骤合约，也没有验证步骤。

### 层次三：37 个工具已注册，但它们的承诺无人核验

`builtin_tools.rs` 注册了 37 个工具，每个都有 JSON Schema 定义输入合约——参数类型、必需字段。但**没有输出合约**。

当 `cas.create` 返回 CID 时，没有任何机制检查：
- CID 是否非平凡（不是 SHA-256 of empty）?
- 返回的 CID 是否实际存在于 CAS 中?
- `get(returned_cid).data` 是否等于输入内容?

Agent Tool Contracts (2026, "9 Clauses That Stop Silent Misfires") 的核心条款之一是**效果验证**。工具合约必须包含后置条件，不仅仅是前置条件。

```
当前状态：
  工具注册 → ✅ 37 个工具
  输入 Schema → ✅ JSON Schema 定义
  权限检查 → ✅ PermissionGuard
  效果验证 → ❌ 不存在
  输出合约 → ❌ 不存在

AIOS 完整工具合约需要：
  前置条件 (precondition)  → 输入合法 + 权限通过
  执行 (execution)         → 调用内核方法
  后置条件 (postcondition) → 效果验证
  审计 (audit trail)       → 操作追踪
```

### 层次四：最大的功能模块零测试——这本身就是问题

`builtin_tools.rs`（761 行, 37 个工具分发）从 Node 14 开始在每一轮审计中被标记。它是 B43 的发源地。它是工具 API 调用到内核方法的翻译层。**零单元测试**。

Node 16 的 36 个新测试覆盖了 `ops/agent.rs`（8）、`ops/memory.rs`（7）、`ops/fs.rs`（6）、`ops/graph.rs`（15）——但跳过了重心所在。这像是测试了汽车的轮子、刹车、引擎，但没测转向机构。

AgentAssay (arXiv:2603.02601) 提出了"agent 执行模型的覆盖率指标"——工具分发层**就是** agent 执行模型。不测试它等于不测试系统的核心翻译层。

### 层次五：Plico 作为 Intelligent Harness Runtime

NLAH (arXiv:2603.25723) 将驾具层外化为带显式合约的自然语言制品。Plico 的 `CLAUDE.md` 和 `AGENTS.md` 已经是 proto-NLAH——它们引导 Agent 行为。但缺少：

| NLAH 组件 | Plico 状态 | 差距 |
|-----------|-----------|------|
| Contracts | ❌ 无机器可检查的前/后置条件 | 核心缺失 |
| Roles | ✅ Agent 注册 + 权限 | 已有 |
| Adapters | ❌ 无确定性验证钩子 | 核心缺失 |
| State conventions | ✅ CAS + KG + Memory | 已有 |
| Stages | ⚠️ Intent 路由但无阶段合约 | 部分 |

VIGIL (arXiv:2512.07094) 的 Observation → Evaluation → Adaptation 循环：
- Plico 有 **Observation**（事件总线、审计日志）
- 缺 **Evaluation**（合约检查）
- 缺 **Adaptation**（合约违反时的自愈）

### 推演结论

Node 15 完成了**输入安全**。Node 16 完成了**持久化与幽灵防御(delete)**。

Node 17 的使命：**让系统的每个操作都诚实——输入被完整捕获，效果被真实验证，工具承诺有合约保障。**

"信"的三层含义：
1. **自信**（Self-trust）：系统信任自己的操作产生了真实效果
2. **可信**（Trustworthiness）：外部 Agent 可以信任 Plico 的报告
3. **保真**（Fidelity）：输入保真（B49）+ 操作保真（效果合约）+ 报告保真（无虚假成功）

---

## 1. 审计发现总结

### 1.1 N16 达成率

| 维度 | 承诺 | 实现 | 验证证据 |
|------|------|------|---------|
| D1 持久运营 | ~/.plico 默认 | ✅ 100% | `main.rs:60-62` 确认 `dirs::home_dir().join(".plico")` |
| D2 幽灵防御 | delete 错误传播 | ✅ 100% | `semantic_fs/mod.rs:355` `self.cas.get(cid)?` |
| D3 解析完备 | skills 全路径解析 | ✅ 100% | `skills.rs` 4 个命令全用 `resolve_agent()` |
| D4 幂等引导 | bootstrap 去重 | ✅ 100% | `plico-bootstrap.sh` 幂等检查 |
| D5 操作审计 | 事件留痕 | ✅ 100% | 事件总线 + 审计日志 |
| D6 规格绑定 | ADR 入库 | ✅ 100% | 15 个历史 ADR |
| **F-5 单元测试** | **~40 新测试** | **🟡 90%** | **36/40, builtin_tools/handlers 仍为 0** |

### 1.2 新发现 Bug

| ID | 严重度 | 描述 | 根因 | 发现方式 |
|----|--------|------|------|---------|
| **B49** | 🔴 CRITICAL | `put "content"` 位置参数被忽略，存入空内容 | `cmd_create` 只读 `--content` flag | Dogfood: `put "content"` → CID=SHA256("") |
| **V-06** | 🟠 P2 | `create()` 自动 LLM 摘要（灵魂违规） | `semantic_fs/mod.rs:198-213` 隐式调用 summarizer | 代码审查 |
| **V-07** | 🟡 P3 | `create()` 自动 KG 关联（灵魂边界） | `semantic_fs/mod.rs:189-196` 隐式 upsert | 代码审查 |

### 1.3 单元测试覆盖率现状

| 指标 | 值 |
|------|-----|
| 总测试数 | **934** (414 unit + 50 binary + 456 integration + 5 doc + 9 mcp) |
| 有效代码文件 | **73** |
| 有测试覆盖 | **53** (含伴生 tests.rs) |
| **文件覆盖率** | **72.6%** (53/73) |
| **P0 零测试文件** | **3** (`builtin_tools.rs` 761行, `persistence.rs` 418行, `execution.rs` 265行) |
| P1 零测试文件 | 4 (`dashboard.rs`, `model.rs`, `context_loader.rs`, `graph/types.rs`) |

### 1.4 历史 Bug 状态

| Bug | 状态 | 验证 |
|-----|------|------|
| B35 delete panic | ✅ Fixed (N14) | `delete <CID>` 正常 |
| B43 名称解析 | ✅ Fixed (N15) | tool call 成功 |
| B44 send --agent | ✅ Fixed (N15) | 权限检查正确 |
| B45 register --name | ✅ Fixed (N15) | name 正确注册 |
| B46 phantom delete | ✅ Fixed (N16) | `delete "a"` → InvalidCid |
| B47 skills name | ✅ Fixed (N16) | skills register by name 成功 |
| **B49 phantom put** | **❌ New (N17)** | `put "content"` → 空内容 |

---

## 2. Node 17 六大维度

### D1: 输入保真（Input Fidelity）

**问题**: CLI 命令的内容参数处理不一致——`put "text"` 位置参数被忽略。
**目标**: 所有接受内容的 CLI 命令统一支持位置参数和 flag 参数。
**度量**: 0 个 CLI 命令忽略位置内容参数。

### D2: 效果合约（Effect Contracts）

**问题**: 操作返回 `Ok(())` 但不验证是否产生了预期副作用。
**目标**: 关键 write/delete 操作包含后置条件断言。
**度量**: `semantic_create` 返回的 CID 在 CAS 中可验证；空内容 `put` 返回错误。

### D3: 工具合约化（Tool Contracts）

**问题**: 37 个 builtin tools 有输入 Schema 但无输出合约、无效果验证。
**目标**: P0 工具（cas.create, cas.delete, memory.store）具有机器可检查的后置条件。
**度量**: P0 工具后置条件断言 100%。

### D4: P0 测试覆盖（Test Coverage — P0 Files）

**问题**: `builtin_tools.rs`(761行)、`persistence.rs`(418行)、`execution.rs`(265行) 零测试。
**目标**: 每个 P0 文件至少 8 个有效单元测试。
**度量**: P0 文件测试数 >= 24 total。

### D5: 灵魂违规修复（Soul Alignment）

**问题**: V-06 `create()` 隐式调用 LLM 摘要——OS 层策略泄漏。
**目标**: 自动摘要改为 API 参数 `auto_summarize: bool`，默认 `false`。
**度量**: `semantic_create` 不再隐式调用 Summarizer。

### D6: CLI 系统审计（CLI Systematic Audit）

**问题**: B49 暴露了 CLI 层可能存在更多位置参数处理不一致。
**目标**: 系统性审计所有 CLI handler，确保位置参数、flag 参数、agent 解析一致。
**度量**: 0 个 CLI handler 忽略预期的位置参数。

---

## 3. Node 17 特性清单

### F-1: CLI Input Fidelity — 修复 B49 + 系统审计

**根因**: `cmd_create` (crud.rs:8) 只用 `extract_arg(args, "--content")`，忽略位置参数。

**修复**:

```rust
// crud.rs — cmd_create 修复
pub fn cmd_create(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let content = extract_arg(args, "--content")
        .or_else(|| {
            // 支持位置参数: put "content" --tags ...
            args.get(1)
                .filter(|a| !a.starts_with("--"))
                .cloned()
        })
        .unwrap_or_default();

    if content.is_empty() {
        return ApiResponse::error(
            "put requires content: put <content> --tags ... or put --content <content> --tags ..."
        );
    }

    let tags = extract_tags(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let intent = extract_arg(args, "--intent");

    match kernel.semantic_create(content.into_bytes(), tags, &agent_id, intent) {
        Ok(cid) => ApiResponse::with_cid(cid),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
```

**系统审计范围**: 审查所有 16 个 handler 文件，确认：
1. 每个 content 参数同时支持位置参数和 flag
2. 空内容输入显式拒绝（不是静默接受）
3. Agent ID 解析统一使用 `resolve_agent()`

**预期审计结果**:

| Handler | 文件 | 位置参数 | Flag | 空输入检查 | resolve_agent |
|---------|------|---------|------|-----------|---------------|
| cmd_create | crud.rs | ❌→✅ | ✅ | ❌→✅ | ❌→审计 |
| cmd_read | crud.rs | ✅ | ❌ | — | ❌→审计 |
| cmd_update | crud.rs | ❌ | ✅ | ❌→审计 | ❌→审计 |
| cmd_delete | crud.rs | ✅ | ✅ | ✅ | ❌→审计 |
| cmd_search | crud.rs | ✅ | ✅ | ✅ | ❌→审计 |
| cmd_remember | memory.rs | ? | ✅ | ?→审计 | ?→审计 |
| cmd_send | messaging.rs | ? | ✅ | ?→审计 | ✅ |
| cmd_agent | agent.rs | — | ✅ | — | ✅ |
| cmd_skills | skills.rs | — | ✅ | ✅ | ✅ (N16) |

### F-2: Effect Contracts — 效果合约系统

**设计**: 在 `semantic_create` 层增加返回值 enrichment，在 CLI 层增加后置条件检查。

**内核层**:

```rust
// kernel/ops/fs.rs — semantic_create 效果合约
pub fn semantic_create(
    &self, content: Vec<u8>, tags: Vec<String>,
    agent_id: &str, intent: Option<String>,
) -> Result<String, std::io::Error> {
    // 前置条件: 内容非空
    if content.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Cannot create object with empty content"
        ));
    }

    let cid = self.fs.create(content.clone(), tags, agent_id, intent)?;

    // 后置条件: CID 在 CAS 中可检索
    debug_assert!(
        self.fs.cas_ref().get(&cid).is_ok(),
        "Effect contract violated: created CID {} not found in CAS", cid
    );

    Ok(cid)
}
```

**审计日志合约** (新增 `AuditContract` 概念):

```rust
// fs/semantic_fs/mod.rs — 审计合约
pub struct AuditContract {
    pub operation: AuditAction,
    pub precondition_met: bool,
    pub postcondition_met: bool,
    pub effect_verified: bool,
}

impl SemanticFS {
    pub fn delete(&self, cid: &str, agent_id: String) -> std::io::Result<AuditContract> {
        let obj = self.cas.get(cid)?;  // precondition: CID exists

        self.recycle_bin.write().unwrap().insert(cid.to_string(), RecycleEntry { ... });

        // postcondition: CID in recycle bin
        let in_bin = self.recycle_bin.read().unwrap().contains_key(cid);

        self.audit_log.write().unwrap().push(AuditEntry { ... });

        Ok(AuditContract {
            operation: AuditAction::Delete,
            precondition_met: true,
            postcondition_met: in_bin,
            effect_verified: true,
        })
    }
}
```

### F-3: Tool Pre/Post-Conditions — builtin_tools 合约化

**设计**: 在 `builtin_tools.rs` 的 `dispatch_tool_call` 中增加后置条件检查。

```rust
// kernel/builtin_tools.rs — P0 工具后置条件
"cas.create" => {
    let content = args["content"].as_str().unwrap_or("").to_string();
    let tags: Vec<String> = /* ... */;

    // 前置条件
    if content.is_empty() {
        return ToolResult::error("cas.create requires non-empty content");
    }

    match kernel.semantic_create(content.into_bytes(), tags, agent_id, None) {
        Ok(cid) => {
            // 后置条件: CID 可检索
            if kernel.get_object(&cid, agent_id, "default").is_err() {
                tracing::error!("Effect contract violated: cas.create returned CID {} but get failed", cid);
                return ToolResult::error(format!("Internal error: created object {} not retrievable", cid));
            }
            ToolResult::ok(json!({"cid": cid}))
        }
        Err(e) => ToolResult::error(e.to_string()),
    }
}
```

**P0 工具合约覆盖**:

| 工具 | 前置条件 | 后置条件 |
|------|---------|---------|
| cas.create | content 非空 | CID 可检索 |
| cas.delete | CID 存在 | CID 在 recycle bin |
| cas.read | CID 有效 | data 非空 |
| memory.store | content 非空 | recall 可检索 |
| kg.add_node | label 非空 | node_id 可查 |

### F-4: P0 Unit Tests — 三大零测试文件

#### F-4a: `kernel/builtin_tools.rs` 单元测试 (~10 tests)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_kernel() -> (AIKernel, tempfile::TempDir) {
        crate::kernel::tests::make_kernel()
    }

    #[test]
    fn test_cas_create_dispatch() {
        let (kernel, _dir) = make_kernel();
        let result = kernel.dispatch_tool_call(
            "cas.create",
            &serde_json::json!({"content": "test data", "tags": ["unit-test"]}),
            "kernel", "default",
        );
        assert!(result.success);
        assert!(result.output["cid"].is_string());
    }

    #[test]
    fn test_cas_create_empty_content_rejected() {
        let (kernel, _dir) = make_kernel();
        let result = kernel.dispatch_tool_call(
            "cas.create",
            &serde_json::json!({"content": "", "tags": []}),
            "kernel", "default",
        );
        assert!(!result.success);
    }

    #[test]
    fn test_cas_read_existing() { /* ... */ }

    #[test]
    fn test_cas_read_nonexistent() { /* ... */ }

    #[test]
    fn test_cas_delete_existing() { /* ... */ }

    #[test]
    fn test_cas_delete_nonexistent_returns_error() { /* ... */ }

    #[test]
    fn test_memory_store_and_recall() { /* ... */ }

    #[test]
    fn test_memory_recall_agent_name_resolution() { /* ... */ }

    #[test]
    fn test_kg_add_node_dispatch() { /* ... */ }

    #[test]
    fn test_unknown_tool_returns_error() {
        let (kernel, _dir) = make_kernel();
        let result = kernel.dispatch_tool_call(
            "nonexistent.tool",
            &serde_json::json!({}),
            "kernel", "default",
        );
        assert!(!result.success);
    }
}
```

#### F-4b: `kernel/persistence.rs` 单元测试 (~8 tests)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_atomic_write_json_roundtrip() { /* write + read back */ }

    #[test]
    fn test_persist_and_restore_agents() { /* persist → new kernel → restore → compare */ }

    #[test]
    fn test_persist_and_restore_memories() { /* ... */ }

    #[test]
    fn test_persist_and_restore_permissions() { /* ... */ }

    #[test]
    fn test_persist_and_restore_intents() { /* ... */ }

    #[test]
    fn test_atomic_write_json_no_corrupt_on_error() { /* simulate write failure */ }

    #[test]
    fn test_restore_from_empty_dir() { /* fresh dir, no panic */ }

    #[test]
    fn test_build_embedding_provider_stub() { /* EMBEDDING_BACKEND=stub */ }
}
```

#### F-4c: `intent/execution.rs` 单元测试 (~8 tests)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_execute_sync_basic_single_action() { /* ... */ }

    #[test]
    fn test_execute_sync_multi_action() { /* ... */ }

    #[test]
    fn test_execute_sync_below_threshold() { /* low confidence → not executed */ }

    #[test]
    fn test_execute_sync_with_learning() { /* learn=true → memory stored */ }

    #[test]
    fn test_recall_learned_workflow() { /* previously learned → reused */ }

    #[test]
    fn test_execute_actions_sequence_all_ok() { /* ... */ }

    #[test]
    fn test_execute_actions_sequence_partial_failure() { /* ... */ }

    #[test]
    fn test_execute_sync_empty_text() { /* empty input handling */ }
}
```

### F-5: V-06 Remediation — 自动摘要可选化

**当前代码** (`semantic_fs/mod.rs:198-213`):

```rust
// 当前: 有 summarizer 就自动摘要 — 灵魂违规 V-06
if let Some(ref summarizer) = self.summarizer {
    // ... 自动调用 summarizer.summarize()
}
```

**修复方案**: `SemanticFS::create` 增加 `auto_summarize` 参数，默认 `false`。

```rust
pub fn create(
    &self, content: Vec<u8>, tags: Vec<String>,
    agent_id: &str, intent: Option<String>,
    auto_summarize: bool,  // NEW: 调用者决定是否摘要
) -> std::io::Result<String> {
    // ... CAS 存入、索引更新 ...

    // V-06 修复: 只在调用者显式请求时摘要
    if auto_summarize {
        if let Some(ref summarizer) = self.summarizer {
            // ... 摘要逻辑 ...
        }
    }

    Ok(cid)
}
```

**影响传播**:
- `kernel/ops/fs.rs` 的 `semantic_create` 增加 `auto_summarize` 参数
- CLI `cmd_create` 增加 `--summarize` flag
- Tool API `cas.create` 增加 `auto_summarize` schema 字段
- 现有调用点默认 `false`（保守迁移）

### F-6: CLI Command Systematic Audit

**审计清单**:

| 文件 | 命令 | 检查项 | 状态 |
|------|------|--------|------|
| crud.rs | cmd_create | 位置参数 + 空内容 | F-1 修复 |
| crud.rs | cmd_read | 位置参数 CID | ✅ (args.get(1)) |
| crud.rs | cmd_update | --cid + --content | 审计: 缺位置参数 |
| crud.rs | cmd_delete | 位置 + --cid | ✅ (N15) |
| crud.rs | cmd_search | 位置 + --query | ✅ |
| memory.rs | cmd_remember | --content 位置参数 | 审计: 是否支持位置参数 |
| memory.rs | cmd_recall | --query 位置参数 | 审计 |
| agent.rs | cmd_agent | --name | ✅ (N15) |
| skills.rs | cmd_skills_* | --agent resolve | ✅ (N16) |
| messaging.rs | cmd_send | --content | 审计: 位置参数 |
| graph.rs | cmd_add_node | --label | 审计: 位置参数 |
| permission.rs | cmd_permission | --action | ✅ |
| intent.rs | cmd_intent | 位置参数 text | 审计 |
| deleted.rs | cmd_deleted | 无参数 | ✅ |
| session.rs | cmd_session_* | --agent | 审计 |

---

## 4. 前沿研究对标

### 4.1 TDAD — 规格驱动 Agent 测试

| TDAD 组件 | Plico 状态 | Node 17 目标 |
|-----------|-----------|-------------|
| YAML Spec → Tests | ❌ 无 | — (N18+) |
| Visible/Hidden Test Splits | ❌ | — |
| Semantic Mutation Testing | ❌ | — |
| **Behavioral Contracts** | ❌ → ✅ | F-2/F-3 效果合约 |
| Spec Evolution | ❌ | — |

Node 17 实现 TDAD 的**第一步**：行为合约。YAML Spec 编译是 N18+ 的目标。

### 4.2 Trace-Based Assurance — MAT 合约

| Framework 组件 | Plico 状态 | Node 17 目标 |
|---------------|-----------|-------------|
| Message-Action Trace | ⚠️ Event Bus (写入-only) | F-2 审计合约 |
| Step Contracts | ❌ 无 | F-2/F-3 前/后置条件 |
| Deterministic Replay | ❌ | — (N18+) |
| Stress Testing | ❌ | — |
| Contract Violation Localization | ❌ → ⚠️ | `debug_assert!` 在效果合约中 |

### 4.3 NLAH — 驾具合约

| NLAH 组件 | Plico 状态 | Node 17 目标 |
|-----------|-----------|-------------|
| Contracts (pre/post) | ❌ → ✅ | F-2/F-3 |
| Roles | ✅ Agent 注册 | — |
| Adapters (verifiers) | ❌ → ⚠️ | F-2 效果验证 |
| State conventions | ✅ CAS + KG + Memory | — |
| Stages | ⚠️ Intent 路由 | — |

### 4.4 VIGIL — 自愈运行时

| VIGIL 组件 | Plico 状态 | Node 17 目标 |
|------------|-----------|-------------|
| Observation | ✅ Event Bus + Audit | — |
| **Evaluation** | ❌ → ✅ | F-2 后置条件检查 |
| Adaptation | ❌ | — (N18+) |

### 4.5 Rust 2026 Agent Infrastructure

Plico 的 Rust 基座与 2026 行业趋势完全吻合：

> "The bottom two layers — where agents actually execute things — are flipping to Rust. The top layers stay Python." (OSS Insight, 2026)

Plico 作为 Rust AIOS 内核，正位于 "CLI / Runtime Layer" 和 "OS / Sandbox Layer"——Rust 的核心优势区域。934 tests 全部通过、0 ignored 的纪录证明了编译时保证的价值。

---

## 5. 影响分析

### 5.1 B49 影响范围

```
B49: put 位置参数被忽略
  → 任何用 `put "text"` 语法的操作存入空内容
  → Agent 获得有效 CID（SHA-256 of empty）
  → CID 存入 KG / Memory → 形成"虚假记忆"
  → 后续 recall/get 返回空内容
  → Agent 信任链断裂

影响路径:
  CLI (aicli put) → cmd_create → kernel.semantic_create → CAS
  Tool API (cas.create) → builtin_tools → kernel.semantic_create → CAS ← 不受影响（Tool API 显式用 args["content"]）
  MCP (plico_mcp) → API → kernel → CAS ← 不受影响（API 显式传内容）

仅 CLI 路径受影响。Tool API 和 MCP 路径不受影响。
```

### 5.2 F-5 (V-06) 影响传播

```
auto_summarize 参数传播:
  SemanticFS::create(content, tags, agent_id, intent, auto_summarize)
    ← kernel/ops/fs.rs semantic_create()  ← 需要增加参数
      ← API handler                       ← 需要增加 ApiRequest 字段
        ← CLI cmd_create                  ← 需要增加 --summarize flag
        ← builtin_tools cas.create        ← 需要增加 schema 字段
        ← MCP server                      ← 需要增加 tool schema

向后兼容: 所有现有调用默认 auto_summarize=false
```

### 5.3 变更影响矩阵

| 特性 | 修改文件 | 新增行 | 修改行 | 新测试 |
|------|---------|-------|--------|-------|
| F-1 | crud.rs | ~5 | ~3 | 4 |
| F-2 | kernel/ops/fs.rs, semantic_fs/mod.rs | ~20 | ~10 | 6 |
| F-3 | builtin_tools.rs | ~30 | ~15 | 5 |
| F-4a | builtin_tools.rs | ~80 | 0 | 10 |
| F-4b | persistence.rs | ~60 | 0 | 8 |
| F-4c | intent/execution.rs | ~60 | 0 | 8 |
| F-5 | semantic_fs/mod.rs, ops/fs.rs, api, cli | ~15 | ~20 | 3 |
| F-6 | crud.rs, memory.rs, intent.rs | ~10 | ~8 | 4 |
| **合计** | | ~280 | ~56 | **48** |

---

## 6. 量化目标

| 指标 | N16 现状 | N17 目标 | 计算方式 |
|------|---------|---------|---------|
| 总测试数 | 934 | **982+** | +48 new |
| 文件覆盖率 | 72.6% (53/73) | **82%+** (60/73) | +3 P0 + 4 P1 |
| P0 零测试文件 | 3 | **0** | 全部覆盖 |
| 灵魂对齐分 | 93/100 | **95+** | V-06 修复 |
| CLI Bug | 1 (B49) | **0** | F-1 修复 |
| 工具合约覆盖 | 0% | **14%** (5/37) | P0 工具 |

---

## 7. 实施计划

### Phase 1: B49 修复 + CLI 审计（~1 day）

1. 修复 `cmd_create` 支持位置参数
2. 增加空内容拒绝
3. 审计全部 16 个 handler 的参数处理
4. 为 `cmd_create` 添加 4 个测试

### Phase 2: P0 单元测试（~2 days）

1. `builtin_tools.rs` — 10 tests（工具分发 + 效果验证）
2. `persistence.rs` — 8 tests（持久化 roundtrip）
3. `execution.rs` — 8 tests（意图执行 + 学习）

### Phase 3: 效果合约（~1 day）

1. `semantic_create` 空内容前置条件
2. `semantic_create` CID 后置条件 (debug_assert)
3. `cas.create` 工具后置条件
4. `cas.delete` 工具后置条件
5. `memory.store` 工具后置条件

### Phase 4: V-06 修复（~1 day）

1. `SemanticFS::create` 增加 `auto_summarize` 参数
2. 传播到 kernel/API/CLI/Tool/MCP
3. 现有调用默认 `false`
4. 添加 3 个测试

### Phase 5: Dogfood 验证（~0.5 day）

1. 干净环境验证 B49 修复
2. 验证效果合约拒绝空内容
3. 验证 V-06 自动摘要不再默认触发
4. 回归测试全量 934 + 48 新测试

---

## 8. AIOS 路线图对齐

```
Node 1-3:   ████████████████  CAS + 语义 FS + 搜索 = AIOS 存储层
Node 4-6:   ████████████████  Agent + 权限 + 消息 = AIOS 调度层
Node 7-8:   ████████████████  事件 + Delta = AIOS 驾具层
Node 9-10:  ████████████████  弹性 + 整流 = AIOS 韧性层
Node 11-12: ████████████████  4 层记忆 + 合并 = AIOS 记忆层
Node 13:    ████████████████  API v18 + MCP + Intent = AIOS 传导层
Node 14:    ████████████████  融合 = AIOS 集成层
Node 15:    ████████████████  输入安全 = AIOS 防御层(输入)
Node 16:    ██████████████░░  持久化 + 幽灵防御(delete) = AIOS 持续层
Node 17:    ░░░░░░░░░░░░░░░░  效果合约 + 工具合约 = AIOS 诚信层  ← 当前
Node 18+:   ................  MAT 追踪 + 回放 + 自愈 = AIOS 自治层
```

### 与 AIOS 2026 方向校准

| AIOS 2026 趋势 | Plico 对应 | Node 17 贡献 |
|----------------|-----------|-------------|
| Mode 3: Persistent Personal Data | ✅ (N16 `~/.plico`) | — |
| Agent Tool Contracts | ❌ → ✅ | F-2/F-3 |
| Trace-Based Testing (MAT) | ❌ | Foundation (F-2 audit contracts) |
| Self-Healing Runtime (VIGIL) | ❌ | Foundation (Evaluation layer) |
| Rust Runtime Layer | ✅ | Continued |
| NLAH Contracts | ❌ → ⚠️ | F-2/F-3 as proto-contracts |
| TDAD Behavioral Specs | ❌ | Foundation (behavioral asserts) |

---

## 9. 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-5 (auto_summarize) 传播范围大 | 中 | 高 | 默认 false，向后兼容 |
| F-2 效果合约的 debug_assert 在 release 被优化掉 | 高 | 中 | P0 合约用 assert!，非 debug_assert |
| F-4 测试依赖 make_kernel() 的稳定性 | 低 | 中 | make_kernel 已在 50+ tests 中验证 |
| F-1 修复可能破坏 `put --content "text"` 现有语法 | 低 | 高 | 位置参数只在无 --content 时生效 |

---

## 10. 从 Node 17 到 Node 18 的推演

Node 17 完成后，Plico 将具备：
- 输入保真（无幽灵存储）
- 效果合约（关键操作的后置条件）
- P0 文件 100% 测试覆盖
- V-06 灵魂违规修复

**Node 18 展望**: **谱 (Spectrum) — 追踪可回放与自愈基础**

基于 Node 17 的效果合约基础，Node 18 将引入：

1. **MAT (Message-Action Trace) 完整实现**: 将审计日志升级为结构化 MAT 追踪，支持合约绑定和确定性回放
2. **Contract Runtime Monitoring**: 从 debug_assert 升级为运行时合约监控，违反时触发告警而非 panic
3. **Regression Gate**: 集成 AgentAssay 概念的回归测试门控，CI 中自动验证合约
4. **Self-Healing Foundation**: VIGIL 的 Evaluation → Adaptation 循环初步实现——合约违反时自动回退操作

这将使 Plico 从"诚信"走向"自治"——系统不仅诚实报告操作结果，还能在检测到合约违反时主动修复。

---

*文档基于 934 个自动化测试 + 独立 Dogfood 实测 + 60+ 源文件逐行审计 + 5 篇 2026 前沿论文对标。*
*B49 (phantom put) 通过真实 CLI 执行确认。V-06 通过源码审查确认。*
*TDAD / Trace-Based Assurance / AgentAssay / NLAH / VIGIL 均为 2026 年 1-4 月最新研究成果。*
