# Plico 灵魂对齐审计报告

**日期**: 2026-04-19  
**审计范围**: src/ 全部 82 个 Rust 源文件  
**审计标准**: system.md（灵魂）+ v2 设计报告（Agent Namespace 原则）  

---

## 审计原则

根据 v2 设计报告确立的核心原则：
1. **OS 提供资源，不提供执行逻辑** — 内核不定义 Agent 怎么思考
2. **Agent 隔离** — 记忆/工具/存储有命名空间隔离
3. **协议在接口层** — MCP/A2A 不渗入内核
4. **一切皆工具** — 统一的工具抽象
5. **模型无关** — 不绑定特定 LLM

---

## 对齐状态总览

| 编号 | 模块 | 状态 | 严重度 |
|------|------|------|--------|
| V-01 | `intent/` — 内核包含 NL 解析引擎 | **违背** | P1-高 |
| V-02 | `kernel/ops/intent.rs` — `intent_execute_sync` 是执行引擎 | **违背** | P1-高 |
| V-03 | `memory/` — 无 MemoryScope（私有/共享） | **缺失** | P2-中 |
| V-04 | `kernel/ops/dispatch.rs` — 自动学习硬编码在内核 | **违背** | P2-中 |
| V-05 | `scheduler/dispatch.rs` — `AgentExecutor` 是 IntentExecutor 变体 | **需讨论** | P3-低 |
| A-01 | `tool/` — ExternalToolProvider 协议无关 | **对齐** | — |
| A-02 | `bin/plico_mcp.rs` — MCP 在接口层 | **对齐** | — |
| A-03 | 内核零 MCP 引用 | **对齐** | — |
| A-04 | `cas/` — CAS 有 `created_by` ownership | **对齐** | — |
| A-05 | `kernel/ops/fs.rs` — 搜索有 ownership 过滤 | **对齐** | — |
| A-06 | `api/permission.rs` — ReadAny 权限模型 | **对齐** | — |
| A-07 | `scheduler/agent.rs` — AgentResources 有配额 | **对齐** | — |
| A-08 | `memory/layered/` — store_checked 强制配额 | **对齐** | — |
| A-09 | `scheduler/dispatch.rs` — cpu_time_quota 强制执行 | **对齐** | — |
| A-10 | `llm/` — LlmProvider trait 模型无关 | **对齐** | — |
| A-11 | `fs/search/` — SemanticSearch trait 后端无关 | **对齐** | — |
| A-12 | `builtin_tools.rs` — 一切皆工具 | **对齐** | — |

---

## 详细违背分析

### V-01: IntentRouter 是应用层逻辑，不应在内核 [P1-高]

**位置**: `src/intent/` (180 + 648 + 186 = 1014 行)

**灵魂说**: "交互范式：从'命令'到'提示'：Agent 通过结构化提示词与系统交互"

**问题**: `IntentRouter` 将自然语言解析为 `ApiRequest`。这是**应用层行为**，不是内核行为。

**链式推理**:
```
灵魂说"自然语言是主要接口"
  → 但谁来理解自然语言？
    → v1 思路：内核理解 → IntentRouter 在内核
    → v2 修正：Agent 自己理解 → IntentRouter 应在 Agent 侧
      → OpenClaw 自带 NL 理解（Gateway + Agent 层）
      → Claude Code 自带 NL 理解（QueryEngine）
      → 内核的 IntentRouter = OS 替 Agent 思考 = 违背原则
```

**类比**: Unix 内核不包含 HTTP 请求解析器。HTTP 解析在 Apache/nginx 应用层。同样，NL→API 解析应在 Agent 应用层。

**当前代码路径**:
```
用户 NL 输入
  → kernel.intent_execute_sync()          ← 内核在"替 Agent 思考"
    → ChainRouter → HeuristicRouter       ← 648 行 pattern matching
    → 或 LlmRouter → 调 Ollama           ← 内核调用 LLM 理解输入
    → 生成 ApiRequest
    → kernel.handle_api_request()
    → 自动学习 → remember_procedural()
```

**修正方向**: `IntentRouter` 移出内核，成为接口层组件（如 `aicli` 的 NL 前端），或作为用户空间库。内核只接受结构化 `ApiRequest`。

