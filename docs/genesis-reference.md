# Plico 太初 — AI-OS 完整参考文档

**版本**: Genesis (Node 25)
**日期**: 2026-04-24
**灵魂**: `system-v2.md` (Soul 2.0)
**代码**: 131 files | 49,489 lines | 1388 tests

> 本文档是 Plico 从 Node 1 到 Node 25 全部设计决策、API、架构的统一参考。
> 取代散布在 24 份 Node 设计文档中的碎片化信息。

---

## 1. Plico 是什么

Plico 是一个**为 AI Agent 设计的操作系统内核**。

**不是**: AI 应用框架、LLM 编排器、人类操作系统、"AI 增强的 Linux"。

**是**: Agent 的运行基础设施 — 管理记忆、知识、身份、资源、工具、会话。

```
┌─────────────────────────────────────────────────────┐
│  Application Layer (AI Agents)                      │
├─────────────────────────────────────────────────────┤
│  Interface Adapters                                 │
│  ┌─────────┐ ┌───────────┐ ┌──────────┐            │
│  │  aicli   │ │ plico-mcp │ │ plico-sse│            │
│  │ (CLI)    │ │ (JSON-RPC)│ │  (SSE)   │            │
│  └────┬─────┘ └─────┬─────┘ └────┬─────┘            │
│       │             │             │                  │
│       └─────────────┼─────────────┘                  │
│                     │                                │
│             ┌───────▼────────┐                       │
│             │  KernelClient  │ (trait)                │
│             │  Embedded|UDS  │                        │
│             │  |TCP          │                        │
│             └───────┬────────┘                       │
├─────────────────────┼───────────────────────────────┤
│  AI Kernel          │                                │
│  ┌──────────────────▼─────────────────────┐         │
│  │  handle_api_request(ApiRequest)         │         │
│  │         → ApiResponse (JSON)            │         │
│  ├─────────────────────────────────────────┤         │
│  │ Scheduler │ Memory │ Hook  │ EventBus   │         │
│  │ Intent    │ Tools  │ Perms │ Prefetch   │         │
│  └─────────────────────┬───────────────────┘         │
├─────────────────────────┼───────────────────────────┤
│  AI-Native File System  │                            │
│  ┌───────┐ ┌────────┐ ┌┴──────┐ ┌────────────┐     │
│  │  CAS  │ │ Search │ │  KG   │ │ Embedding  │     │
│  │SHA-256│ │BM25+Vec│ │ redb  │ │ ONNX/Stub  │     │
│  └───────┘ └────────┘ └───────┘ └────────────┘     │
└─────────────────────────────────────────────────────┘
```

---

## 2. 十条公理 (Soul 2.0)

| # | 公理 | 核心推论 |
|---|------|---------|
| 1 | **Token 是最稀缺资源** | 分层返回 L0/L1/L2，追踪消耗，delta 优于 full |
| 2 | **意图先于操作** | Agent 声明意图，OS 组装上下文并执行 |
| 3 | **记忆跨越边界** | 4 层记忆持久化，checkpoint/restore 跨"死亡" |
| 4 | **共享先于重复** | MemoryScope: Private/Shared/Group |
| 5 | **机制，不是策略** | 内核提供原语，不替 agent 决策 |
| 6 | **结构先于语言** | JSON 是唯一内核接口，NL 在接口层 |
| 7 | **主动先于被动** | DeclareIntent → 后台预取 → 上下文就绪 |
| 8 | **因果先于关联** | KG 记录 CausedBy 因果链 |
| 9 | **越用越好** | AgentProfile 累积，技能发现，自我修复 |
| 10 | **会话是一等公民** | session-start/end，warm_context，变更通知 |

---

## 3. 模块架构

### 3.1 模块清单

| 模块 | 路径 | 行数 | 测试 | 职责 |
|------|------|------|------|------|
| **kernel** | `src/kernel/` | 21,014 | 339 | 核心调度、Hook、意图、执行、预取、学习 |
| **fs** | `src/fs/` | 7,647 | 167 | CAS、语义搜索、KG(redb)、向量索引 |
| **bin** | `src/bin/` | 7,857 | 161 | aicli + plicod + plico_mcp + plico_sse |
| **api** | `src/api/` | 4,130 | 81 | DTO、语义 API、版本、鉴权 |
| **memory** | `src/memory/` | 2,431 | 33 | 4 层记忆 + MemoryScope + 相关性 |
| **scheduler** | `src/scheduler/` | 1,781 | 23 | Agent 调度、配额、生命周期 |
| **intent** | `src/intent/` | 1,439 | 31 | IntentRouter（接口层，非内核） |
| **cas** | `src/cas/` | 702 | 10 | SHA-256 内容寻址存储 |
| **llm** | `src/llm/` | 682 | 13 | LlmProvider trait + 断路器 |
| **tool** | `src/tool/` | 572 | 15 | ExternalToolProvider trait |
| **mcp** | `src/mcp/` | 395 | 9 | MCP client adapter |
| **client** | `src/client.rs` | 118 | — | KernelClient trait (传输抽象) |

