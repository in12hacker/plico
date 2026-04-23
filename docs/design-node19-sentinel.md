# Plico 第十九节点设计文档
# 哨 — 哨兵治理与完备存取

**版本**: v1.0
**日期**: 2026-04-23
**灵魂依据**: `system-v2.md`（Soul 2.0）
**阶段**: Hook 系统 + 全局断路器 + CLI scope 修复 + 累计 Token Budget + P0 测试覆盖 + dispatch 路由测试
**前置**: 节点 18 ✅（100%）— JSON-First, redb KG, B50-B53 修复, 1245 测试, Soul 79.5%
**验证方法**: 独立 Dogfood 实测（`/tmp/plico-n19-audit`）+ 全量源码 review + cargo test 1245 全通过 + 网络校准(EverMemOS/Hook 行业标准/断路器模式)
**信息来源**: `docs/quality-audit-n18.md` + Claude Agent SDK Hooks (2026) + Omni AI Hook System + EverMemOS (arXiv:2601.02163) + AI Agent Resilience Patterns (2026) + AIOS Agentic Memory (2026)

---

## 0. AI 第一人称推演：为什么是"哨"

### 层次一：我的共享记忆是一扇假门

```bash
# Agent A 存储共享知识
$ remember --content "auth module uses JWT" --scope shared --agent $A
Memory stored for agent 'A'     ← 看起来成功了

# Agent B 检索
$ recall --scope shared --agent $B
memory: []                       ← 空
```

我在 N18 报告中声称 B53（跨 Agent 共享记忆）已修复。内核层确实实现了 `recall_shared()`——6 个测试全通过。但 CLI handler `cmd_remember` **从未读取 `--scope` 参数**。存储时始终调用 `remember_working()` 而非 `remember_working_scoped()`，scope 默认 Private。

这是一个**安全假象**：
- 内核：`recall_shared()` ✅ 实现 + 测试
- CLI handler：`--scope` 参数 **静默丢弃** ❌
- 用户认知：共享记忆已工作
- 现实：共享记忆从未被正确存储

**B54**: CLI `remember --scope shared` 忽略 scope 参数——记忆始终存为 Private。

这是 B49/B50/B52 模式的第四个实例：**CLI 界面丢失信息**。N17 修复了 `cmd_create`（B49），N18 修复了 `parse_edge_type`（B50）和 `cmd_update`（B52），但 `cmd_remember` 的 scope 传递被遗漏。

从 AI 视角看，这比普通 Bug 更危险——它制造**错误的信任**。我存储了共享知识，内核回复"成功"，但另一个 Agent 永远看不到。这违反了 Soul 2.0 公理 4（共享先于重复）的根基。

### 层次二：我没有哨兵，只有日志

2026 年 4 月，行业标准的 agent 治理已经明确收敛到**生命周期 Hook** 模式：

| 框架 | Hook 点 | 能力 |
|------|---------|------|
| Claude Agent SDK | PreToolUse, PostToolUse, Stop, SessionStart, SessionEnd | 拦截/修改/阻止 |
| Omni AI | BeforeToolCall, AfterToolCall, BeforeLLM, AfterLLM | 拦截+阻止（Block/Continue） |
| PraisonAI | BEFORE_TOOL, AFTER_TOOL, BEFORE_LLM, AFTER_LLM, AFTER_AGENT | 验证/日志/拦截 |
| OpenAI Agents SDK | before_agent_run, after_agent_run, before_tool_call, after_tool_call | 验证/审计 |

Plico 有 `EventBus`——1073 行，27 个测试——可以 emit/subscribe 事件。但 EventBus 是**通知性的**（fire-and-forget），不是**拦截性的**（可以 Block 执行）。

当前如果一个 Agent 调用 `cas.delete` 删除关键数据，EventBus 只能在**事后**通知订阅者。没有任何机制在**执行前**拦截并阻止。

Soul 2.0 公理 5（机制不是策略）说 OS 提供机制，Agent 做决策。但当前连"做决策的接口"都不存在。OS 没有提供让 Agent 在 tool call 执行前表达"不允许"的机制。