**影响文件**:
- `src/intent/mod.rs` — 180 行
- `src/intent/heuristic.rs` — 648 行
- `src/intent/llm.rs` — 186 行
- `src/kernel/mod.rs` — `intent_router` 字段
- `src/kernel/ops/intent.rs` — 263 行

---

### V-02: `intent_execute_sync` 是执行引擎 [P1-高]

**位置**: `src/kernel/ops/intent.rs:89-217`

**问题**: `intent_execute_sync` 是一个完整的 ReAct 变体：
1. 检查程序记忆（reuse loop）
2. NL 解析为 action
3. 执行 action sequence
4. 记录结果到 working memory
5. 如果成功，自动学习为 procedural memory

**这是应用逻辑，不是 OS 原语。** 它定义了 Agent 的"思考-执行-学习"循环，而 v2 原则明确指出：Agent 自带执行模型。

**链式推理**:
```
intent_execute_sync 做了什么？
  → 理解输入（IntentRouter）        = Agent 应用逻辑
  → 检查历史（recall_learned_workflow）= Agent 应用逻辑
  → 执行序列（execute_actions_sequence）= 可以保留（是调度）
  → 自动学习（remember_procedural）   = Agent 应用逻辑
  
结论：4 步中 3 步是应用逻辑
```

**修正方向**: 将 `intent_execute_sync` 移到接口层（作为 aicli 或某个 "smart agent" 的能力），内核保留纯粹的 `handle_api_request`。

---

### V-03: Memory 缺少 MemoryScope [P2-中]

**位置**: `src/memory/layered/mod.rs`

**现状**:
- 记忆以 `agent_id` 键存储 ✅
- `get_all(agent_id)` 只返回该 Agent 的记忆 ✅
- 但**没有共享记忆**的概念 ❌
- Agent A 存的东西 Agent B 完全看不到 ❌

**灵魂说**: "程序记忆(Procedural Memory)：可调用的、经过学习的工作流和技能"

**v2 设计需要**:
```rust
pub enum MemoryScope {
    Private(AgentId),    // 只有自己能读写
    Shared,              // 所有 Agent 可读
    Group(String),       // 组内可见
}
```

**当前差距**: 所有记忆都是 Private。没有 Agent 间知识共享。

**注意**: CAS 层已有 `ReadAny` 权限和 ownership 过滤。差距仅在 Memory 层——Agent A 的程序记忆对 Agent B 不可发现。

**修正方向**: 在 `MemoryEntry` 中添加 `scope: MemoryScope` 字段，默认 `Private`。`recall_procedural` 支持 `scope=Shared` 查询，返回所有 Agent 的共享程序记忆。

---

### V-04: 自动学习硬编码在内核 [P2-中]

**位置**: 
- `src/kernel/ops/intent.rs:183-209` — `intent_execute_sync` 自动存 procedural memory
- `src/kernel/ops/dispatch.rs:58-91` — `start_result_consumer` 自动存 working memory

**问题**: 内核强制在每次执行后自动学习（存记忆）。这是**策略**，不是**机制**。

**类比**: Unix 内核不会在每次 `write()` 后自动备份到另一个文件。备份是应用层策略。

**链式推理**:
```
灵魂说"记忆系统"要有
  → 但"什么时候存记忆"是策略问题
    → OpenClaw 自己决定什么值得记住
    → Claude Code 自己决定什么存入 Dream
    → 内核强制存 = OS 替 Agent 决定记忆策略 = 违背
```

**修正方向**: 将自动学习从内核移到接口层。内核只提供 `remember_*` 原语，由 Agent 或 Smart CLI 决定何时调用。

---

### V-05: DispatchLoop 中的 AgentExecutor [P3-低/需讨论]

**位置**: `src/scheduler/dispatch.rs`

**现状**: `AgentExecutor` trait 定义 Agent 如何执行 intent：
```rust
pub trait AgentExecutor: Send + Sync {
    async fn execute(&self, intent: &Intent, agent: &Agent) -> ExecutionResult;
}
```