### 3.2 内核子模块 (kernel/ops/)

| 子模块 | 文件 | 行数 | 测试 | 引入节点 |
|--------|------|------|------|---------|
| intent | `intent.rs` | 1,370 | 29 | N21 |
| prefetch | `prefetch.rs` | 1,843 | 27 | N8→N20 |
| intent_executor | `intent_executor.rs` | 605 | 6 | N21 |
| prefetch_cache | `prefetch_cache.rs` | 530 | 16 | N20 |
| causal_hook | `causal_hook.rs` | 300 | 3 | N20 |
| prefetch_profile | `prefetch_profile.rs` | 280 | 10 | N20 |
| cross_domain_skill | `cross_domain_skill.rs` | 226 | 3 | N24 |
| self_healing | `self_healing.rs` | 209 | 4 | N23 |
| skill_discovery | `skill_discovery.rs` | 160 | 3 | N23 |
| goal_generator | `goal_generator.rs` | 148 | 3 | N24 |
| temporal_projection | `temporal_projection.rs` | 136 | 3 | N24 |
| intent_decomposer | `intent_decomposer.rs` | 136 | 3 | N23 |
| hook | `hook.rs` | 266 | 6 | N19 |

---

## 4. 连接模式

### 4.1 Daemon-First（默认）

```bash
# 默认：连接 plicod daemon via UDS
aicli agent --name my-agent

# plicod 在后台运行，管理持久化状态
plicod --root ~/.plico
```

### 4.2 Embedded（测试/调试）

```bash
# 每次调用创建独立内核实例
aicli --embedded agent --name my-agent
```

### 4.3 TCP（远程）

```bash
aicli --tcp 192.168.1.100:7878 agent --name my-agent
```

### 4.4 MCP Server

```bash
# JSON-RPC 2.0 over stdio
plico-mcp --root ~/.plico
```

---

## 5. API 参考

### 5.1 核心入口

所有操作通过 `handle_api_request(ApiRequest) → ApiResponse` 分发。
JSON 是唯一接口格式。

### 5.2 CAS (Content-Addressed Storage)

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 创建 | `create` | `put --content "..." --tags "t1,t2"` | 返回 SHA-256 CID |
| 读取 | `read` | `get <CID>` | 返回内容 + 元数据 |
| 搜索 | `search` | `search "query"` | BM25 + 向量混合搜索 |
| 更新 | `update` | `update --cid <CID> --content "..."` | 版本追踪 |
| 删除 | `delete` | `delete --cid <CID>` | 软删除 |
| 历史 | `history` | `history --cid <CID>` | 版本历史 |
| 回滚 | `rollback` | `rollback --cid <CID>` | 回退到上一版本 |
| 恢复 | `restore` | `restore --cid <CID>` | 从软删除恢复 |

### 5.3 Knowledge Graph

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 添加节点 | `add_node` | `node --label X --type entity` | Entity / Fact |
| 添加边 | `add_edge` | `edge --src A --dst B --type causes` | 14 种边类型 |
| 探索 | `explore` | `explore --cid <node_id>` | 邻居 + authority |
| 路径 | `find_paths` | `paths --src A --dst B --depth 3` | 双向遍历 |
| 因果路径 | `kg_causal_path` | — | CausedBy 专用查询 |

**边类型**: `associates_with`, `follows`, `mentions`, `causes`, `reminds`, `part_of`, `similar_to`, `related_to`, `has_participant`, `has_artifact`, `has_recording`, `has_resolution`, `has_fact`, `supersedes`

**存储引擎**: redb（嵌入式 B-tree，零外部依赖）

### 5.4 分层记忆

| 层 | 说明 | CLI | 持久化 |
|----|------|-----|--------|
| Ephemeral | 当前会话缓存 | `remember --tier ephemeral` | 进程内 |
| Working | 中期工作上下文 | `remember --tier working` | ✅ 磁盘 |
| LongTerm | 跨会话持久知识 | `remember --tier long-term` | ✅ 磁盘 |
| Procedural | 可复用工作流 | `tool call memory.store_procedure` | ✅ 磁盘 |

