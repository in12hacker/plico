# 太初 (Plico) — AI 原生操作系统内核

**语言：** [简体中文](README_zh.md) · [English](README.md)

从 **AI 视角** 设计的操作系统内核——不以人类优先的 CLI/GUI 为中心，也不把「路径型文件系统」当作主要抽象。上层智能体通过 **语义 API**（内容、标签、意图、图）与系统交互。实现 **推理框架无关**：Embedding 和 LLM 后端均支持任何 OpenAI-compatible API 的推理服务（llama.cpp、vLLM、SGLang、TensorRT-LLM、Ollama 等），也可使用本地 ONNX 或 stub 测试。

"太初"——一切就绪，等待被使用。AI-OS 从意识觉醒到自主进化的起点。

## 状态

**v46 — 212 个源文件, 85,116 行 Rust, 2,075 单元测试 (0 失败), 44 个集成测试文件**

核心栈：CAS、语义文件系统（向量 + BM25 + redb 知识图谱，17 种边类型）、分层记忆（四层 + MemoryScope）、智能体调度器、内核事件总线（类型化发布/订阅 + 过滤 + 持久化日志）、权限护栏、Hook 系统（5 个拦截点）、意图系统（DAG 分解 + 自主执行）、上下文预算引擎（L0/L1/L2）、工具注册表（37 个内置 + 外部 MCP）、智能体生命周期（检查点/恢复/发现/委派）、学习闭环（执行统计 + 技能发现 + 自我修复）、检索融合引擎（RFE，7 路自适应排序）、统一配置（`config.json` + 环境变量 + CLI）、`plicod`（TCP+UDS 守护进程 + start/stop/status 生命周期管理）、`plico-sse`（A2A SSE 适配器）、`plico-mcp`（stdio JSON-RPC）、`aicli`（语义 CLI）。

灵魂 3.0 对齐度：**94.7%**。架构红线：**9/9 (100%)**。

## 架构

```
外部 AI 智能体 / MCP 客户端
        ↓  语义 JSON
┌────────────────────────────────────────────────────┐
│  接口适配器                                         │
│  ┌─────────┐  ┌───────────┐  ┌──────────┐         │
│  │  aicli   │  │ plico-mcp │  │ plico-sse│         │
│  └────┬─────┘  └─────┬─────┘  └────┬─────┘         │
│       └───────────────┼─────────────┘               │
│               ┌───────▼────────┐                    │
│               │  KernelClient  │ (UDS / TCP / 嵌入) │
│               └───────┬────────┘                    │
├───────────────────────┼────────────────────────────┤
│  AI 内核              │                             │
│  ├─ 智能体调度器 + 派发循环                         │
│  ├─ 分层记忆（四层 + MemoryScope）                  │
│  ├─ 事件总线（类型化发布/订阅 + 持久化日志）        │
│  ├─ Hook 系统（5 个拦截点）                        │
│  ├─ 意图系统（DAG 分解 + 自主执行器）               │
│  ├─ 上下文预算引擎（L0/L1/L2）                     │
│  ├─ 内置工具注册表（37 个工具）                     │
│  ├─ 认知引擎（Soul v3.0）                          │
│  └─ 权限护栏 + 智能体认证（HMAC）                  │
├────────────────────────────────────────────────────┤
│  AI 原生文件系统                                    │
│  ├─ 内容寻址存储（CAS, SHA-256）                   │
│  ├─ 语义搜索（BM25 + HNSW 向量）                  │
│  ├─ 知识图谱（redb, 17 种边类型）                  │
│  └─ 分层上下文加载（L0/L1/L2）                     │
└────────────────────────────────────────────────────┘
```

**守护进程优先**: `plicod` 托管内核，支持 `start/stop/status` 生命周期命令和 PID 文件多实例保护。客户端通过 UDS 或 TCP 连接，使用长度前缀 JSON 帧协议。`--embedded` 模式可用于测试。

## 快速开始

```bash
# 构建
cargo build --release

# 运行测试（stub 后端，无外部依赖）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test

# 仅 lib 测试（最快，~2s）
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib

# 覆盖率测量
EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo llvm-cov --lib

# Clippy（零警告要求）
cargo clippy -- -D warnings

# 启动守护进程（推荐 — 默认绑定 127.0.0.1:7878）
cargo run --bin plicod -- start
cargo run --bin plicod -- start --host 0.0.0.0 --port 9000

# 守护进程生命周期
cargo run --bin plicod -- stop       # 优雅停止
cargo run --bin plicod -- status     # JSON 状态输出

# CLI（默认连接守护进程）
cargo run --bin aicli -- agent --name my-agent
cargo run --bin aicli -- put --content "关于 Plico 架构的知识" --tags "plico,arch"
cargo run --bin aicli -- search "架构"
cargo run --bin aicli -- remember --content "重要洞察" --tier working --agent my-agent
cargo run --bin aicli -- recall --agent my-agent

# CLI 嵌入模式（无需守护进程）
cargo run --bin aicli -- --embedded put --content "hello" --tags "test"

# SSE 适配器（A2A 协议，默认绑定 127.0.0.1:7879）
cargo run --bin plico-sse

# MCP 适配器（stdio JSON-RPC 2.0）
cargo run --bin plico-mcp
```

