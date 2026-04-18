# Plico — AI 原生操作系统

**语言：** [简体中文](README_zh.md) · [English](README.md)

从 **AI 视角** 设计的操作系统——不以人类优先的 CLI/GUI 为中心，也不把「路径型文件系统」当作主要抽象。上层智能体通过 **语义 API**（内容、标签、意图、图）与系统交互。实现 **与具体模型解耦**：向量与可选的 LLM 路由可使用本地后端（例如 Ollama），测试时也可用 stub。

## 状态

**活跃开发中——核心栈已实现，并由集成测试覆盖。** 当前基础能力包括：CAS、语义文件系统（向量 + 知识图谱）、分层记忆、带调度循环的智能体调度器、权限护栏、自然语言 **意图路由**（启发式 + 可选 LLM）、工具注册表、时间语义辅助、**TCP 守护进程**、面向编辑器/智能体的 **MCP 服务**（stdio JSON-RPC），以及 **`aicli`** 语义命令行。

设计理念与论证见中文文档 `system.md`。

## 架构

```
外部 AI 智能体 / MCP 客户端
        ↓  语义 JSON（TCP / CLI / MCP）
┌─────────────────────────────────────────────┐
│  AI 内核                                     │
│  ├─ 智能体调度器 + 派发循环                   │
│  ├─ 分层记忆 + 持久化钩子                     │
│  ├─ 内置工具注册表与执行                      │
│  └─ 权限护栏                                 │
│                                              │
│  意图层 — 自然语言 → ApiRequest（可选）       │
│                                              │
│  AI 原生「文件系统」                          │
│  ├─ 内容寻址存储（CAS）                      │
│  ├─ 语义 / 混合检索（向量、BM25）            │
│  ├─ 知识图谱                                 │
│  └─ 分层上下文加载（L0/L1/L2）               │
└─────────────────────────────────────────────┘
```

`plicod` 在 **TCP JSON** 行协议（默认端口 **7878**）之外，还提供小型 **HTTP 面板**（默认 `http://127.0.0.1:7879`，详见进程输出）。

## 快速开始

```bash
# 构建
cargo build --release

# 运行测试
cargo test

# AI 友好 CLI（进程内内核；数据目录由 --root 指定）
cargo run --bin aicli -- --root /tmp/plico put --content "hello" --tags "greeting"
cargo run --bin aicli -- --root /tmp/plico get <CID>
cargo run --bin aicli -- --root /tmp/plico search --query "greeting"

# 连接已运行的守护进程（存储以 plicod 启动参数为准；TCP 模式下请勿与 --root 混用）
cargo run --bin aicli -- --tcp 127.0.0.1:7878 search --query "hello"

# 常驻守护进程（TCP API + 派发循环 + 面板）
cargo run --bin plicod -- --port 7878 --root /tmp/plico
# 或：PLICO_ROOT=/tmp/plico cargo run --bin plicod

# MCP 适配器（stdio）；需要时可设置 PLICO_ROOT 与内核使用同一存储
PLICO_ROOT=/tmp/plico cargo run --bin plico-mcp
```

完整子命令（CRUD、检索、智能体、记忆、图、工具、事件、意图等）见：

`cargo run --bin aicli -- --help`

## 代码布局

```
src/
├── cas/          # SHA-256 内容寻址对象存储
├── memory/       # 分层记忆（瞬时 → 长期）与持久化
├── intent/       # 自然语言 → 结构化 ApiRequest
├── scheduler/    # 智能体、优先级、消息、执行派发
├── fs/           # 语义存储：标签、嵌入、图、上下文加载器
├── kernel/       # AIKernel — 编排、工具、持久化、派发
├── api/          # ApiRequest / ApiResponse 协议 + 权限层
├── tool/         # Tool trait 与注册表（「一切皆工具」）
├── temporal/     # 自然语言时间范围（启发式 + 可选 LLM）
├── llm/          # 共享 LLM 客户端辅助
├── mcp/          # plico-mcp 二进制使用的 MCP 相关辅助
├── bin/
│   ├── plicod.rs      # 异步 TCP 服务 + HTTP 面板
│   ├── plico_mcp.rs   # MCP stdio 服务
│   └── aicli/         # 语义 CLI 实现
├── lib.rs
└── main.rs         # 占位 — 请使用 plicod / aicli / plico-mcp 各二进制

tests/              # 集成测试（内核、CLI、FS、记忆、MCP、意图等）
AGENTS.md           # 面向贡献者与智能体的详细目录说明
CLAUDE.md           # 维护者 / 智能体协作指引
```

## 设计文档

完整 AI 原生操作系统设计见 **`system.md`**（中文）。