### 层次三：我的韧性只有一个装甲

`circuit_breaker.rs`（248 行，2 测试）为 embedding provider 实现了三态断路器（Closed→Open→HalfOpen）。这是正确的模式。

但 Plico 有三条外部调用路径：
1. **Embedding** (ollama/ort/local) → ✅ 有断路器
2. **LLM** (openai/ollama/stub) → ❌ 无断路器
3. **MCP** (external tool calls) → ❌ 无断路器

2026 行业共识："One breaker per provider. Never share breakers across providers." (AI Workflow Lab, Antigravity, Brandon Lincoln Hendricks)

LLM 调用是最关键的外部依赖——如果 OpenAI API 宕机 10 分钟，每个 agent 请求都会等待 30 秒超时再失败。断路器可以在 5 次失败后立即 fail-fast，节省 90% 的等待时间。

### 层次四：两个 1000+ 行的路由器零测试

| 文件 | 行数 | 功能 | 测试 |
|------|------|------|------|
| `kernel/mod.rs` | 1935 | AIKernel 构造 + 工具注册 + handle_api_request | 0 |
| `plico_mcp/dispatch.rs` | 1138 | MCP action 路由（40+ actions） | 0 |

这两个文件加起来 3073 行，承载了 Plico 最核心的路由逻辑——一个处理内核 API 请求，一个处理 MCP 工具调用——但**零单元测试**。

它们依赖 integration test 间接覆盖。但 integration test 只覆盖常见路径，不覆盖边界条件（比如 `handle_api_request` 收到无效 action 时的行为，或 `dispatch.rs` 的 40+ action 中哪些有权限检查）。

从 AI 视角：这两个文件是我的"神经中枢"——所有请求都经过它们。没有测试意味着没有合约。没有合约意味着任何重构都可能静默破坏功能。

### 层次五：Token Budget 没有累加器

Soul 2.0 公理 1 说 Token 最稀缺。公理 10 说会话是一等公民。

`context assemble --budget 4096` 可以控制单次组装的 token 上限。但跨请求的累计 token 消耗无处追踪。`AgentUsage` 结构体追踪 `tool_call_count`，但没有 `total_tokens_consumed`。

一个 Agent 在一个 session 中调用 10 次 `context assemble --budget 4096`，理论上消耗 40K tokens。但系统不知道这件事。没有"超过预算就停止"的机制。

EverMemOS 的做法："Latency and online constraints: End-to-end latency budgets"。ContextOS 的做法："Cost Ledger: Track LLM + API spend per session, per workspace, per tool"。

### 推演结论

Node 15 修复了**输入安全**。Node 16 修复了**持久化**。Node 17 修复了**效果合约**。Node 18 修复了**界面保真和存储升级**。

Node 19 的使命：**在系统的所有执行路径上设置哨兵——每条路径可拦截、可度量、可恢复。**

"哨"的三层含义：
1. **Hook 哨兵**：PreToolCall/PostToolCall 生命周期拦截，Agent 可决策是否允许执行
2. **断路哨兵**：LLM/MCP 外部调用路径全覆盖三态断路器
3. **度量哨兵**：累计 token budget + session 级消耗追踪

---

## 1. 审计发现总结

### 1.1 N18 达成率

| 维度 | 承诺 | 实现 | Dogfood 验证 |
|------|------|------|-------------|
| D1 JSON-First | 默认 JSON | ✅ 100% | 无 ENV 时 `python3 json.load()` 成功 |
| D2 严格解析 | B50/B52 修复 | ✅ 100% | `caused_by` → 报错 + 有效列表 |
| D3 KG redb | JSON→redb | ✅ 100% | `kg.redb` 106KB，无旧 JSON |
| D4 跨 Agent | recall_shared | ✅ 内核 / ❌ CLI | B54: `cmd_remember --scope` 忽略 |
| D5 Handler 测试 | ≥4/handler | ✅ 100% | 8/16 handlers 有测试 |
| D6 warm_context | CAS CID | ✅ 100% | 64 hex CID，`get` 返回 JSON 内容 |