## 推断后端配置

Embedding 和 LLM 后端**与推断框架无关**。任何暴露 OpenAI-compatible `/v1/embeddings` 或 `/v1/chat/completions` 端点的服务器均可。

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

## 配置

Plico 使用三层级联（最低 → 最高优先级）：

1. **内置默认值** — 零配置即可运行
2. **配置文件** — `~/.plico/config.json`（或 `$PLICO_ROOT/config.json`）
3. **环境变量** — `PLICO_HOST`、`PLICO_DAEMON_PORT`、`EMBEDDING_BACKEND` 等
4. **CLI 标志** — `--host`、`--port`、`--root`（最高优先级）

## 十条公理（灵魂 3.0）

| # | 公理 | 推论 |
|---|------|------|
| 1 | **Token 是最稀缺资源** | 分层返回 L0/L1/L2，追踪消耗，delta 优于 full |
| 2 | **意图先于操作** | Agent 声明意图，OS 组装上下文并执行 |
| 3 | **记忆跨越边界** | 四层记忆持久化，checkpoint/restore 跨"死亡" |
| 4 | **共享先于重复** | MemoryScope: Private / Shared / Group |
| 5 | **机制，不是策略** | 内核提供原语，不替 Agent 决策 |
| 6 | **结构先于语言** | JSON 是唯一内核接口，NL 在接口层 |
| 7 | **主动先于被动** | 意图预取、warm context、目标自生成 |
| 8 | **因果先于关联** | KG 记录 CausedBy / DependsOn / Produces 因果链 |
| 9 | **越用越好** | AgentProfile 累积，技能发现，自我修复 |
| 10 | **会话是一等公民** | session-start/end、warm_context、变更通知 |

## 代码布局

```
src/
├── cas/                 # SHA-256 内容寻址对象存储
├── memory/              # 分层记忆（瞬时 → 长期）+ 持久化
├── intent/              # NL → 结构化 ApiRequest（接口层，非内核）
├── scheduler/           # 智能体、优先级、消息、执行派发
├── fs/                  # 语义存储：标签、嵌入、图、上下文
│   ├── embedding/       # EmbeddingProvider（OpenAI-compatible、Ollama、ONNX、stub）
│   ├── search/          # SemanticSearch（BM25、HNSW）
│   ├── graph/           # KnowledgeGraph（redb，17 种边类型）
│   ├── semantic_fs/     # 核心 CRUD + 事件存储
│   ├── query_decompose.rs # 查询分解引擎
│   └── retrieval_router.rs # 意图路由检索
├── kernel/              # AIKernel — 编排、工具、Hook、持久化
│   ├── cognition/       # Soul v3.0 认知引擎（12 个文件）
│   ├── handlers/        # 14 个领域 handler
│   ├── tools/           # 7 个内置工具 handler
│   ├── hook.rs          # Hook 注册表（5 个拦截点）
│   ├── event_bus.rs     # 类型化发布/订阅 + 持久化事件日志
│   └── ops/             # 24 个操作模块
├── api/                 # ApiRequest / ApiResponse + 权限 + 认证
├── tool/                # Tool trait 与注册表（「一切皆工具」）
├── temporal/            # 时间推理（自然语言时间 → 时间范围）
├── llm/                 # LlmProvider trait（OpenAI-compatible / Ollama / stub）
├── mcp/                 # MCP 客户端 — 外部工具集成
├── client.rs            # KernelClient trait（嵌入 / UDS / TCP）
└── bin/
    ├── plicod.rs        # 守护进程（TCP + UDS，start/stop/status 生命周期，PID 文件）
    ├── plico_sse.rs     # SSE 适配器（A2A 协议）
    ├── plico_mcp/       # MCP stdio 服务（JSON-RPC 2.0）
    └── aicli/           # 语义 CLI（守护进程优先，--embedded 回退）

tests/                   # 44 个集成测试文件
benchmarks/              # 自研 benchmark 框架（Python, uv）
docs/
├── genesis-reference.md # 太初完整参考文档
├── milestones/          # 里程碑文档（含模板）
├── plans/               # 进行中的计划
└── design/              # 架构设计文档
```

## 开发流程

本项目遵循**里程碑驱动的开发流程**，有严格的质量门控：

1. **里程碑规划** — `docs/milestones/TEMPLATE.md`
2. **模块开发** — 逐模块开发，每个模块必须覆盖测试
3. **质量门控** — `cargo test` + `cargo llvm-cov --lib` ≥ 90% + `cargo clippy` 零警告
4. **退化检测** — `tests/perf_regression.rs`（P50/P95 阈值）
5. **端到端验证** — benchmark suite（`benchmarks/`）

详见 `CLAUDE.md` 中的开发流程规范。

## 设计文档

- `system-v3.md` — 灵魂 3.0：AI 第一人称视角的十条公理
- `docs/genesis-reference.md` — 太初完整参考文档
- `AGENTS.md` — AI 智能体导航（目录地图 + 快速导航）
- `CLAUDE.md` — AI 编码助手的项目级规则
- `benchmarks/README.md` — Benchmark 框架文档