**分析**: 这与 v1 的 `IntentExecutor` 类似，但有关键区别：
- `AgentExecutor` 只管"拿到 action JSON，执行它"——这是**调度**，不是**思考**
- `KernelExecutor` 反序列化 JSON 为 `ApiRequest` 并调用 `handle_api_request`——这是**纯分发**
- 它不做 NL 理解、不做学习——只是一个 `execute(action) → result` 分发器

**结论**: 这更像 Unix 的 `execve()` 系统调用（执行预定义的程序），而非"替进程思考"。**基本对齐**，但命名容易误导。建议重命名为 `ActionDispatcher`。

---

## 对齐项详解（亮点）

### A-01 ~ A-03: MCP 完全正确地在接口层

```
src/kernel/ 中搜索 "mcp|MCP|jsonrpc|stdio":
  仅一处：ops/tools_external.rs:3 注释说 "The kernel doesn't know about MCP"

src/bin/plico_mcp.rs:
  纯粹的 JSON-RPC 适配器，调用 kernel.handle_api_request()
  零内核逻辑泄漏
```

**这是灵魂对齐的典范。** MCP 完全在 `bin/`（接口层），内核零感知。

### A-04 ~ A-06: CAS 所有权隔离已实现

```rust
// fs.rs:31 — 读取时检查 ownership
self.permissions.check_ownership(agent_id, &obj.meta.created_by)?;

// fs.rs:76-91 — 搜索时过滤
let can_read_any = self.permissions.can_read_any(agent_id);
if !can_read_any {
    results.retain(|r| r.meta.created_by == agent_id);
}
```

**CAS 存储已有 Agent 级隔离 + ReadAny 跨 Agent 访问控制。** 这与 v2 设计的 AgentNamespace 理念一致。

### A-07 ~ A-09: 资源配额已实现并强制执行

```rust
// AgentResources: memory_quota, cpu_time_quota, allowed_tools
// memory: store_checked() 检查 quota
// dispatch: cpu_time_quota 作为 timeout 强制执行
// builtin_tools: allowed_tools 白名单检查
```

**v2 设计的 ResourceBudget 概念已部分实现。** 缺少 KG 节点配额和搜索频率限制。

---

## 违背严重度评估

### P1-高（违背核心原则）

| ID | 描述 | 影响范围 | 修正成本 |
|----|------|---------|---------|
| V-01 | IntentRouter 在内核 | 1014 行代码 + 内核耦合 | 高（需重构移出） |
| V-02 | intent_execute_sync 是执行引擎 | 263 行 + 学习逻辑 | 中（移到接口层） |

### P2-中（缺失关键能力）

| ID | 描述 | 影响范围 | 修正成本 |
|----|------|---------|---------|
| V-03 | Memory 无共享范围 | 影响跨 Agent 知识共享 | 低（添加 scope 字段） |
| V-04 | 自动学习硬编码 | dispatch + intent 两处 | 低（移到调用者） |

### P3-低（命名/风格）

| ID | 描述 | 影响范围 | 修正成本 |
|----|------|---------|---------|
| V-05 | AgentExecutor 命名误导 | 仅命名问题 | 极低 |

---

## 修正优先级建议

```
最高优先级（修正方向偏差）:
  V-01 + V-02: 将 intent/ 从内核层移到接口层
    → IntentRouter 变为 aicli 的 NL 前端
    → intent_execute_sync 变为 CLI 的 smart 模式
    → 内核只保留 handle_api_request (纯结构化 API)
    
中优先级（补全 v2 设计）:
  V-03: MemoryScope 添加共享/私有/组概念
    → MemoryEntry 增加 scope 字段
    → recall API 支持 scope 过滤
    
低优先级（清理）:
  V-04: 自动学习从内核移到接口
  V-05: AgentExecutor → ActionDispatcher 重命名
```

---

## 灵魂对齐得分