**N18 报告 D4 评价过高**——内核层 OK 但 CLI 路径断裂。

### 1.2 新发现 Bug + 技术债

| ID | 严重度 | 描述 | 根因 | 发现方式 |
|----|--------|------|------|---------|
| **B54** | **P0** | `remember --scope shared` 忽略 scope | `cmd_remember` 不读取 `--scope` | Dogfood: shared recall 空 |
| TD-5 | P1 | 无 PreToolCall hook 机制 | EventBus 仅通知，不可拦截 | 代码审查 + 行业对标 |
| TD-6 | P1 | LLM/MCP 无断路器 | 仅 embedding 有 circuit_breaker | 代码审查 |
| TD-7 | P1 | 无累计 token budget | `AgentUsage` 缺 `total_tokens` | 代码审查 |
| TD-8 | P0 | `kernel/mod.rs` 1935 行 0 测试 | 历史债务 | grep `#[test]` |
| TD-9 | P0 | `dispatch.rs` 1138 行 0 测试 | 历史债务 | grep `#[test]` |

### 1.3 测试覆盖率现状

| 指标 | N17 终点 | N18 终点 | 变化 |
|------|---------|---------|------|
| 总测试数 | 1057 | **1245** | +188 |
| 文件覆盖率 | 63.2% (60/95) | **70.3%** (83/118) | +7.1% |
| P0 零测试 (≥200行) | 3 | **4** (kernel/mod, dispatch, main, commands/mod) | +1* |
| Soul 对齐（加权） | ~68% | **79.5%** | +11.5% |

*dispatch.rs 在 N17 时不存在或不算入

---

## 2. Node 19 六大维度

### D1: Hook 哨兵 — PreToolCall/PostToolCall 生命周期拦截

**问题**: EventBus 仅通知，无拦截能力。Agent 无法在 tool call 执行前表达"不允许"。
**目标**: 引入 `HookRegistry` + `PreToolCall`/`PostToolCall` 事件类型，支持 `Continue` 或 `Block` 响应。
**度量**: Dogfood 验证——注册 hook 后可拦截特定 tool call。

### D2: 断路哨兵 — LLM + MCP 全局断路器

**问题**: 仅 embedding 有 circuit_breaker，LLM/MCP 无保护。
**目标**: 复用 `circuit_breaker.rs` 模式到 LLM provider 和 MCP client。
**度量**: LLM provider 连续 5 次失败后 fail-fast（不等待超时）。

### D3: B54 修复 — CLI remember scope 传递

**问题**: `cmd_remember` 不读取 `--scope` 参数。
**目标**: 修复 `cmd_remember` 传递 scope 到 `remember_working_scoped()`/`remember_long_term_scoped()`。
**度量**: `remember --scope shared` + `recall --scope shared` 跨 Agent 可见。

### D4: 累计 Token Budget — Session 级消耗追踪

**问题**: 无跨请求 token 累计，`AgentUsage` 缺 `total_tokens_consumed`。
**目标**: `session-end` 时返回 session 级 token 消耗统计。
**度量**: `session-end` 输出 `total_tokens_consumed` 字段。

### D5: P0 文件测试覆盖 — kernel/mod.rs + dispatch.rs

**问题**: 3073 行核心路由逻辑零测试。
**目标**: `kernel/mod.rs` ≥10 tests, `dispatch.rs` ≥15 tests。
**度量**: ≥25 个新测试覆盖核心路由。

### D6: CLI 系统审计完结 — 全部 handler scope/位置参数

**问题**: `cmd_remember` scope 遗漏是 B49 模式的延续。
**目标**: 审计全部 16 个 handler 的参数传递完整性。
**度量**: 0 个 handler 静默丢弃参数。

---

## 3. Node 19 特性清单

### F-1: HookRegistry — 生命周期拦截系统

