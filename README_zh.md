# 太初 (Plico) — AI 原生操作系统内核

**语言：** [简体中文](README_zh.md) · [English](README.md)

从 **AI 视角** 设计的操作系统内核——不以人类优先的 CLI/GUI 为中心，也不把「路径型文件系统」当作主要抽象。上层智能体通过 **语义 API**（内容、标签、意图、图）与系统交互。实现 **推理框架无关**：Embedding 和 LLM 后端均支持任何 OpenAI-compatible API 的推理服务（llama.cpp、vLLM、SGLang、TensorRT-LLM、Ollama 等），也可使用本地 ONNX 或 stub 测试。

"太初"——一切就绪，等待被使用。AI-OS 从意识觉醒到自主进化的起点。

## 状态

**太初 (Node 25) — 132 个源文件, 50,671 行 Rust, 1,405 个测试 (0 失败)**

核心栈：CAS、语义文件系统（向量 + BM25 + redb 知识图谱）、分层记忆（四层 + MemoryScope）、智能体调度器、内核事件总线（类型化发布/订阅 + 过滤 + 持久化日志）、权限护栏、Hook 系统（5 个拦截点）、意图系统（DAG 分解 + 自主执行）、上下文预算引擎（L0/L1/L2）、工具注册表（37 个内置 + 外部 MCP）、智能体生命周期（检查点/恢复/发现/委派）、学习闭环（执行统计 + 技能发现 + 自我修复）、`plicod`（TCP+UDS 守护进程）、`plico-mcp`（stdio JSON-RPC）、`aicli`（语义 CLI）。

灵魂 2.0 对齐度：**94.7%**。架构红线：**8/8 (100%)**。

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
│  └─ 权限护栏 + 智能体认证（HMAC）                  │
├────────────────────────────────────────────────────┤
│  AI 原生文件系统                                    │
│  ├─ 内容寻址存储（CAS, SHA-256）                   │
│  ├─ 语义搜索（BM25 + HNSW 向量）                  │
│  ├─ 知识图谱（redb, 14 种边类型）                  │
│  └─ 分层上下文加载（L0/L1/L2）                     │
└────────────────────────────────────────────────────┘
```

**守护进程优先**: `plicod` 托管内核。客户端通过 UDS 或 TCP 连接，使用长度前缀 JSON 帧协议。`--embedded` 模式可用于测试。

## 快速开始

```bash
# 构建
cargo build --release

# 运行全部测试 (1,405 个)
cargo test

# 启动守护进程（推荐）
cargo run --bin plicod -- --port 7878

# CLI（默认连接守护进程）
aicli agent --name my-agent
aicli put --content "关于 Plico 架构的知识" --tags "plico,arch"
aicli search "架构"
aicli remember --content "重要洞察" --tier working --agent my-agent
aicli recall --agent my-agent

# CLI 嵌入模式（无需守护进程）
aicli --embedded put --content "hello" --tags "test"

# MCP 适配器（stdio JSON-RPC 2.0）
cargo run --bin plico-mcp
```

## 十条公理（灵魂 2.0）

| # | 公理 | 推论 |
|---|------|------|
| 1 | **Token 是最稀缺资源** | 分层返回 L0/L1/L2，追踪消耗，delta 优于 full |
| 2 | **意图先于操作** | Agent 声明意图，OS 组装上下文并执行 |
| 3 | **记忆跨越边界** | 四层记忆持久化，checkpoint/restore 跨"死亡" |
| 4 | **共享先于重复** | MemoryScope: Private / Shared / Group |
| 5 | **机制，不是策略** | 内核提供原语，不替 Agent 决策 |
| 6 | **结构先于语言** | JSON 是唯一内核接口，NL 在接口层 |
| 7 | **主动先于被动** | 意图预取、warm context、目标自生成 |
| 8 | **因果先于关联** | KG 记录 CausedBy 因果链 |
| 9 | **越用越好** | AgentProfile 累积，技能发现，自我修复 |
| 10 | **会话是一等公民** | session-start/end、warm_context、变更通知 |

## 代码布局

```
src/
├── cas/            # SHA-256 内容寻址对象存储
├── memory/         # 分层记忆（瞬时 → 长期）+ 持久化
├── intent/         # NL → 结构化 ApiRequest（接口层，非内核）
├── scheduler/      # 智能体、优先级、消息、执行派发
├── fs/             # 语义存储：标签、嵌入、图、上下文
│   ├── embedding/  # EmbeddingProvider（OpenAI-compatible、Ollama、ONNX、stub）
│   ├── search/     # SemanticSearch（BM25、HNSW）
│   └── graph/      # KnowledgeGraph（redb，14 种边类型）
├── kernel/         # AIKernel — 编排、工具、Hook、持久化
│   ├── hook.rs     # Hook 注册表（5 个拦截点）
│   ├── event_bus.rs # 类型化发布/订阅 + 持久化事件日志
│   └── ops/        # 24 个操作模块
├── api/            # ApiRequest / ApiResponse + 权限 + 认证
├── tool/           # Tool trait 与注册表（「一切皆工具」）
├── llm/            # LlmProvider trait（OpenAI-compatible / Ollama / llama.cpp / stub）
├── mcp/            # MCP 客户端 — 外部工具集成
├── client.rs       # KernelClient trait（嵌入 / UDS / TCP）
└── bin/
    ├── plicod.rs       # 守护进程（TCP + UDS）
    ├── plico_mcp/      # MCP stdio 服务（JSON-RPC 2.0）
    └── aicli/          # 语义 CLI（守护进程优先）

tests/              # 33 个集成测试文件
docs/
├── genesis-reference.md    # 太初完整参考文档
├── genesis-audit-n25*.md   # 审计报告
└── design-node*.md         # 24 份设计文档（N2-N25）
```

## 设计文档

- `system-v2.md` — 灵魂 2.0：AI 第一人称视角的十条公理
- `docs/genesis-reference.md` — 太初完整参考文档
- `AGENTS.md` — 面向 AI 智能体的详细目录导航
