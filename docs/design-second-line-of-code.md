# AIOS 第二行代码：设计报告（v2 — 用例驱动修正）

**作者**: plico-dev  
**日期**: 2026-04-19  
**状态**: Draft v2 — 基于具体用例修正  
**v1 修正**: 删除 IntentExecutor（过于武断），修正为资源隔离模型  

---

## 0. 摘要

v1 报告通过类比推导出"Agent 执行运行时"作为第二行代码，提出 `IntentExecutor` trait 定义 Agent 如何思考和行动。**这是错误的。**

本次修正通过将两个真实 AI Agent（OpenClaw、Claude Code）"放到 Plico 上运行"的思想实验，发现：

> **Agent 自带执行模型。OS 不应定义"如何思考"，而应提供"思考所需的资源"。**

修正后的结论：
1. ~~IntentExecutor~~（删除）——Agent 自带 ReAct/QueryEngine/Gateway，不需要 OS 定义执行循环
2. **第二行代码是 Agent 隔离与资源模型**——让多个 Agent 共存、共享知识、但互不干扰
3. **AIOS 的独特价值不是执行，而是共享语义基底**——跨 Agent 的知识发现与共享

---

## 1. 具体用例：两个真实 Agent 如何使用 AIOS

### 1.1 OpenClaw（2026 年 GitHub 最火开源项目，165K+ Stars）

**它是什么**：自托管多通道 AI 网关，通过 WhatsApp/Telegram/Discord 接入，本地运行。

**它自带什么**：
| 能力 | OpenClaw 自带实现 |
|------|-----------------|
| 执行模型 | Gateway → Agent → ReAct 循环 → Skills → 沙箱执行 |
| 记忆管理 | SQLite + FTS5 + sqlite-vec，三级记忆（MEMORY.md/日志/会话） |
| 工具系统 | Skills（SKILL.md YAML frontmatter），51+ 官方插件 |
| 模型访问 | 多 Provider 配置（Anthropic/OpenAI/Ollama/本地），自管连接 |
| 上下文管理 | 自有上下文拼装逻辑 |
| 权限控制 | TOOLS.md 白名单/黑名单，沙箱隔离 |

**它需要 OS 提供什么**：
| 资源 | 为什么需要 |
|------|-----------|
| 文件系统 | workspace 目录、记忆文件、会话存档 |
| 进程执行 | 启动临时 Python 进程运行 Skills |
| 网络 | WebSocket 长连接，多通道接入 |
| 数据库 | SQLite 存元数据和向量 |

**它不需要 OS 提供什么**：
- ❌ Agent 循环——它自带 Gateway-Agent-ReAct
- ❌ 工具定义——它自带 Skills 系统
- ❌ 记忆框架——它自带三级记忆
- ❌ 模型调度——它自管 LLM Provider 连接

### 1.2 Claude Code（Anthropic 官方 CLI Agent，51 万行 TypeScript）

**它是什么**：终端内运行的 AI 编程 Agent，可读写文件、执行命令、操作 MCP。

**它自带什么**：
| 能力 | Claude Code 自带实现 |
|------|-------------------|
| 执行模型 | REPL → QueryEngine → Think → Tool → Observe → 循环 |
| 记忆管理 | SessionMemory + Dream 系统 + Compact/MicroCompact |
| 工具系统 | 40+ 内建工具（Read/Write/Bash/Grep/Agent/MCP...） |
| 模型访问 | Anthropic API 直连，流式传输，token 预算管理 |
| 上下文管理 | AutoCompact + MicroCompact + PTL 紧急截断 |
| 多 Agent | Swarm 模式，tmux 多进程，独立 QueryEngine 实例 |

**它需要 OS 提供什么**：
| 资源 | 为什么需要 |
|------|-----------|
| 文件系统 | 读写代码文件，Git 操作 |
| Shell | 执行命令，构建测试 |
| 进程管理 | 子进程、tmux、Git worktree |
| 网络 | API 调用，MCP 服务器 |