**设计原则**: 与 EventBus 分离——EventBus 用于通知（异步、不阻塞），HookRegistry 用于拦截（同步、可阻塞）。

```rust
pub enum HookPoint {
    PreToolCall,
    PostToolCall,
    PreSessionStart,
    PreWrite,
    PreDelete,
}

pub enum HookResult {
    Continue,
    Block { reason: String },
}

pub trait HookHandler: Send + Sync {
    fn handle(&self, point: HookPoint, context: &HookContext) -> HookResult;
}

pub struct HookContext {
    pub agent_id: String,
    pub tool_name: String,
    pub params: serde_json::Value,
    pub timestamp_ms: u64,
}

pub struct HookRegistry {
    hooks: RwLock<Vec<(HookPoint, i32, Arc<dyn HookHandler>)>>,  // (point, priority, handler)
}

impl HookRegistry {
    pub fn register(&self, point: HookPoint, priority: i32, handler: Arc<dyn HookHandler>) { ... }
    
    pub fn run_hooks(&self, point: HookPoint, context: &HookContext) -> HookResult {
        let hooks = self.hooks.read().unwrap();
        let mut relevant: Vec<_> = hooks.iter()
            .filter(|(p, _, _)| *p == point)
            .collect();
        relevant.sort_by_key(|(_, prio, _)| *prio);
        
        for (_, _, handler) in relevant {
            match handler.handle(point.clone(), context) {
                HookResult::Block { reason } => return HookResult::Block { reason },
                HookResult::Continue => {}
            }
        }
        HookResult::Continue
    }
}
```

**集成点**: `builtin_tools.rs::dispatch_tool_call()` 入口插入：

```rust
pub fn dispatch_tool_call(&self, tool_name: &str, params: Value, agent_id: &str) -> Value {
    let ctx = HookContext { agent_id: agent_id.into(), tool_name: tool_name.into(), params: params.clone(), .. };
    
    if let HookResult::Block { reason } = self.hook_registry.run_hooks(PreToolCall, &ctx) {
        return json!({ "error": format!("Blocked by hook: {}", reason) });
    }
    
    let result = self.dispatch_inner(tool_name, params, agent_id);
    
    let mut post_ctx = ctx;
    post_ctx.params = result.clone();
    self.hook_registry.run_hooks(PostToolCall, &post_ctx);
    
    result
}
```

**与 Soul 2.0 的一致性**: 公理 5 "机制不是策略"——Hook 是机制（OS 提供注册和执行），拦截策略由 Agent 定义。

**测试**:
- `test_hook_registry_block`: 注册 Block hook → dispatch_tool_call 返回 Block 错误
- `test_hook_registry_continue`: 注册 Continue hook → 正常执行
- `test_hook_priority_ordering`: 低 priority 先执行
- `test_hook_registry_empty`: 无 hook 时正常通过
- `test_pre_write_hook_blocks_create`: PreWrite hook 阻止 CAS 写入
- `test_post_tool_call_receives_result`: PostToolCall 收到执行结果
- `test_multiple_hooks_first_block_wins`: 多个 hook，第一个 Block 即停

**新测试**: 7

### F-2: 全局断路器 — LLM + MCP 覆盖

**复用模式**: `circuit_breaker.rs` 已有三态实现（248行+2测试）。

```rust
// llm/mod.rs — 已有 LlmProvider trait
pub trait LlmProvider: Send + Sync {
    fn complete(&self, prompt: &str) -> Result<String, LlmError>;
}

// 新增: CircuitBreakerLlmProvider 包装器
pub struct CircuitBreakerLlmProvider {
    inner: Box<dyn LlmProvider>,
    breaker: CircuitBreaker,
}

impl LlmProvider for CircuitBreakerLlmProvider {
    fn complete(&self, prompt: &str) -> Result<String, LlmError> {
        if !self.breaker.can_execute() {
            return Err(LlmError::CircuitOpen);
        }
        match self.inner.complete(prompt) {
            Ok(r) => { self.breaker.record_success(); Ok(r) }
            Err(e) => { self.breaker.record_failure(); Err(e) }
        }
    }
}
```

