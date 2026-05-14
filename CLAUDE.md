# CLAUDE.md — 太初 (Plico) AI-OS

本文件为 Claude Code 提供项目级指引。

## 项目概述

**太初 (Plico)** 是一个 AI 原生操作系统内核，专为 AI Agent 设计——无人类 CLI/GUI，无人类文件系统路径。所有数据管理通过 AI 友好的语义 API 完成。系统与模型无关，不依赖任何特定 AI 或 Agent。"太初"意为"Genesis / In the Beginning"——AI-OS 自我觉醒的原初状态。

设计文档：`system-v3.md`（Soul 3.0，中文）
完整参考：`docs/genesis-reference.md`

## 架构要点

```
Application Layer (AI Agent Ecosystem)
        ↓
AI-Friendly Interface Layer (Semantic API/CLI, Natural Language Interface)
        ↓
AI Kernel Layer
  ├─ Cognitive Loop       — Soul v3.0: 主动上下文优化、技能进化、意图语义网络
  ├─ Agent Scheduler      — 生命周期管理 (create/pause/resume/destroy)
  ├─ Layered Memory       — Ephemeral → Working → Long-term → Procedural
  ├─ Model & Tool Runtime — 加载/运行/卸载模型；外部工具作为"技能"
  └─ Permission & Safety Guardrails
        ↓
AI-Native File System
  ├─ Content-Addressed Storage (CAS) — SHA-256 内容寻址，自动去重
  ├─ Semantic Vector Index           — 每个文件的 embedding 语义搜索
  ├─ Knowledge Graph                 — 自动关联文件为知识网络
  └─ Layered Context Loading         — L0 (~100 tokens), L1 (~2k tokens), L2 (full)
```

核心哲学：管理单元 = agents/intents（非 processes/files）；存储寻址 = content hashes + semantic tags（非 filesystem paths）；索引 = vectors + knowledge graphs（非 filenames）。

## 构建与测试

```bash
# 构建
cargo build

# 运行测试（stub 后端，无外部依赖）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# 仅 lib 测试（最快，~2s）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 覆盖率测量
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# Clippy（零警告要求）
cargo clippy -- -D warnings

# 构建 release
cargo build --release

# 运行 CLI（embedded 模式 — 直接 kernel，无 daemon）
cargo run --bin aicli -- --embedded put --content "test" --tags "test"

# 运行 CLI（daemon 模式 — 默认，需要运行中的 plicod）
cargo run --bin aicli -- put --content "test" --tags "test"

# 运行 daemon（start/stop/status 生命周期）
cargo run --bin plicod -- start --port 7878
cargo run --bin plicod -- stop
cargo run --bin plicod -- status
```

## 推断后端配置

Embedding 和 LLM 后端**与推断框架无关**。任何暴露 OpenAI-compatible `/v1/embeddings` 或 `/v1/chat/completions` 端点的服务器均可（llama.cpp, vLLM, SGLang, TensorRT-LLM, Ollama, OpenAI 等）。

**默认配置（自动检测 llama-server 端口，回退 :8080）：**
- `LLM_BACKEND=llama` → 自动检测 llama-server URL
- `EMBEDDING_BACKEND=openai` → 同上
- Model: `qwen2.5-coder-7b-instruct`（通过 `LLAMA_MODEL` 覆盖）

URL 解析优先级：`LLAMA_URL` env > `OPENAI_API_BASE` env > `~/.plico/llama.url` 文件 > `ps` 自动检测 > `:8080` 回退。

```bash
# 仅用于单元测试：stub 后端（无外部服务）
export EMBEDDING_BACKEND=stub
export LLM_BACKEND=stub
```

**禁止** 使用 `EMBEDDING_BACKEND=local` — 它会启动 Python 子进程调用 Ollama，极慢。

## 工具配置

### Web Search (MCP)

MiniMax MCP server 提供 `web_search` 和 `understand_image` 工具。配置方式见 `.claude/settings.json`。

**重要**：内置 `WebSearch` 工具不适用于 MiniMax API。始终使用 MiniMax MCP server 的 `web_search`。

## 开发流程规范

### 里程碑开发流程

每个里程碑必须经历以下阶段，不可跳过：