**它不需要 OS 提供什么**：
- ❌ QueryEngine——它自带
- ❌ 上下文压缩——它自带 Compact/MicroCompact
- ❌ 工具系统——它自带 40+ 工具
- ❌ 模型调用——它直连 Anthropic API

### 1.3 关键发现

> **两个最成功的 AI Agent 都自带完整的执行模型、记忆管理、工具系统和模型连接。**
> **它们需要的是底层资源，不是执行框架。**

---

## 2. Unix 类比纠正

### 2.1 v1 的错误类比

v1 说："Agent 像没有 CPU 的进程。需要 IntentExecutor 作为 AI CPU。"

**这是错的。** 更准确的类比：

- OpenClaw 像 Apache HTTPD——自带事件循环、请求处理、模块系统
- Claude Code 像 Vim——自带编辑模型、命令系统、插件框架
- **它们不需要 OS 告诉它们怎么运行，它们需要 OS 提供运行环境**

### 2.2 正确的类比

Unix 提供什么给 Apache/Vim？

| Unix 提供 | Apache/Vim 使用方式 | AIOS 对应 |
|-----------|-------------------|-----------|
| 进程隔离 | 各自独立运行，互不干扰 | Agent 隔离（记忆/工具/权限） |
| 文件系统 | 读写配置、日志、数据 | CAS + SemanticFS |
| 虚拟内存 | 各自独立的地址空间 | 各自独立的记忆空间 |
| IPC（pipe/socket） | 进程间通信 | Agent 消息传递 |
| rlimit | 资源配额 | ExecutionBudget |
| 用户/权限 | 访问控制 | Agent 权限模型 |

**Unix 不提供什么**：
- ❌ 不告诉 Apache 怎么处理 HTTP 请求
- ❌ 不告诉 Vim 怎么编辑文件
- ❌ 不定义应用层协议（HTTP/SSH）

**类推：AIOS 不应提供什么**：
- ❌ 不告诉 Agent 怎么推理（不需要 IntentExecutor）
- ❌ 不告诉 Agent 怎么管理上下文（Agent 自带）
- ❌ 不定义通信协议（MCP/A2A 是接口层）

---

## 3. 那 AIOS 的独特价值是什么？

传统 OS 跑 Apache 和 Vim，它们通过文件系统共享数据——Apache 写日志，Vim 读日志。但文件系统只提供路径和字节，不理解内容。

**AIOS 的独特价值是：跨 Agent 的语义知识共享。**

### 3.1 场景：OpenClaw + Claude Code 共存于 Plico

```
用户对 OpenClaw 说（via Telegram）:
  "我正在做一个 Rust 项目，记住我喜欢 snake_case"
  
OpenClaw → Plico:
  plico remember --content "用户偏好：Rust 项目使用 snake_case"
                 --tags "user:preference,lang:rust"

... 一小时后 ...

Claude Code 编辑代码时 → Plico:
  plico recall --query "用户的代码风格偏好"
  
Plico 返回:
  "用户偏好：Rust 项目使用 snake_case"（由 OpenClaw 存储）

Claude Code 据此生成 snake_case 风格代码 ✓
```

**传统 OS 做不到这件事**——Apache 写的日志 Vim 可以读，但 Vim 不知道日志里有什么有用信息。
**AIOS 可以做到**——因为存储是语义化的，搜索是基于意义的，知识是跨 Agent 可发现的。

### 3.2 更深层场景：知识溢出效应

```
Agent A（数据分析）:
  发现"用户公司的季度报告总是在每月15号生成"
  → plico put --content "季度报告生成日期：每月15号" 
             --tags "company:schedule,plico:type:fact"

Agent B（邮件助手）:
  用户说"帮我准备季度汇报的邮件"
  → plico search "季度报告相关信息"
  → 找到 Agent A 存的事实
  → 知道在15号之后才有数据，提醒用户等待

Agent C（日历助手）:
  → plico search "定期生成的报告"
  → 自动在每月16号创建"审阅季度报告"提醒
```

**没有任何一个 Agent 被"教"过这些——知识通过语义基底自然流动。**

