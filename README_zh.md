# Plico — AI 原生操作系统

**语言：** [简体中文](README_zh.md) · [English](README.md)

从 **AI 视角** 设计的操作系统——不以人类优先的 CLI/GUI 为中心，也不把「路径型文件系统」当作主要抽象。上层智能体通过 **语义 API**（内容、标签、意图、图）与系统交互。实现 **与具体模型解耦**：向量与可选 LLM 路由可使用本地后端（Ollama、ONNX），测试时也可用 stub。

## 状态

**活跃开发中——核心栈已实现，并由集成测试覆盖。** 包括：CAS、语义文件系统（向量 + BM25 + 知识图谱）、分层记忆（四层）、智能体调度器（派发循环 + 结果消费）、内核事件总线（类型化发布/订阅 + 过滤 + 持久化事件日志）、权限护栏、自然语言意图路由（启发式 + 可选 LLM）、工具注册表（内置 + 外部 MCP）、基于知识图谱的 Skill 发现、时间语义、智能体检查点、发现 / 委派 / 配额 API、LLM 提供者抽象（Ollama / OpenAI 兼容 / stub）、`plicod`（仅 TCP 守护进程）、`plico-mcp`（stdio JSON-RPC），以及 `aicli`（语义命令行）。

设计理念与论证见 `system.md`（中文）。

## 架构

```
外部 AI 智能体 / MCP 客户端
        ↓  语义 JSON（TCP / CLI / MCP stdio）
┌──────────────────────────────────────────────┐
│  AI 内核                                      │
│  ├─ 智能体调度器 + 派发循环                    │
│  ├─ 分层记忆 + 持久化                         │
│  ├─ 事件总线（类型化发布/订阅 + 持久化日志）    │
│  ├─ 内置工具注册表与执行                       │
│  └─ 权限护栏                                  │
│                                               │
│  意图层 — 自然语言 → ApiRequest（可选）        │
│                                               │
│  AI 原生「文件系统」                           │
│  ├─ 内容寻址存储（CAS）                       │
│  ├─ 语义 / 混合检索（向量、BM25）             │
│  ├─ 知识图谱（PetgraphBackend）               │
│  └─ 分层上下文加载（L0/L1/L2）                │
└──────────────────────────────────────────────┘
```

`plicod` **仅 TCP**（默认 `0.0.0.0:7878`）。没有独立 HTTP 面板；系统状态通过 `ApiRequest::SystemStatus`（JSON `{"system_status":null}`）查询。CLI 方式：`aicli system-status`。

## 快速开始

```bash
# 构建
cargo build --release

# 运行测试
cargo test

# AI 友好 CLI（进程内内核；数据目录由 --root 指定）
cargo run --bin aicli -- put --content "hello" --tags "greeting"
cargo run --bin aicli -- get <CID>
cargo run --bin aicli -- search --query "greeting"

# 连接已运行的守护进程
cargo run --bin aicli -- --tcp 127.0.0.1:7878 search --query "hello"

# 常驻守护进程（TCP 语义 API + 派发循环 + 结果消费）
cargo run --bin plicod -- --port 7878
# 或：PLICO_ROOT=~/.plico cargo run --bin plicod

# 查看内核状态（本地内核模式）
cargo run --bin aicli -- system-status

# MCP 适配器（stdio JSON-RPC）
cargo run --bin plico-mcp
```

完整子命令（CRUD、检索、智能体、记忆、图、工具、事件、意图、技能等）见：

`cargo run --bin aicli -- --help`

## 代码布局

```
src/
├── cas/            # SHA-256 内容寻址对象存储
├── memory/         # 分层记忆（瞬时 → 长期）与持久化
│   └── layered/    # LayeredMemory 核心 + 测试
├── intent/         # 自然语言 → 结构化 ApiRequest + 执行辅助
├── scheduler/      # 智能体、优先级、消息、执行派发
├── fs/             # 语义存储：标签、嵌入、图、上下文加载器
│   ├── semantic_fs/ # SemanticFS 核心 + 事件 + 测试
│   ├── embedding/   # EmbeddingProvider（Ollama、本地 ONNX、stub、JSON-RPC）
│   ├── search/      # SemanticSearch（内存、BM25、HNSW）
│   └── graph/       # KnowledgeGraph trait + PetgraphBackend
├── kernel/         # AIKernel — 编排、工具、持久化、派发
│   ├── event_bus.rs # 类型化发布/订阅 + 持久化事件日志
│   └── ops/         # 操作分组（fs、agent、memory、events、graph 等）
├── api/            # ApiRequest / ApiResponse 协议 + 权限层
├── tool/           # Tool trait 与注册表（「一切皆工具」）
├── temporal/       # 自然语言时间范围（启发式 + 可选 LLM）
├── llm/            # LlmProvider trait（Ollama / OpenAI 兼容 / stub）
├── mcp/            # MCP 客户端 — 外部工具集成
├── bin/
│   ├── plicod.rs       # 异步 TCP 服务（JSON ApiRequest/ApiResponse）
│   ├── plico_mcp.rs    # MCP stdio 服务（JSON-RPC 2.0）
│   └── aicli/          # 语义 CLI（按命令组拆分处理器）
├── lib.rs
└── main.rs

tests/               # 集成测试（内核、CLI、FS、记忆、检索、MCP、意图、权限等）
AGENTS.md            # 面向贡献者与智能体的详细目录说明
CLAUDE.md            # 维护者 / 智能体协作指引
```

## 设计文档

完整 AI 原生操作系统设计见 **`system.md`**（中文）。