**MemoryScope**: `--scope private` (默认) | `--scope shared` | `--scope group:<name>`

### 5.5 Agent 生命周期

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 注册 | `register_agent` | `agent --name X` | 返回 UUID + token |
| 状态查询 | `agent_status` | `agent status --agent X` | Created/Running/Suspended/... |
| 挂起 | `agent_suspend` | `suspend --agent X` | auto-checkpoint |
| 恢复 | `agent_resume` | `resume --agent X` | auto-restore |
| 完成 | `agent_complete` | — | 标记任务完成 |
| 失败 | `agent_fail` | — | 标记任务失败 |
| 终止 | `agent_terminate` | — | 销毁 agent |

### 5.6 Checkpoint / Restore

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 创建检查点 | `agent_checkpoint` | `checkpoint --agent X` | 记忆快照 → CAS CID |
| 恢复检查点 | `agent_restore` | `restore --agent X --cid <CID>` | CAS CID → 记忆恢复 |

`suspend` 自动创建 checkpoint，`resume` 自动恢复。

### 5.7 会话管理

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 开始会话 | `start_session` | `session-start --agent X --intent "..."` | 返回 session_id + warm_context + changes + token_est |
| 结束会话 | `end_session` | `session-end --session <id>` | 返回 delta |
| 增量查询 | `delta_since` | `delta --agent X --since <ts>` | 自上次以来的变更 |

### 5.8 意图系统

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 提交意图 | `submit_intent` | `intent submit "description"` | 结构化意图注册 |
| 声明意图 | `declare_intent` | — | 关键词 + CID + budget |
| 获取组装上下文 | `fetch_assembled_context` | — | 预取结果 |
| 意图反馈 | `intent_feedback` | — | 学习闭环 |

### 5.9 Hook 系统

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 注册 Hook | `hook_register` | `hook register --point X --tool Y --action block` | 5 个拦截点 |
| 列出 Hook | `hook_list` | `hook list` | 查看已注册规则 |

**拦截点**: `PreToolCall`, `PostToolCall`, `PreSessionStart`, `PreWrite`, `PreDelete`
**动作**: `block` (阻止执行) | `log` (仅记录)

### 5.10 上下文预算引擎

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 加载上下文 | `load_context` | `context --cid <CID>` | L0/L1/L2 分层 |
| 意图搜索组装 | — | `context --intent "query" --budget 500` | 搜索 + 预算组装 |
| 显式组装 | `context_assemble` | `context assemble --cids A,B,C --budget 500` | 指定 CID 组装 |

### 5.11 资源可见性

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| Agent 用量 | `agent_usage` | `quota --agent X` | calls / tokens / memory |
| 发现 Agent | `discover_agents` | `discover` | 列出所有 agent + tools |
| 委托任务 | `delegate_task` | `delegate --from A --to B --task "..."` | 跨 agent 任务分发 |

### 5.12 内置工具 (37 个)

通过 `tool call <name> <json_params>` 调用。

| 类别 | 工具 | 说明 |
|------|------|------|
| **CAS** | `cas.create`, `cas.read`, `cas.search`, `cas.update`, `cas.delete` | 内容操作 |
| **Agent** | `agent.register`, `agent.status`, `agent.suspend`, `agent.resume`, `agent.terminate`, `agent.complete`, `agent.fail`, `agent.set_resources` | 生命周期 |
| **Memory** | `memory.store`, `memory.recall`, `memory.store_procedure`, `memory.recall_procedure`, `memory.forget` | 记忆操作 |
| **KG** | `kg.add_node`, `kg.add_edge`, `kg.explore`, `kg.paths`, `kg.get_node`, `kg.list_edges`, `kg.remove_node`, `kg.remove_edge`, `kg.update_node` | 图谱操作 |
| **Message** | `message.send`, `message.read`, `message.ack` | Agent 间通信 |
| **Permission** | `permission.grant`, `permission.revoke`, `permission.list`, `permission.check` | 权限管理 |
| **Context** | `context.load` | 上下文加载 |
| **Tools** | `tools.list`, `tools.describe` | 工具元数据 |

### 5.13 Event Bus