这就是 AIOS 的"文件系统效应"——Unix 的文件系统让程序无需相互了解就能交换数据。AIOS 的语义基底让 Agent 无需相互了解就能共享知识。

---

## 4. 修正后的"第二行代码"

### 4.1 链式推导（修正版）

```
CAS 解决了"数据如何存在"（第一行代码）
  → 但多个 Agent 的数据混在一起 → 需要隔离
    → Agent 记忆命名空间（每个 Agent 有独立的记忆空间）
      → 但隔离太强则无法共享 → 需要跨 Agent 发现机制
        → 语义搜索 + 标签约定（跨 Agent 知识发现）
          → 但资源是有限的 → 需要配额
            → Agent 资源模型（存储/搜索/模型调用配额）
              → 完整图景：Agent 隔离 + 共享知识 + 资源管理
```

### 4.2 第二行代码 = Agent 命名空间 + 资源模型

不是 IntentExecutor（定义"怎么思考"），而是：

```rust
/// Agent 的资源命名空间——类比 Unix 进程的 uid + 地址空间 + rlimit
pub struct AgentNamespace {
    pub agent_id: AgentId,
    
    // 记忆隔离：每个 Agent 有私有记忆空间
    // 类比 Unix 进程的虚拟地址空间
    pub private_memory: MemoryScope,  // 只有自己能读写
    pub shared_memory: MemoryScope,   // 所有 Agent 可读，写入者标记来源
    
    // 工具权限：每个 Agent 能使用哪些工具
    // 类比 Unix 用户的文件权限
    pub tool_permissions: ToolScope,
    
    // 资源配额：防止一个 Agent 耗尽系统资源
    // 类比 Unix rlimit
    pub budget: ResourceBudget,
}

/// 资源配额
pub struct ResourceBudget {
    pub max_storage_bytes: u64,       // CAS 存储上限
    pub max_search_per_hour: u32,     // 搜索频率限制
    pub max_memory_entries: u32,      // 记忆条目上限
    pub max_kg_nodes: u32,            // KG 节点上限
}

/// 记忆范围
pub enum MemoryScope {
    /// 私有：只有该 Agent 可读写
    Private(AgentId),
    /// 共享：所有 Agent 可读，写入时标记来源 Agent
    Shared,
    /// 组：只有同组 Agent 可访问
    Group(String),
}
```

### 4.3 与 v1 的关键差异

| 方面 | v1 设计（已否决） | v2 设计（修正） |
|------|-----------------|---------------|
| 核心原语 | IntentExecutor（执行引擎） | AgentNamespace（资源隔离） |
| 哲学 | OS 定义 Agent 怎么思考 | OS 提供资源，Agent 自己决定怎么思考 |
| 类比 | OS 内置应用逻辑 | OS 提供进程隔离 + 文件系统 |
| 模型调度 | ModelScheduler（OS 分配模型） | Agent 自管 LLM 连接（OS 可选提供） |
| 工具 | OS 驱动工具调用 | Agent 自己调用，OS 管理权限 |
| 适配性 | 只适合遵循 ReAct 的 Agent | 适配任何执行模型的 Agent |

---

## 5. 具体设计：AIOS 如何服务 OpenClaw

### 5.1 OpenClaw 接入 Plico 的方式

```
OpenClaw (Node.js)
    │
    ├─ 自有: Gateway, Channels, ReAct Loop, Skills
    │
    └─ 通过 plico-mcp 接入 Plico ←── 这是唯一的连接点
         │
         ├─ plico_put    → 存储对话知识、用户偏好
         ├─ plico_search → 搜索历史知识（包括其他 Agent 存的）
         ├─ plico_read   → 读取特定内容
         ├─ plico_tags   → 发现知识分类
         ├─ plico_skills_list → 查看 OS 级可用技能
         └─ plico_skills_run  → 复用其他 Agent 学到的程序记忆
```

OpenClaw 把 Plico 当作**外部知识库**使用，不是当作运行环境。Plico 不干涉 OpenClaw 的执行——就像 PostgreSQL 不干涉应用怎么用查询结果。

### 5.2 OpenClaw 从 Plico 获得的独特价值