1. **里程碑规划** — 从 `docs/milestones/TEMPLATE.md` 复制模板，填写目标、任务拆分、验收标准、风险评估
2. **模块开发** — 按功能模块逐个开发，不可一次性生成全部代码。每个模块必须覆盖测试用例
3. **质量门控**（每个模块完成后）— 测试通过 + Clippy 无新增警告 + 无 O(n²) 算法
4. **里程碑验收**（所有模块完成后）— 回归测试 + 覆盖率 ≥ 90% + 性能回归 + 可达性测试
5. **端到端测试** — 运行 benchmark suite 验证业务流程，生成报告到 `benchmarks/results/`

### 测试覆盖率门控

- **全局门控**：`cargo llvm-cov --lib` ≥ 90%
- **不可覆盖的模块**：在代码中用注释标注，例如：
  ```rust
  // coverage:skip requires external LLM service
  // coverage:skip requires running daemon
  ```

### 退化判定规则

以下任一条件成立即判定为退化，里程碑开发失败，必须重新进入开发阶段修正：

- `cargo test` 出现新增失败
- 覆盖率下降（低于里程碑前的基线）
- 性能回归测试失败（P50/P95 超过阈值）
- Clippy 出现新增警告
- Benchmark 指标下降（对比上一版本报告）

### 版本快照存储规则

版本特定的数据（测试数量、覆盖率、QPS、benchmark 结果等）**禁止写入 CLAUDE.md**。
存储位置：`docs/milestones/vXX-summary.md`
模板：`docs/milestones/TEMPLATE.md`

### 测试文件管理

- **单元测试**：`#[cfg(test)] mod tests` 写在源文件内
- **集成测试**：写在 `tests/` 目录下，命名 `{module}_test.rs`
- **性能回归测试**：写在 `tests/perf_regression.rs`
- **Benchmark 脚本**：写在 `benchmarks/scripts/` 或 `benchmarks/src/` 下
- **临时测试文件**：用完立即删除，不提交到 Git

## 安全红线

**安全问题和硬编码值是代码红线。发现时必须立即修复。**

### 保留的 Agent 名称
- `"kernel"`, `"system"`, `"root"`, `"admin"` 在 `PermissionGuard::new()` 中硬编码为受信（`src/api/permission.rs:179-181`）
- 这些名称**禁止**被用户注册 — 它们绕过所有权限检查
- 执行：`register_agent()` 拒绝这些名称，返回 `PermissionDenied`
- 添加新的受信 agent 名称时，**必须**同时添加到 `src/kernel/ops/agent.rs` 的 `RESERVED_AGENT_NAMES`

### 内容大小限制
- 每个对象最大内容：**10 MB**（`MAX_CONTENT_BYTES` in `src/api/semantic.rs`）
- 在 `decode_content()` 中执行 — 拒绝超限 payload

### 原子写入
- 所有持久化**必须**使用 `atomic_write_json()` 或 `atomic_write_bytes()`（`src/kernel/persistence.rs`）
- **禁止** 直接使用 `std::fs::write()` 写入 JSON/index 文件 — 部分写入会损坏状态
- 模式：写入 `.tmp` → rename 到最终路径

### 权限边界
- 租户隔离是**最严格的安全边界** — 即使受信 agent（`kernel`, `system`）也不能绕过
- 跨租户访问需要显式 `CrossTenant` 权限授予
- rename 操作上禁止 `let _ =` — 错误必须被记录或传播

### 密钥
- **禁止** 在源码中硬编码 API key、token 或密码
- 使用环境变量或 `~/.plico/` 配置文件

## Tokio 运行时模式（Daemon）

**"Cannot start a runtime from within a runtime" panic** 发生在 `#[tokio::main]` 创建多线程运行时后，provider 方法在该上下文中调用 `block_on`。

**修复**：使用 `try_current()` + `block_in_place()` 模式：
```rust
match tokio::runtime::Handle::try_current() {
    Ok(handle) => tokio::task::block_in_place(|| handle.block_on(async_fn())),
    Err(_) => rt.block_on(async_fn()),
}
```

**异步构造**：使用 `OnceLock` 延迟探测：
```rust
dimension: OnceLock<usize>  // 构造时不计算
```

**CLI daemon 路由**：`commands/mod.rs` 中的命令仅用于 embedded 模式。Daemon 模式需要在 `main.rs` 的 `build_remote_request()` 中路由。

详见 skill `plico-tokio-patterns`。