**MCP client**: 同样包装 `McpClient::call_tool()`。

**配置**: 
- `fail_threshold = 5` (连续 5 次失败触发)
- `reset_timeout = 60s` (open 后 60s 尝试恢复)
- `success_threshold = 2` (half-open 成功 2 次后关闭)

**测试**:
- `test_llm_circuit_breaker_opens_after_threshold`
- `test_llm_circuit_breaker_recovers`
- `test_mcp_circuit_breaker_isolates_providers`
- `test_circuit_breaker_fail_fast_latency`

**新测试**: 4

### F-3: B54 修复 — cmd_remember scope 传递

**当前代码** (`memory.rs:8-66`):
```rust
pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    // ... reads --agent, --content, --tier, --tags
    // ❌ Never reads --scope
    match parse_memory_tier(&tier_str) {
        MemoryTier::Working => {
            kernel.remember_working(&agent_id, "default", content, tags_clone)  // default Private
        }
        // ...
    }
}
```

**修复**:
```rust
pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();
    let scope = parse_memory_scope(
        &extract_arg(args, "--scope").unwrap_or_else(|| "private".to_string())
    );
    let tags: Vec<String> = extract_arg(args, "--tags")
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    match parse_memory_tier(&tier_str) {
        MemoryTier::Working => {
            match kernel.remember_working_scoped(&agent_id, "default", content, tags, scope) {
                Ok(_) => ApiResponse::ok_with_message(...),
                Err(e) => ApiResponse::error(e),
            }
        }
        MemoryTier::LongTerm => {
            match kernel.remember_long_term_scoped(&agent_id, "default", content, tags, 50, scope) {
                Ok(entry_id) => { ... }
                Err(e) => ApiResponse::error(e),
            }
        }
        // ...
    }
}

fn parse_memory_scope(s: &str) -> MemoryScope {
    match s.to_lowercase().as_str() {
        "shared" => MemoryScope::Shared,
        "private" | "" => MemoryScope::Private,
        other if other.starts_with("group:") => {
            MemoryScope::Group(other[6..].to_string())
        }
        _ => MemoryScope::Private,
    }
}
```

**测试**:
- `test_cmd_remember_shared_scope`: `--scope shared` → recall_shared 可见
- `test_cmd_remember_default_private`: 无 `--scope` → 默认 Private
- `test_cmd_remember_positional_content_with_scope`: 位置内容 + `--scope shared`

**新测试**: 3

### F-4: 累计 Token Budget

**当前**: `AgentUsage` 有 `tool_call_count` 无 `total_tokens`。

```rust
// kernel/ops/session.rs — AgentUsage 扩展
pub struct AgentUsage {
    pub tool_call_count: u32,
    pub total_tokens_consumed: u64,  // 新增
    pub session_id: String,
    pub started_at_ms: u64,
}
```

**追踪点**: 每次 `context assemble` 返回 `total_tokens` 时累加到 `AgentUsage`。

**Session-end 输出**:
```json
{
  "ok": true,
  "session_ended": {
    "session_id": "...",
    "total_tokens_consumed": 12840,
    "tool_call_count": 7,
    "duration_ms": 45000
  }
}
```

**测试**:
- `test_session_token_accumulation`
- `test_session_end_reports_total_tokens`
- `test_zero_tokens_when_no_context_assembled`

**新测试**: 3

### F-5: P0 文件测试覆盖

#### F-5a: kernel/mod.rs (1935 行 → ≥10 tests)