1. **跨通道知识持久化**：用户在 Telegram 说的话，Plico 存为语义知识，用户换到 Discord 时可找回
2. **跨 Agent 知识**：Claude Code 学到的代码模式，OpenClaw 可以在对话中引用
3. **程序记忆复用**：OpenClaw 学到的工作流（如"部署三步骤"），其他 Agent 可通过 `plico_skills_run` 复用
4. **知识图谱导航**：OpenClaw 可以 explore 用户知识的关联关系

---

## 6. 具体设计：AIOS 如何服务 Claude Code

### 6.1 Claude Code 接入 Plico 的方式

```
Claude Code (TypeScript/Bun)
    │
    ├─ 自有: REPL, QueryEngine, 40+ Tools, Compact, Swarm
    │
    └─ 两种接入方式:
         │
         ├─ 方式A: MCP Server（Claude Code 原生支持 MCP）
         │    └─ plico-mcp 作为 MCP Server 注册到 Claude Code
         │
         └─ 方式B: CLI 工具（Claude Code 可执行 bash 命令）
              └─ 直接调用 aicli put/search/recall/skills
```

### 6.2 Claude Code 从 Plico 获得的独特价值

1. **跨会话知识持久化**：这次 session 学到的代码模式，下次 session 可以 recall
2. **语义代码搜索**：不只是 grep 关键词，而是"找到关于认证流程的代码"
3. **开发知识积累**：ADR、经验、进度都语义化存储，持续增长
4. **协作记忆**：多个 Claude Code 实例（Swarm workers）通过 Plico 共享发现

---

## 7. 与灵魂重新对齐

### 7.1 灵魂说的是什么

灵魂（system.md）描述的四大内核组件：

1. **智能体调度器**："负责多个 AI Agent 的生命周期管理（创建、暂停、恢复、销毁），并根据任务优先级、资源可用性等因素，智能地分配计算、模型和工具资源"
2. **分层内存管理**："专为 AI 工作负载设计的'记忆'系统"
3. **模型与工具运行时**："负责加载、运行和卸载本地及云端的各类 AI 模型，并将外部工具作为'技能'供 Agent 动态调用"
4. **权限与安全护栏**："对所有 Agent 操作进行细粒度权限控制"

### 7.2 灵魂的关键词解读

注意灵魂说的是"**分配**计算、模型和工具资源"，不是"**执行**计算"。

这与 v2 设计完全一致：
- "分配资源" = AgentNamespace + ResourceBudget（v2）
- "执行计算" = IntentExecutor（v1，已否决）

灵魂的"模型与工具运行时"也是说"**加载、运行和卸载**"模型——这是资源管理，不是应用逻辑。就像 Linux 的模块加载器 `insmod/rmmod`——它管理模块的加载卸载，不管模块内部怎么工作。

### 7.3 修正后的灵魂对齐

| 灵魂组件 | 当前实现 | v2 补足 |
|----------|---------|--------|
| 智能体调度器 | 状态机 + 意图队列 ✅ | + AgentNamespace（隔离） |
| 分层内存管理 | 4层完整 ✅ | + MemoryScope（私有/共享） |
| 模型与工具运行时 | LlmProvider + ToolRegistry ✅ | + 可选的 ModelPool（Agent 不必须用） |
| 权限与安全护栏 | 基本权限 ✅ | + ResourceBudget + ToolScope |

---

## 8. 修正后的里程碑链

### 8.1 从 v2.2-M11 推导

```
v2.2-M11: Procedural Skills（已完成）
  → Skills 可以存储和跨 Agent 共享
    → 但 Agent 之间没有隔离 → 任何 Agent 读写任何记忆
      → v3.0-M1: MemoryScope（私有/共享记忆命名空间）
        → 记忆隔离了 → 但工具权限没有
          → v3.0-M2: ToolScope（Agent 级工具权限）
            → 隔离完成 → 但没有资源限制
              → v3.0-M3: ResourceBudget（存储/搜索配额）
                → 隔离 + 配额完成 → 验证
                  → v3.0-M4: 多 Agent E2E
                      OpenClaw via MCP + Claude Code via CLI
                      共存、隔离、知识共享
```

