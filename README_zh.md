# 太初 (Plico) — AI 原生操作系统内核

[![CI](https://github.com/in12hacker/plico/actions/workflows/ci.yml/badge.svg)](https://github.com/in12hacker/plico/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2021-edition.svg)](https://www.rust-lang.org)

**语言：** [简体中文](README_zh.md) · [English](README.md)

面向**个人数字分身**的 AI 原生内核。对象与记忆是 canonical 主数据；文档、表格、PPT
和图形界面只在人需要时生成，不是核心数据模型。AI 客户端使用类型化
`plico.personal.v2` 对象/记忆/会话/投影 API，不直接操作路径文件或内核管理接口。推理栈保持
模型无关。

"太初"——一切就绪，等待被使用。AI-OS 从意识觉醒到自主进化的起点。

## 状态

项目处于持续开发阶段。验证结果随每次变更报告，不在此维护容易失真的静态计数。

核心栈：CAS、语义文件系统（向量 + BM25 + 知识图谱）、分层记忆、智能体调度器、内核事件总线、权限护栏、意图与上下文预算系统、检索融合、模型无关推理后端、`plicod`、`plico-mcp` 和 `aicli`。

## 架构

```
个人 AI 客户端 / MCP 客户端
        ↓  语义 JSON
┌────────────────────────────────────────────────────┐
│  接口适配器                                         │
│          ┌─────────┐  ┌───────────┐                 │
│          │  aicli  │  │ plico-mcp │                 │
│          └────┬────┘  └─────┬─────┘                 │
│               └─────────────┤                       │
│               ┌───────▼────────┐                    │
│               │  KernelClient  │ (UDS / TCP / 嵌入) │
│               └───────┬────────┘                    │
├───────────────────────┼────────────────────────────┤
│  AI 内核              │                             │
│  ├─ 个人 vault 类型化公共服务（14 项）              │
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

**唯一公共契约**：`plicod`、`KernelClient`、`plico-mcp` 与 `aicli` 共用 14 项类型化
operation：`capabilities.describe`、`runtime.readiness`、`object.put/get/search`、
`memory.create/get/recall/update/delete`、`projection.status/rebuild`、`session.start/end`。
UDS、MCP、嵌入模式使用可信本地 owner；TCP 默认仅监听 loopback，并强制 bearer。
`projection.rebuild` 仅允许 owner 调用。当前支持 Memory embedding 控制面，但 Memory
vector/hybrid/BM25 检索仍明确 unsupported；`memory.recall` 仍为 lexical。

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

# 启动守护进程（默认 127.0.0.1:7878）。首次启动会在 PLICO_ROOT 下
# 以 0600 权限创建/复用 agent_tokens.json，不打印或记录 token。
cargo run --bin plicod -- start

# 守护进程生命周期
cargo run --bin plicod -- stop       # 优雅停止
cargo run --bin plicod -- status     # JSON 状态输出

# CLI（默认走 UDS；点号 operation 与公共协议完全一致）
cargo run --bin aicli -- capabilities.describe
cargo run --bin aicli -- object.put --content "关于 Plico 的知识" --tag plico --tag 架构
cargo run --bin aicli -- object.search --query "架构"
cargo run --bin aicli -- memory.create --content "重要洞察" --tag 洞察
cargo run --bin aicli -- memory.recall --query "洞察"
cargo run --bin aicli -- projection.status --revision-id UUID
cargo run --bin aicli -- projection.rebuild --all-eligible

# CLI 嵌入模式（无需守护进程）
cargo run --bin aicli -- --embedded object.put --content "hello" --tag test

# TCP：只在本机读取 owner-only 凭据；不要把 token 写入参数或日志。
export PLICO_ROOT="${PLICO_ROOT:-$HOME/.plico}"
export PLICO_BEARER_TOKEN="$(jq -r '.\"personal-owner\".token' "$PLICO_ROOT/agent_tokens.json")"
cargo run --bin aicli -- --tcp 127.0.0.1:7878 runtime.readiness

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
| 4 | **主数据先于投影** | 个人 Memory/CAS 是主数据，人类文件是按需视图 |
| 5 | **机制，不是策略** | 内核提供原语，不替 Agent 决策 |
| 6 | **结构先于语言** | JSON 是唯一内核接口，NL 在接口层 |
| 7 | **主动先于被动** | 意图预取、warm context、目标自生成 |
| 8 | **因果先于关联** | KG 记录 CausedBy / DependsOn / Produces 因果链 |
| 9 | **越用越好** | AgentProfile 累积，技能发现，自我修复 |
| 10 | **会话是一等公民** | 持久化 session start/end 与单调事件水位 |

## 代码布局

```
src/
├── cas/                 # SHA-256 内容寻址对象存储
├── memory/              # 分层记忆（瞬时 → 长期）+ 持久化
├── intent/              # 内部自然语言意图路由，不是公共 wire 协议
├── scheduler/           # 智能体、优先级、消息、执行派发
├── fs/                  # 语义存储：标签、嵌入、图、上下文
│   ├── embedding/       # EmbeddingProvider（OpenAI-compatible、Ollama、local worker、stub）
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
├── api/                 # plico.personal.v2 类型化协议 + 内部旧命令
├── tool/                # Tool trait 与注册表（「一切皆工具」）
├── temporal/            # 时间推理（自然语言时间 → 时间范围）
├── llm/                 # LlmProvider trait（OpenAI-compatible / Ollama / stub）
├── mcp/                 # MCP 客户端 — 外部工具集成
├── client.rs            # KernelClient trait（嵌入 / UDS / TCP）
└── bin/
    ├── plicod.rs        # 守护进程（TCP + UDS，start/stop/status 生命周期，PID 文件）
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
