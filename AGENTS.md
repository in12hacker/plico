# 太初 (Plico) — AI-Native Operating System

专为 AI Agent 设计的操作系统内核。无人类 CLI/GUI，所有数据操作通过语义 API。系统与模型无关。详见 `system-v3.md`（Soul 3.0）和 `docs/genesis-reference.md`。

## 目录地图

```
src/
├── cas/                 # Content-Addressed Storage — SHA-256 对象标识，自动去重
├── memory/              # Layered Memory — Ephemeral / Working / LongTerm / Procedural
├── intent/              # Intent Router — NL → ApiRequest（启发式 + LLM 链）
├── scheduler/           # Agent 生命周期 — 注册、优先队列、意图调度、消息传递
├── fs/                  # Semantic Filesystem — 标签 CRUD、向量搜索、KG
│   ├── semantic_fs/     #   核心 CRUD + 事件存储
│   ├── embedding/       #   Embedding 后端（5 种 + circuit breaker）
│   ├── search/          #   向量 + BM25 搜索（HNSW + 内存后端）
│   └── graph/           #   Knowledge Graph（PetgraphBackend + redb 4.0）
├── kernel/              # AI Kernel — 中央编排器
│   ├── cognition/       #   Soul v3.0 认知共生引擎（12 文件）
│   ├── handlers/        #   14 个领域 handler
│   ├── tools/           #   7 个内置工具 handler
│   └── ops/             #   24 个操作文件
├── api/                 # API 层 — 权限护栏 + 语义 JSON 协议
├── tool/                # Tool 抽象 — "Everything is a Tool"
├── temporal/            # 时间推理 — 自然语言时间 → 时间范围
├── llm/                 # LLM provider 抽象 — 模型无关聊天接口
├── mcp/                 # MCP client — 连接外部 MCP server
├── client.rs            # KernelClient trait + EmbeddedClient + RemoteClient
├── bin/                 # 4 个二进制入口
│   ├── plicod.rs        #   Daemon — TCP + UDS，start/stop/status 生命周期
│   ├── plico_mcp.rs     #   MCP stdio server (JSON-RPC 2.0)
│   ├── plico_sse.rs     #   SSE 流式适配器
│   └── aicli/           #   AI 语义 CLI（daemon-first, --embedded 回退）
├── lib.rs               # Crate root
└── main.rs              # Stub — 指向 plicod/aicli/plico-mcp

tests/                   # 集成测试（33 文件）
benchmarks/              # 自研 benchmark 框架（Python, uv 管理）
```

## 快速导航

| 区域 | 入口 | 用途 |
|------|------|------|
| CAS 存储 | `src/cas/INDEX.md` | AIObject, CASStorage, 内容寻址 |
| Memory 系统 | `src/memory/INDEX.md` | LayeredMemory, 4 层架构, 持久化 |
| Intent 路由 | `src/intent/INDEX.md` | NL → `ApiRequest`, ChainRouter |
| Agent 调度 | `src/scheduler/INDEX.md` | AgentScheduler, Intent, 消息传递 |
| Semantic FS | `src/fs/INDEX.md` | SemanticFS, 向量搜索, KG, 上下文加载 |
| AI Kernel | `src/kernel/INDEX.md` | AIKernel — 中央编排器 |
| API 层 | `src/api/INDEX.md` | 权限护栏, 语义 JSON 协议 |
| Tool 系统 | `src/tool/INDEX.md` | ToolRegistry, "Everything is a Tool" |
| 认知引擎 | `src/kernel/cognition/INDEX.md` | Soul v3.0 — CognitiveLoop, SkillForge |
| 二进制 | `src/bin/INDEX.md` | plicod, plico-mcp, plico-sse, aicli |
| Benchmark | `benchmarks/README.md` | 端到端性能与质量评测 |

## 构建与测试

| 命令 | 用途 |
|------|------|
| `cargo build` | 构建所有目标 |
| `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib` | 运行单元测试（最快） |
| `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test` | 运行所有测试 |
| `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib` | 覆盖率测量 |
| `cargo clippy -- -D warnings` | Lint 检查（必须零警告） |
| `cargo build --release` | Release 构建 |

## 代码规范

- **文件**：`snake_case.rs`，每文件一个概念，目标 < 300 行
- **命名**：`snake_case` 函数，`PascalCase` 类型，`SCREAMING_SNAKE` 常量
- **模块**：`pub mod` 在 `mod.rs`；大模块拆分为 `dir/mod.rs` + 子文件
- **公共 API**：`pub fn`，默认私有
- **测试**：`#[cfg(test)] mod tests` 同文件内联；大测试套件放在模块目录下 `tests.rs`

## 架构约束

- 依赖方向：**api/bin → kernel → tool/fs/intent → cas/memory/scheduler/temporal/llm**（禁止反向）
- `kernel/` 是唯一导入所有其他模块的模块 — 所有子系统调用通过 `AIKernel`
- `AIKernel` 字段为 `pub(crate)` — 仅 crate 内可见
- CAS 是唯一直接接触宿主文件系统的模块
- 无 `unsafe` 块（库代码中）除非有 `# Safety` 文档注释
- **Soul v3.0**：Plico 是**认知共生体** — 优化 Agent 的输入质量，但从不替代 Agent 的决策

## 跨模块模式

### 错误处理
- 所有错误类型化：`CASError`, `MemoryError`, `SchedulerError`, `FSError`, `KGError`, `LlmError`, `McpError`（均 `thiserror`）
- 库代码中禁止 panic（除非关键不变量 `expect()` 带消息）

### 日志
- `tracing` crate 结构化日志，`tracing_subscriber::fmt` + `env_filter`

### 并发
- `RwLock` 用于内存映射，`tokio` 用于异步 TCP/UDS，`EventBus` 用 `tokio::sync::broadcast`

### 序列化
- JSON 用于 CAS、TCP 协议、事件日志、图持久化、MCP 消息

### Clippy 策略
- `cargo clippy` 零警告 — 无 `#[allow(clippy::...)]`（结构 lint）

## Agent 工作流（编辑前检查清单）

**在任何代码修改前完成：**

- [ ] 通过快速导航表定位目标模块
- [ ] 打开模块 `INDEX.md`，修改公共 API 前检查 Dependents
- [ ] 确认签名/错误类型变更的 Modification Risk
- [ ] 运行 `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib` — 所有测试必须通过
- [ ] 如果二进制变更：`cargo build --bin [name]` 成功
- [ ] 如果新模块或构建命令变更：更新 AGENTS.md

## 索引排除

```
target/          # Cargo 构建输出
Cargo.lock       # Lock file
.claude/         # Claude Code 设置
.cursor/         # Cursor 设置
.runtime/        # 开发时运行时暂存空间
.logs/           # Daemon 日志文件
benchmarks/      # Benchmark 框架（独立管理）
*.rlib           # 编译的 Rust 库文件
*.bak            # 备份文件
```