### 8.2 里程碑详情

| 里程碑 | 目标 | 关键交付 |
|--------|------|---------|
| v3.0-M1 | MemoryScope | 记忆命名空间：private/shared/group，写入标记来源 Agent |
| v3.0-M2 | ToolScope | Agent 级工具白名单/黑名单，权限继承 |
| v3.0-M3 | ResourceBudget | 存储/搜索/KG 配额，超限拒绝 |
| v3.0-M4 | 多 Agent E2E | 两个 Agent 共存 demo：隔离正确、知识可共享 |

### 8.3 ModelPool 的定位（可选，不阻塞）

v1 将 ModelScheduler 作为核心组件。v2 将其降级为**可选**：
- Agent（如 OpenClaw、Claude Code）自管 LLM 连接 → 不需要 OS 的 ModelPool
- 简单 Agent（如自定义脚本）可能没有自己的 LLM 管理 → 可以用 OS 的 ModelPool
- ModelPool 作为**便利层**存在，不是**必须层**

这类似 Unix 的 `/dev/null`——有用，但不是 OS 的核心设计。

---

## 9. 自我批判与开放问题

### 9.1 v2 可能的弱点

1. **Agent 是否真的不需要执行引擎？** 对于 OpenClaw/Claude Code 这样的成熟 Agent 不需要。但对于"写在 Plico 上的原生 Agent"可能需要。未来可以作为 **用户空间库** 提供（不是内核）。

2. **ModelPool 真的是可选的吗？** 如果 10 个 Agent 同时请求同一个本地 Ollama 实例，需要排队。这时 ModelPool 变得重要。但它应该是**资源管理器**（像 cgroups），不是**执行引擎**。

3. **语义基底够用吗？** Agent 之间的知识共享依赖标签约定。如果标签不一致，Agent A 存的东西 Agent B 找不到。可能需要**标准标签本体**（如 Plico 的 `plico:type:*` 约定）。

### 9.2 一句话总结

> **AIOS 的第一行代码是 CAS——让数据存在。**
> **第二行代码是 Agent Namespace——让多个智能体在共享语义基底上共存、隔离、协作。**
> **不是"如何思考"，而是"在哪里思考"。**

---

## 附录 A：OpenClaw 架构参考

```
四层架构：
  ① 交互层 Channels (Telegram/WhatsApp/Discord/...)
  ② 网关层 Gateway (路由/排队/调度/鉴权)  
  ③ 智能体层 Agent (LLM推理/ReAct/记忆/Skills)
  ④ 执行层 Execution (本地节点/远端节点/沙箱)

三级记忆：
  MEMORY.md (长期) + memory/YYYY-MM-DD.md (短期) + sessions/ (会话)
  底层: SQLite + FTS5 + sqlite-vec

Skills 系统：
  SKILL.md (YAML frontmatter) + 沙箱执行
  优先级: workspace > ~/.openclaw/skills > 内置
```

## 附录 B：Claude Code 架构参考

```
运行时架构：
  接入层: CLI / IDE Bridge / MCP Server / Agent SDK
  交互层: REPL + 命令路由 + 权限管理
  核心层: QueryEngine (stateful) → ask() (stateless stream)
  工具层: 40+ 工具，统一接口 (call/permission/classification)
  编排层: SubAgent + Swarm (tmux) + 独立 QueryEngine 实例

上下文管理：
  MicroCompact (增量) → AutoCompact (阈值) → Manual → PTL (紧急)

Agent 循环：
  1. 组装上下文 (system + tools + history + CLAUDE.md)
  2. 检查预算
  3. ask() → 流式 API 调用
  4. 收到 tool_use → 权限检查 → 执行 → tool_result
  5. 再次 ask() 直到模型结束
  6. Compact + 推测执行
```

---

*本报告 v2 由具体用例驱动修正：将 OpenClaw 和 Claude Code 作为试金石，验证 AIOS 设计是否真正服务于真实 Agent 的需求，而非构建空中楼阁。*