## 测试编写模式

```bash
# 全量测试
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# 仅 lib 测试（最快）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 单模块测试
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib kernel::ops::fs::tests
```

**Kernel 测试**：使用 `crate::kernel::tests::make_kernel()` 返回 `(Arc<AIKernel>, TempDir)`。

**Handler 测试**：构造 `ApiRequest` 变体 → `kernel.handle_api_request(req)` → 检查 `ApiResponse`。

**Tool 测试**：直接调用 `handle(kernel, "tool.name", &params, agent_id)` → 检查 `ToolResult`。

**权限门控**：Delete、SendMessage、Execute 等操作需要先 `GrantPermission`：
```rust
kernel.handle_api_request(ApiRequest::GrantPermission {
    agent_id: "test_agent".to_string(),
    action: "Delete".to_string(),
    scope: Some("*".to_string()),
    expires_at: None,
});
```

**Stub embedding 限制**：stub 后端返回空向量，语义搜索可能返回 0 结果。测试中不要断言搜索结果数量，用 `assert!(resp.ok)` 即可。

## 常见测试陷阱

| 问题 | 原因 | 解决 |
|------|------|------|
| `RegisterAgent` 字段错误 | 只有 `name`，无 `agent_id`/`display_name` | 只传 `name` |
| `EventType::Meeting` 不存在 | 枚举是 `Sync` | 查源码确认变体名 |
| `ToolResult.value` 不存在 | 字段是 `output` | 用 `result.output["key"]` |
| `BatchCreateItem` 无 Default | 必须手动构造所有字段 | 逐字段赋值 |
| `context_assemble` 参数 | 第一个参数是 `&[ContextCandidate]` 不是 `&[String]` | 传空切片测试 |
| `make_kernel` 不在作用域 | `fs.rs` 等 ops 模块需要全路径 | 用 `crate::kernel::tests::make_kernel()` |
| `end_session` 缺 `session_id` | 服务端要求 `session_id` 字段 | 从 `start_session` 响应中提取 |

## AI 导航

创建或更新 `AGENTS.md`、`INDEX.md` 或任何项目导航索引时，使用 **ariadne-thread** skill。所有模块目录都有 `INDEX.md`（L1），包含公共 API、依赖、被依赖、任务路由和修改风险。详见 `AGENTS.md`。

## 文档组织规范

### 目录结构

```
docs/
├── milestones/       # 里程碑文档（统一模板，见 TEMPLATE.md）
├── plans/            # 进行中的计划（完成后迁移到 milestones/）
├── design/           # 架构设计文档（长期有效）
├── genesis-reference.md  # 核心参考文档
├── genesis.md        # 项目起源
└── dashboard/        # 可视化仪表盘
```

### 文件命名规则

- **禁止**文件名中包含空格（用连字符 `-` 替代）
- 里程碑文件：`vXX-<name>.md`（如 `v46-summary.md`）
- 设计文档：`<topic>.md`（如 `soul-v3-architecture.md`）
- 审计报告：已完成的迁移到 `docs/milestones/`，不保留在 `docs/` 根目录
- Benchmark 报告：存放在 `benchmarks/docs/` 或 `benchmarks/results/`

### 文档生命周期

- **计划类**（`docs/plans/`）→ 完成后迁移到 `docs/milestones/`
- **里程碑总结**（`docs/milestones/`）→ 长期保留
- **设计文档**（`docs/design/`）→ 长期保留
- **临时报告** → 用完删除，不提交到 Git
- **根目录禁止**放置 `.log`、临时 `.py`、临时报告文件

## 相关参考

- [AIOS (Rutgers)](https://github.com/agiresearch/AIOS) — 完整架构参考
- VexFS — 内核级向量搜索集成
- LanceDB — 列式向量 + 元数据存储
- OpenViking — 上下文管理，减少 token 使用约 91%

## 详细规则文件

以下规则按需加载，详见 `.claude/rules/` 目录：
- **编码原则**：`.claude/rules/coding-principles.md` — Think Before Coding、Simplicity First、Surgical Changes 等
- **Benchmark 操作**：`.claude/rules/benchmark.md` — Benchmark 框架完整操作指南
- **开发流程详细说明**：`.claude/rules/development-workflow.md` — 里程碑模板、退化判定、性能回归测试标准等