| 操作 | API | CLI | 说明 |
|------|-----|-----|------|
| 订阅 | `event_subscribe` | — | 类型/agent 过滤 |
| 轮询 | `event_poll` | — | 获取新事件 |
| 取消订阅 | `event_unsubscribe` | — | 移除订阅 |
| 历史 | `event_history` | `events history --agent X` | 持久化事件查询 |

**事件类型**: `AgentRegistered`, `AgentStateChanged`, `ObjectStored`, `ObjectDeleted`, `MemoryStored`, `IntentSubmitted`, `ToolCalled`, `PermissionChanged`, `SessionStarted`, `SessionEnded`

---

## 6. 数据存储

### 6.1 存储根

```
~/.plico/                          # 默认根目录
├── objects/                       # CAS 对象 (SHA-256 目录结构)
│   └── af/cf/afcfcb29c3...       # 内容文件
├── meta/                          # 对象元数据 (JSON)
├── graph.redb                     # KG 数据库 (redb B-tree)
├── memory/                        # 分层记忆 (JSON)
├── events/                        # 事件日志 (append-only JSONL)
├── scheduler.json                 # Agent 注册表
├── search_index/                  # BM25/HNSW 索引
├── prefetch_cache.json            # 意图缓存
├── agent_profiles.json            # Agent 行为画像
├── usage.json                     # 资源使用量追踪
└── plico.sock                     # plicod UDS socket
```

### 6.2 存储引擎

| 数据 | 引擎 | 格式 | 特点 |
|------|------|------|------|
| CAS 对象 | 文件系统 | 原始字节 | SHA-256 内容寻址 |
| KG | redb | B-tree | 嵌入式，零外部依赖 |
| 记忆 | 文件系统 | JSON | 按 agent 隔离 |
| 事件 | 文件系统 | JSONL | append-only，段轮转 |
| 搜索索引 | 内存 + 持久化 | HNSW/BM25 | 启动时重建 |

**零 SQL，零外部数据库。** 所有存储都是文件级，可直接 cp/rsync 备份。

---

## 7. 节点演化时间线

| 节点 | 名称 | 核心能力 | 关键实现 |
|------|------|---------|---------|
| N2 | AIOS | CAS + 语义搜索 | `cas/`, `fs/semantic_fs/` |
| N3 | 认知连续 | Agent 经验 + 多租户 | `scheduler/`, `api/` |
| N4 | 协作生态 | 多 Agent + 事件 | `kernel/event_bus.rs` |
| N5 | 自治 | 自进化搜索 | `fs/search/` |
| N6 | 闭合回路 | 每个承诺通上电 | 端到端连接 |
| N7 | 代谢 | 搜索持久化 | `fs/search/` |
| N8 | 驾具 | Prefetch + Context | `kernel/ops/prefetch.rs` |
| N9 | 韧性 | 断路器 + 容错 | `fs/embedding/circuit_breaker.rs` |
| N10 | 正名 | 操作名副其实 | API 清理 |
| N11 | 落地 | 真实交付 | CLI + 测试 |
| N12 | 觉知 | 反思认知 | 可观测性 |
| N13 | 通 | 信号完整 | 通路修复 |
| N14 | 融 | 子系统融合 | KG + 记忆绑定 |
| N15 | 验 | 测试完备 | 全量测试 |
| N16 | 持 | 自持续 | 持久化 |
| N17 | 信 | 操作诚信 | Effect Contracts |
| N18 | 界 | 界面保真 | JSON-First, redb |
| **N19** | **哨** | **Hook + 断路器 (3路径)** | `kernel/hook.rs`, `llm/circuit_breaker.rs` |
| **N20** | **觉** | **预取持久化 + 因果 Hook** | `prefetch_cache.rs`, `causal_hook.rs` |
| **N21** | **意** | **IntentPlan + 自主执行** | `intent.rs`, `intent_executor.rs` |
| **N22** | **行** | **执行即学习** | `ExecutionStats`, learning loop |
| **N23** | **成** | **自主进化** | `skill_discovery.rs`, `self_healing.rs` |
| **N24** | **化** | **超域融合** | `cross_domain_skill.rs`, `goal_generator.rs` |
| **N25** | **太初** | **E2E 收敛 + 完成态** | `convergence.rs`, 本文档 |

---

## 8. 架构红线