| 灵魂组件 | 得分 | 说明 |
|----------|------|------|
| AI原生文件系统（CAS + 语义搜索 + KG） | **95/100** | 完整且优雅，ownership 隔离到位 |
| 一切皆工具 | **95/100** | ToolRegistry + ExternalToolProvider 范式正确 |
| 模型无关 | **90/100** | LlmProvider/SemanticSearch 全 trait 化 |
| Agent 调度器 | **85/100** | 状态机 + 资源配额 + dispatch 循环完整 |
| 权限与安全 | **85/100** | ReadAny + scope + allowed_tools |
| 分层内存 | **80/100** | 4 层完整但缺少共享范围 |
| 协议分层 | **90/100** | MCP 正确在接口层，内核零协议感知 |
| **内核纯净度** | **70/100** | IntentRouter + 自动学习 泄漏应用逻辑 |

**总体**: 86/100 — 架构根基正确，有两个方向性偏差需要修正。

---

*审计结论：Plico 的存储层（CAS）、工具层（ToolRegistry）、协议分层（MCP at interface）完全对齐灵魂。主要违背在 intent 模块——内核不应理解自然语言或自动学习，这些是 Agent 应用层的责任。修正后灵魂对齐度可达 95+。*

---

## 2026-04-19 更新：第二节点完成后的状态

### 已修复的违背

| ID | 描述 | 修复版本 | 状态 |
|----|------|---------|------|
| V-01 | IntentRouter 在内核 | v3.0-M1 ✅ | **已修复** — IntentRouter 迁至 `src/intent/`，内核零感知 |
| V-02 | intent_execute_sync 是执行引擎 | v3.0-M1 ✅ | **已修复** — 执行引擎移至接口层 |
| V-04 | 自动学习硬编码在内核 | v3.0-M3 ✅ | **已修复** — 自动学习从内核移至接口层 |

### 当前灵魂对齐得分（更新后）

| 灵魂组件 | 得分 | 说明 |
|----------|------|------|
| AI原生文件系统（CAS + 语义搜索 + KG） | **95/100** | 完整且优雅，ownership 隔离到位 |
| 一切皆工具 | **95/100** | ToolRegistry + ExternalToolProvider 范式正确 |
| 模型无关 | **90/100** | LlmProvider/SemanticSearch 全 trait 化 |
| Agent 调度器 | **85/100** | 状态机 + 资源配额 + dispatch 循环完整 |
| 权限与安全 | **85/100** | ReadAny + scope + allowed_tools |
| 分层内存 | **95/100** | 4 层完整 + MemoryScope 共享机制 |
| 协议分层 | **90/100** | MCP 正确在接口层，内核零协议感知 |
| **内核纯净度** | **95/100** | IntentRouter + 自动学习已迁出内核 |

**总体**: **94/100** — 架构根基正确，所有 P1/P2 违背已修正。

---

## 2026-04-22 v2 更新：新发现违规

### V-06: create() 自动 LLM 摘要 [P2-中]

**位置**: `src/fs/semantic_fs/mod.rs:198-213`

**问题**: `SemanticFS::create()` 在存储内容时自动调用 `summarizer.summarize()` 生成 L0 摘要。这是策略决定（何时摘要、摘要级别），不是机制。等同于 V-04 的"OS 替 Agent 决定"模式。

**缓解因素**: `summarizer` 是 `Option`，LLM 不可用时跳过。但"有 LLM 就自动摘要"仍属内核策略。

**修正方向**: 将自动摘要改为 API 参数（`auto_summarize: bool`），默认 `false`，由调用者决定。

### V-07: create() 自动 KG 关联 [P3-低]

**位置**: `src/fs/semantic_fs/mod.rs:189-196`

**问题**: 存储内容时自动 upsert KG Document 节点 + SimilarTo 边。类似 inode 创建（机制），但 SimilarTo 边的阈值判断属策略。

### 更新后灵魂对齐得分

| 灵魂组件 | 得分 | 说明 |
|----------|------|------|
| AI原生文件系统 | **93/100** | V-06 自动摘要 -2 |
| 一切皆工具 | **95/100** | 不变 |
| 模型无关 | **90/100** | 不变 |
| Agent 调度器 | **85/100** | 不变 |
| 权限与安全 | **85/100** | 不变 |
| 分层内存 | **95/100** | 不变 |
| 协议分层 | **90/100** | 不变 |
| **内核纯净度** | **93/100** | V-06/V-07 轻微策略泄漏 |

**总体**: **93/100** — V-06/V-07 为新发现的边界案例，修正后可达 95+。