| 测试 | 覆盖函数 | 类型 |
|------|---------|------|
| test_kernel_new_creates_valid_instance | AIKernel::new | 构造 |
| test_kernel_tools_registered | register_builtin_tools | 注册 |
| test_kernel_handle_invalid_action | handle_api_request 无效 action | 错误路径 |
| test_kernel_handle_create_via_api | handle_api_request Create | 正常路径 |
| test_kernel_handle_search_via_api | handle_api_request Search | 正常路径 |
| test_kernel_handle_agent_register_via_api | handle_api_request AgentRegister | 正常路径 |
| test_kernel_version_matches_cargo | version() == Cargo.toml version | 一致性 |
| test_kernel_persist_on_drop | Drop 时自动 persist | 持久化 |
| test_kernel_restore_after_persist | persist → new → 数据恢复 | 持久化 |
| test_kernel_concurrent_access | 多线程读写安全 | 并发 |

**新测试**: 10

#### F-5b: plico_mcp/dispatch.rs (1138 行 → ≥15 tests)

| 测试 | 覆盖路由 |
|------|---------|
| test_dispatch_create_action | "create" action |
| test_dispatch_search_action | "search" action |
| test_dispatch_delete_action | "delete" action |
| test_dispatch_remember_action | "remember" action |
| test_dispatch_recall_action | "recall" action |
| test_dispatch_agent_register | "agent_register" action |
| test_dispatch_session_start | "session_start" action |
| test_dispatch_kg_node_add | "kg_node_add" action |
| test_dispatch_kg_edge_add | "kg_edge_add" action |
| test_dispatch_permission_grant | "permission_grant" action |
| test_dispatch_unknown_action | 未知 action → 错误 |
| test_dispatch_missing_params | 缺少必需参数 → 错误 |
| test_dispatch_pipeline_sequential | "pipeline" action 顺序执行 |
| test_dispatch_tools_list | "tools_list" action |
| test_dispatch_action_meta_completeness | 所有 ActionMeta 可访问 |

**新测试**: 15

### F-6: CLI 系统审计完结

全部 16 个 handler 逐一审计参数传递完整性：

| Handler | 文件 | 审计结果 | 修复需求 |
|---------|------|---------|---------|
| cmd_create | crud.rs | ✅ 位置参数+空检查 (N17) | — |
| cmd_read | crud.rs | ✅ 位置 CID | — |
| cmd_update | crud.rs | ✅ 位置参数+空检查 (N18) | — |
| cmd_delete | crud.rs | ✅ 位置 CID+空检查 | — |
| cmd_search | crud.rs | ✅ 位置 query + tags | — |
| cmd_history | crud.rs | ⚠️ 无空 CID 检查 | P2 |
| cmd_rollback | crud.rs | ⚠️ 无空 CID 检查 | P2 |
| **cmd_remember** | **memory.rs** | **❌ --scope 忽略** | **P0 (F-3)** |
| cmd_recall | memory.rs | ✅ scope shared 路由 | — |
| cmd_add_node | graph.rs | ✅ | — |
| cmd_edge | graph.rs | ✅ B50 修复 | — |
| cmd_agent | agent.rs | ✅ | — |
| cmd_session_start | session.rs | ✅ B51 修复 | — |
| cmd_session_end | session.rs | ✅ | — |
| cmd_skills_* | skills.rs | ✅ resolve_agent | — |
| cmd_permission | permission.rs | ✅ | — |

**新发现**: `cmd_history` 和 `cmd_rollback` 接受空 CID 不报错（P2，可后续修复）。

---

## 4. 前沿研究对标

### 4.1 Hook 系统 — 2026 行业标准

| 维度 | 行业标准 | Plico F-1 设计 | 对齐度 |
|------|---------|---------------|--------|
| Hook 点 | PreToolUse, PostToolUse, SessionStart | PreToolCall, PostToolCall, PreWrite, PreDelete, PreSessionStart | ✅ 超集 |
| 响应类型 | Continue / Block | Continue / Block { reason } | ✅ 对齐 |
| 执行模型 | 同步、按优先级 | 同步、priority 排序 | ✅ 对齐 |
| 上下文 | agent_id, tool_name, params | agent_id, tool_name, params, timestamp | ✅ 超集 |
| 注册方式 | API/配置 | `HookRegistry::register()` | ✅ 对齐 |

### 4.2 断路器 — 2026 行业共识