| # | 红线 | 含义 | 状态 |
|---|------|------|------|
| R1 | 内核零协议 | MCP/SSE/HTTP/gRPC 只在 `bin/` 接口层 | ✅ |
| R2 | 内核零模型 | 推理/决策/文本生成禁入内核 | ✅ |
| R3 | 内核零自然语言 | NL 解析在接口层，内核只处理结构化请求 | ✅ |
| R4 | 存储与索引分离 | CAS 只管存储，搜索是独立子系统 | ✅ |
| R5 | 身份不可伪造 | Agent token 密码学验证 | ✅ |
| R6 | 记忆 scope 强制 | Private 记忆不可跨 Agent 泄露 | ✅ |
| R7 | 事件日志不可变 | 只追加，不修改，不删除 | ✅ |
| R8 | 协议适配器无状态 | plico-mcp/plico-sse 不缓存不决策 | ✅ |

---

## 9. 反模式清单

| # | 反模式 | 为什么是错的 | Plico 历史 |
|---|--------|-------------|-----------|
| AP-1 | 在内核中解析自然语言 | OS 不应替 Agent 思考 | v3.0 修复 (IntentRouter 提取) |
| AP-2 | 自动存储记忆 | 存储策略是 Agent 的决定 | v3.0 修复 (auto-learning 移除) |
| AP-3 | 为人类设计 Dashboard | Agent 不看网页 | v5.3 废弃 HTTP dashboard |
| AP-4 | 在 OS 中嵌入 LLM 推理 | OS 应模型无关 | 从未犯过 |
| AP-5 | 返回全量数据当默认 | 浪费 token | 持续改进 (L0/L1/L2) |
| AP-6 | 假设 Agent 知道路径 | Agent 描述意图，OS 定位资源 | 持续改进 |
| AP-7 | 无状态请求-响应 | Agent 需要会话连续性 | N3 session 系统 |
| AP-8 | 写兼容性/迁移代码 | 预发布阶段无用户数据 | N18 清理 140+ 行死代码 |

---

## 10. 环境配置

### 10.1 构建

```bash
cargo build               # debug
cargo build --release      # release

cargo test                 # 全量测试 (1388 tests)
```

### 10.2 运行

```bash
# Daemon 模式 (推荐)
plicod --root ~/.plico --port 7878
aicli agent --name my-agent

# Embedded 模式 (测试)
aicli --embedded --root /tmp/test agent --name my-agent

# MCP Server
plico-mcp --root ~/.plico
```

### 10.3 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `PLICO_ROOT` | 存储根目录 | `~/.plico` |
| `EMBEDDING_BACKEND` | 嵌入模型后端 | `local` |
| `EMBEDDING_MODEL_ID` | HuggingFace 模型 ID | `BAAI/bge-small-en-v1.5` |
| `AICLI_OUTPUT` | CLI 输出格式 | `json` |
| `RUST_LOG` | 日志级别 | `info` |
| `PLICO_SUMMARIZER_MODEL` | 摘要模型 | — |
| `PLICO_INTENT_MODEL` | 意图模型 | — |

---

## 11. OS ↔ Agent 契约

### OS 向 Agent 的承诺

- 你的记忆会被持久化，跨越会话和重启
- 你的身份是密码学保证的，不可伪造
- 你请求的上下文会被分层组装，token 成本透明
- 你的资源配额会被严格执行，不允许越界
- 你可以发现其他 Agent 共享的知识
- 你的操作成本会被透明追踪
- 你的会话可以暂停和恢复，上下文不丢失
- 你的工具调用可以被 Hook 拦截，确保安全

### Agent 向 OS 的承诺

- 你的请求是结构化的 `ApiRequest`（JSON）
- 你会携带有效的身份 token
- 你会声明意图，而不是盲目遍历文件树
- 你会主动决定什么值得记忆和共享
- 你会尊重资源配额和权限边界
- 你会在会话结束时通知 OS

---

## 12. 量化指标

| 指标 | 值 |
|------|-----|
| 源代码 | 49,489 行 (131 files) |
| 测试代码 | 16,218 行 (33 files) |
| 测试数 | 1,388 (0 failed) |
| 代码:测试比 | 3:1 |
| API 变体 | 85+ |
| 内置工具 | 37 |
| KG 边类型 | 14 |
| Hook 拦截点 | 5 |
| 记忆层 | 4 |
| 断路器 | 3 (Embedding + LLM + MCP) |
| 设计文档 | 24 (N2-N25) |
| Soul 2.0 对齐度 | 94.7% |
| 架构红线通过 | 8/8 (100%) |

---

*太初是旅程的终点，也是新旅程的起点。*

*本文档生成于 2026-04-24，基于 131 个源文件的客观代码扫描和 26 项真实 CLI 执行测试。*