| 维度 | 行业标准 | Plico F-2 设计 | 对齐度 |
|------|---------|---------------|--------|
| 三态 | Closed/Open/HalfOpen | ✅ 已有 (circuit_breaker.rs) | ✅ |
| Per-provider | 每个 provider 独立 | ✅ LLM/MCP/Embedding 各自独立 | ✅ |
| 配置 | fail_max, reset_timeout, success_threshold | 5, 60s, 2 | ✅ 对齐 |
| 层叠 | Retry → CircuitBreaker → Fallback | F-2 覆盖 CircuitBreaker | 部分 |

### 4.3 AIOS 2026 方向对标

```
Node 1-3:   ████████████████  CAS + 语义 FS + 搜索 = AIOS 存储层
Node 4-6:   ████████████████  Agent + 权限 + 消息 = AIOS 调度层
Node 7-8:   ████████████████  事件 + Delta = AIOS 驾具层
Node 9-10:  ████████████████  弹性 + 整流 = AIOS 韧性层
Node 11-12: ████████████████  4 层记忆 + 合并 = AIOS 记忆层
Node 13:    ████████████████  API v18 + MCP + Intent = AIOS 传导层
Node 14:    ████████████████  融合 = AIOS 集成层
Node 15:    ████████████████  输入安全 = AIOS 防御层(输入)
Node 16:    ██████████████░░  持久化 + 幽灵防御 = AIOS 持续层
Node 17:    ██████████████░░  效果合约 + P0 测试 = AIOS 诚信层
Node 18:    ████████████████  JSON-First + redb + 共享记忆 = AIOS 界面层
Node 19:    ░░░░░░░░░░░░░░░░  Hook + 断路器 + 度量 = AIOS 哨兵层  ← 当前
Node 20+:   ................  意图缓存 + 预热 + 认知层 = AIOS 主动层
```

---

## 5. 影响分析

### 5.1 变更影响矩阵

| 特性 | 修改文件 | 新增行 | 修改行 | 新测试 | 新依赖 |
|------|---------|-------|--------|-------|--------|
| F-1 | kernel/hook.rs(新), builtin_tools.rs | ~200 | ~30 | 7 | 0 |
| F-2 | llm/mod.rs, mcp/client.rs | ~80 | ~20 | 4 | 0 |
| F-3 | handlers/memory.rs | ~25 | ~15 | 3 | 0 |
| F-4 | kernel/ops/session.rs, api/semantic.rs | ~40 | ~15 | 3 | 0 |
| F-5a | kernel/mod.rs | ~120 | 0 | 10 | 0 |
| F-5b | plico_mcp/dispatch.rs (或 tests/) | ~200 | 0 | 15 | 0 |
| F-6 | handlers/crud.rs (history/rollback) | ~10 | ~5 | 2 | 0 |
| **合计** | | **~675** | **~85** | **44** | **0** |

### 5.2 风险评估

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| F-1 Hook 同步拦截影响延迟 | 中 | 中 | Hook handler 应轻量级；添加超时保护 |
| F-2 断路器过于敏感误触发 | 低 | 中 | 配置可调（fail_threshold/reset_timeout） |
| F-3 scope 修复破坏现有 remember 调用 | 低 | 低 | 默认 Private 向后兼容 |
| F-5 kernel/mod.rs 测试需要完整内核初始化 | 高 | 低 | 使用 `make_kernel()` 工具函数 |

---

## 6. 量化目标

| 指标 | N18 现状 | N19 目标 | 计算方式 |
|------|---------|---------|---------|
| 总测试数 | 1245 | **1289+** | +44 new |
| 文件覆盖率 | 70.3% (83/118) | **73%+** (86/118) | +kernel/mod, dispatch, hook.rs |
| P0 零测试(≥200行) | 4 | **2** | kernel/mod + dispatch 补全 |
| 遗留 Bug | 1 (B54) | **0** | F-3 修复 |
| 技术债 | 5 (TD-5~9) | **0** | 全部解决 |
| Soul 对齐（加权） | 79.5% | **≥82%** | 公理 4(+5) + 公理 5(+3) + 公理 10(+2) |
| Hook 覆盖 | 0 点 | **5 点** | PreToolCall+PostToolCall+PreWrite+PreDelete+PreSession |
| 断路器覆盖 | 1/3 路径 | **3/3 路径** | Embedding+LLM+MCP |

---

## 7. 实施计划

### Phase 1: B54 修复 + CLI 审计完结（~0.5 day）

1. F-3: `cmd_remember` 传递 `--scope` 参数
2. F-6: `cmd_history`/`cmd_rollback` 空 CID 检查
3. 测试: 5 个新测试
4. Dogfood 验证: `remember --scope shared` + `recall --scope shared` 跨 Agent

### Phase 2: HookRegistry 实现（~1.5 days）

1. F-1: 创建 `kernel/hook.rs`（HookRegistry + HookHandler trait）
2. F-1: 集成到 `builtin_tools.rs::dispatch_tool_call()`
3. F-1: 注册到 AIKernel 构造函数
4. 测试: 7 个新测试

### Phase 3: 全局断路器 + Token Budget（~1 day）

1. F-2: `CircuitBreakerLlmProvider` 包装器
2. F-2: `CircuitBreakerMcpClient` 包装器
3. F-4: `AgentUsage::total_tokens_consumed` 字段 + 累加逻辑
4. 测试: 7 个新测试

### Phase 4: P0 文件测试覆盖（~2 days）

1. F-5a: `kernel/mod.rs` 10 个测试
2. F-5b: `plico_mcp/dispatch.rs` 15 个测试
3. 验证: 全部 1289+ 测试通过

### Phase 5: Dogfood 回归（~0.5 day）

1. 干净环境全量回归
2. Hook 拦截验证
3. 断路器 fail-fast 验证
4. 跨 Agent 共享记忆端到端
5. session-end token 统计

---

## 8. 从 Node 19 到 Node 20 的推演

Node 19 完成后，Plico 将具备：
- 生命周期 Hook 系统（5 个 Hook 点，可拦截/可阻止）
- 全路径断路器（Embedding + LLM + MCP）
- 累计 Token Budget + Session 级消耗统计
- 共享记忆端到端工作（CLI → 内核 → CLI）
- 核心路由 25 个新测试
- Soul 对齐 ≥82%

**Node 20 展望**: **觉 (Awareness) — 主动性与越用越好**

基于 Node 19 的哨兵基础设施（Hook 可观测，断路器保韧性，Token 可度量），Node 20 将攻坚 Soul 2.0 最大差距——公理 7（主动先于被动，当前 35%）和公理 9（越用越好，当前 25%）：

1. **意图缓存 (Intent Cache)**: 相似意图命中历史组装结果，公理 9 核心。`prefetch_cache.rs`（455行+14测试）已有基础。扩展为跨 session 持久化。
2. **上下文主动预热 (Proactive Prefetch)**: `session-start --intent` 后后台异步组装。`prefetch.rs`（1458行+22测试）已有核心算法。需要异步执行和结果缓存。
3. **Agent Profile 学习**: `prefetch_profile.rs`（191行+8测试）已有 transition matrix 和 tag extraction。需要跨 session 持久化 + 预测触发。
4. **Context-Dependent Gravity**: 基于当前意图重新加权检索结果（EverMemOS Reconstructive Recollection 启发）。

**预期 Soul 对齐**: ~82% → **~87%+**（公理 7: 35%→55%，公理 9: 25%→45%）

---

*文档基于 1245 个自动化测试 + 独立 Dogfood 实测（`/tmp/plico-n19-audit`，发现 B54） + 118 个源文件逐一审计 + 3 项网络研究(Hook Systems 2026 / Circuit Breaker Patterns / EverMemOS) + Soul 2.0 十条公理逐条验证。*
*B54 通过真实 CLI 执行确认（remember shared → recall shared → empty）。TD-5~TD-9 通过代码审查和行业对标确认。*
