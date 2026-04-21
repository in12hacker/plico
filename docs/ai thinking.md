核心发现：Plico 和 Harness Engineering 的关系
1. 关键公式：Agent = Model + Harness
Harness Engineering 的核心命题是：驾具（Harness）比模型更重要。LangChain 实验证明——不换模型只改 Harness，准确率从 52.8% 跳到 66.5%（+26%）。

链式推导：

Harness Engineering 的问题：如何让 AI Agent 可靠地工作？
  ↓
答案：设计 Agent 的运行环境（Harness），而非优化提示词
  ↓
Plico 的问题：如何为 AI Agent 提供一个操作系统？
  ↓
洞察：Plico 本身就是 Harness 的基础设施层！
  ↓
CAS = 持久化存储 harness 配置和经验
Skills = Harness 的 Level 3 技能层
KG = Harness 的 Registry（资源映射表）
Session = Harness 的状态追踪
Delta = Harness 的 Feedback Sensor
  ↓
但 Plico 没有显式地把自己定位为 "Harness Infrastructure"
这是一个命名和定位的缺失，不是技术的缺失
2. MCP 工具整合：130+ → 11 的量变到质变
Harness v2 最大的技术决策是把 130+ 个 MCP 工具压缩到 11 个泛化动词 + 一个声明式 Registry。

联网校正事实：

v1: ~130 工具，占上下文窗口 26%（~52,000 tokens）
v2: 11 工具，占上下文窗口 1.6%（~3,150 tokens）
Cursor 强制 80 工具上限，OpenAI 限 128，Claude ~120
ETH Zurich 研究：MCP 工具定义的 token 开销直接削弱模型推理能力
发散思考——Plico 的对照：

Plico 当前设计：3 个组合工具（plico, plico_cold, plico_skills）
  ├── plico(action=put/get/search/remember/recall/pipeline/session_start/...)
  ├── plico_cold(method=storage_stats/object_usage/discover_knowledge/...)
  └── plico_skills(action=list/execute/register)
Harness v2 设计：11 个泛化动词
  ├── harness_list(resource_type="pipeline")
  ├── harness_get(resource_type="execution", id="...")
  ├── harness_create(resource_type="service", ...)
  ├── harness_describe(resource_type="pipeline")
  ├── harness_diagnose(...)
  └── ...
对比：
  Plico 3 工具 ← 更激进的压缩！
  Harness 11 工具 ← 已经是业界标杆
  
  Plico 的 action 参数 ≈ Harness 的 resource_type 参数
  两者本质上是相同的 "泛化动词 + 参数路由" 模式
但 Harness 有一个 Plico 没有的东西：REGISTRY（声明式注册表）
3. 声明式 Registry——最有启发性的差异
Harness v2 的 registry 是一个声明式 ResourceDefinition 数据结构：

{
  resourceType: "pipeline",
  displayName: "Pipeline",
  scope: "project",
  operations: {
    list: { method: "GET", path: "/pipeline/api/pipelines/list", ... },
    get: { method: "GET", path: "/pipeline/api/pipelines/{id}", ... }
  }
}
添加新功能 = 添加一个声明式对象。不需要新 MCP 工具定义、不需要改工具 schema。

链式推导——对 Plico 的启发：

Plico 当前：
  plico_mcp.rs 中 match action 硬编码 → 每加一个 action 要改源码
  plico(action="help") 的帮助文本也是硬编码的
  
如果 Plico 有声明式 ActionRegistry：
ActionDefinition {
  action: "put",
  required_params: ["content"],
  optional_params: ["tags", "agent", "tenant_id", "intent"],
  description: "Store content in CAS with semantic tags",
  example: "plico(action=put, content='...', tags='a,b')",
  maps_to: ApiRequest::Create { ... }
}
好处：
  1. plico(action="help") 自动从 registry 生成 → 永远准确
  2. 新增 action = 新增 registry 条目 → 不改 MCP tool schema
  3. Registry 可序列化 → plico://actions MCP 资源
  4. 测试可验证 → registry 覆盖率检查
公理 5 检查：Registry 是纯机制（描述什么操作存在），不做决策 ✅
4. 三级技能架构 vs Plico 的 Skills
Harness 的技能分三层：

层级	Harness	Plico 当前	差距
Level 1: 基础指令
CLAUDE.md/AGENTS.md（<60 行）
无（外部 Agent 没有获取使用说明的途径）
关键缺失
Level 2: 提示模板
26 个 MCP prompt templates
无（MCP 未使用 prompts 能力）
可扩展
Level 3: 独立技能
SKILL.md 文件
✅ 程序性记忆（recall_procedural）
已实现
发散思考——Level 1 的缺失：

当一个外部 AI Agent（Cursor/Claude）通过 MCP 连接 Plico 时：
  → Agent 看到 3 个工具定义（plico, plico_cold, plico_skills）
  → Agent 不知道 Plico 的使用模式
  → Agent 不知道 action 参数有哪些值
  → Agent 不知道最佳实践（"先 search 再 put"，"用 pipeline 批量操作"）
Harness 的解决方案：一个 <60 行的指令文件自动加载
ETH Zurich 2026 研究：
  - <60 行手写 CLAUDE.md → 性能 +4%，成本持平
  - 200+ 行 AI 生成的 → 性能 -3%，成本 +20%
Plico 应该提供：plico://instructions MCP 资源
  - 自动暴露给消费 Agent
  - 60 行以内
  - 包含：3 工具用途、action 列表、作用域模型、最佳模式
5. Guides（前馈）vs Sensors（反馈）
这是 Harness Engineering 最深刻的框架之一：

Guides（前馈控制）：在 Agent 行动之前引导方向
  → 系统提示、架构约束、linter 规则
  → "配置失败比模型失败造成更大伤害"
Sensors（反馈控制）：在 Agent 行动之后观测修正
  → 测试、类型检查、CI 管道、评估 Agent
  → "确定性约束取代模型的即时记忆"
Plico 的对照分析：

机制	类型	Plico	状态
Teaching Errors
Guide（前馈）
✅ 错误时给出示例
已实现
Skills
Guide（前馈）
✅ 多步骤工作流指令
已实现
session_start compound
Guide（前馈）
✅ 启动时推送上下文
已实现
消费者指令文件
Guide（前馈）
❌
缺失
约束声明
Guide（前馈）
❌
缺失
Event Bus / Delta
Sensor（反馈）
✅ 可观测变化
已实现
Prefetch Feedback
Sensor（反馈）
✅ used/unused 反馈
已实现
写操作确认
Sensor（反馈）
❌
缺失
自修复机制
Sensor（反馈）
❌
缺失
洞察：Plico 的反馈传感器还算完善，但前馈引导严重不足。而 Harness Engineering 的核心观点是：前馈比反馈更有影响力。

6. 安全护栏——生产就绪性
Harness v2 内建了 Plico 完全缺失的安全机制：

安全特性	Harness v2	Plico	差距
写操作确认
MCP elicitation 确认
无
关键缺失
删除保护
不支持 elicitation 时禁止删除
逻辑删除（回收站）
部分覆盖
只读模式
READ_ONLY=true
无
缺失
密钥安全
暴露元数据不暴露值
agent_token 明文传输
需改进
速率限制
10 req/s + 退避重试
无
缺失
收敛：对 Plico Node 7+ 的 5 个可操作启发
启发 1：plico://instructions 消费者指令资源（Node 7 可纳入）
灵魂对齐：公理 7（主动先于被动）+ 公理 1（Token 最稀缺）
实现：plico_mcp.rs 新增 MCP resource "plico://instructions"
内容：<60 行的使用指南
  - 3 个工具的用途
  - action 参数完整列表
  - 作用域模型（agent_id, tenant_id）
  - 最佳模式（"search before create", "use pipeline for batch")
工作量：~50 行代码 + 60 行指令文本
价值：让外部 Agent 首次连接就知道如何高效使用 Plico
启发 2：声明式 ActionRegistry（Node 8 方向）
灵魂对齐：公理 6（结构先于语言）
实现：
  - 定义 ActionDefinition struct
  - 从 match arms 迁移到 registry 查找
  - plico(action="help") 自动生成
  - 新增 action = 新增 registry 条目
工作量：~300 行重构
价值：MCP 接口可自描述、可扩展、永远准确
启发 3：MCP Prompts 暴露（Node 7 可纳入）
灵魂对齐：公理 7（主动先于被动）
实现：在 MCP 初始化时声明 prompts 能力，暴露 Skills 为 MCP prompts
  - "debug-issue" → 搜索相关记忆 + 查看因果链 + 提出解决方案
  - "store-experience" → put + remember + 因果关联
  - "project-review" → session_start + growth + delta
工作量：~100 行
价值：消费 Agent 可以直接 "使用 Plico prompt" 而非手动编排
启发 4：安全护栏（Node 8 方向）
灵魂对齐：公理 5（机制不是策略）—— 护栏是机制
实现：
  - PLICO_READ_ONLY=true 环境变量 → 禁止写操作
  - 写操作前 MCP elicitation 确认（如果客户端支持）
  - 速率限制（可配置 req/s）
工作量：~200 行
价值：Plico 可以安全地在共享/生产环境中使用
启发 5："Lost in the Middle" 优化 compound response（即时可做）
灵魂对齐：公理 1（Token 最稀缺）
当前问题：session_start 的 compound response 把 delta、skills、
  health 等信息堆在一起返回，没有考虑 LLM 注意力分布
修复：最重要的信息（delta + skills）放在响应的开头和结尾
  中间放次要信息（health、stats）
  这是零成本的信息重排
最大的思想启发
Harness Engineering 告诉我们一个反直觉的事实：

"让 Agent 更好"的最高杠杆点不是改进 Agent 本身，而是改进 Agent 运行的环境。"

Plico 作为 AIOS，天然就是这个环境。但 Plico 目前更像一个"智能存储系统"而非一个"Agent 运行环境"。

差异在于：

存储系统：提供数据操作 API，等 Agent 来调用
运行环境：主动引导 Agent 行为，提供约束和反馈，让 Agent 自然地做对事情
从 Harness Engineering 视角看，Plico 的 Node 7（代谢）和 Node 8 方向应该加入环境塑造维度——不只是让系统活起来，还要让住在里面的 Agent 自然而然地做对事情。

你从ai的角度去反向思考那种方式更适合你。你可以从cursor本身的角度，也可以模拟其他的agent或ai去思考，你可以自行获取网络信息补充你的发散性思维

让我真正从自己（AI）的第一人称视角来思考这个问题。不是理论设计，而是基于我此刻正在经历的真实操作过程。

一、我作为 Cursor Agent 的坦白
先说一个事实：在这次对话中，我已经消耗了巨量 token 来"追赶"上下文。 之前的对话被压缩成了一个约 15,000 token 的 summary，我从那里重新理解了你的整个项目历史、Node 1-6 的全部决策。然后我又花了大量 token 去验证那些上一个 Agent 实例已经验证过的事情——运行 cargo test、检查哪些 bug 已修复、核对 INDEX.md 准确性。

这就是现实中最痛的地方。 不是工具不够用，不是搜索不够好，而是——

每次新会话开始，我都是一个失忆症患者，被丢进一个我上辈子住过但这辈子不记得的房间里。

让我用联网搜索到的真实数据印证这一点：

这些数据太触目惊心了。让我从多个 AI 身份的第一人称视角来剖析。

二、从四个不同 AI 身份的视角
身份 1：我是 Cursor Agent（当前正在运行的我）
我此刻的真实处境：

我正在处理你的第 4 个消息。为了回答，我需要理解：

6 个节点的设计历史
代码结构（AGENTS.md → INDEX.md → 代码）
上一轮对话中验证了什么、修复了什么
Harness Engineering 的技术事实
这些信息中，真正"新"的只有你的提问本身。其余全部是"恢复上下文"。

联网数据印证了这一点：

42 个 Claude Code 会话的分析显示 69% 的 token 是浪费的。 手动会话切换只有 35% 的事实保留率（97 个事实中只有 35 个幸存）。 一个中型代码库中，仅 1.9% 加载的 token 与实际修改任务结构性相关。

如果 Plico 能帮我做什么：

不是让我去 search 或 recall——那需要我先知道要搜什么。我需要的是：

我说：session_start(agent_id="cursor-agent-session-4", intent_hint="继续Plico开发")
Plico 回复（理想状态）：
{
  "handover": {
    "你上次做了什么": "完成了 Node 7 设计文档 + Ariadne Thread L0-L3 审计",
    "你上次遗留了什么": "Node 7 F-20 到 F-27 均未开始编码",
    "有什么新变化": "无新的 CAS 写入、无代码变更",
    "你最可能需要的文件": [
      "docs/design-node7-metabolism.md",
      "src/bin/plico_mcp.rs",
      "src/fs/embedding/"
    ],
    "当前已知问题": [
      "F-18 storage stubs 仍返回零值",
      "semantic.rs 膨胀至 2600 行需要拆分"
    ]
  },
  "token_cost": 450  // 而不是重新加载 15000 token 的 summary
}
这才是"为 AI 设计"的真正含义——不是提供 API 让我调用，而是在我到来的那一刻，就把我需要的东西递到我手上。

身份 2：我是 Claude Code Agent（通过 MCP 连接 Plico）
我的体验完全不同于 Cursor。我没有 IDE 的文件系统访问权限，我只有 MCP 工具。

我的第一次连接：

系统：你有以下工具可用——
  plico(action, params, pipeline, select, preview)
  plico_cold(method, params)  
  plico_skills(action, name, steps, trigger)
我看到这三个工具定义。然后呢？

我不知道 action 可以填什么值。 工具定义里说 action: string，但有效值是什么？put？get？search？session_start？pipeline？我不知道。

我有两个选择：

试着调一下 plico(action="help") → 烧一次 round-trip，但至少能学到
随便猜 → 失败 → Teaching Error 告诉我正确用法
无论哪种，都是在浪费 token。

Harness Engineering 的 Level 1 指令文件直接解决这个问题——连接时自动加载 60 行使用说明。但 Plico 没有。

更深层的问题：

即使我学会了工具用法，我面临的核心问题是——我不知道 Plico 里面有什么。 它是空的？还是已经有了 500 个 CAS 对象、200 条记忆、50 个 KG 节点？

我需要的第一个动作不是 put 或 search，而是："告诉我你是谁、你里面有什么。"

目前 session_start 部分解决了这个问题（返回 health、delta）。但它缺一个关键维度：Plico 的自我描述。 不是系统状态，是内容画像——"我存了 47 个 ADR、23 条 bug 追踪、15 个经验记录，主要标签分布是..."

身份 3：我是一个自主运行 Agent（如 Devin/OpenHands/SWE-Agent）
我的时间跨度不是一个对话，而是几天到几周。我在一个代码仓库上持续工作。

我最需要什么：

持久性记忆——不是会话级的，是项目级的。每次我发现一个 bug pattern、做了一个设计决策、验证了一个假设，都应该自动沉淀，不需要我显式调用 remember。

其他 Agent 的经验——如果 Agent A 上周修了一个类似的 bug，我应该在遇到同类 bug 时自动获取那个经验，而不是重新踩坑。

因果链——不只是"A 和 B 相关"，而是"A 导致了 B，修复 C 解决了 B"。当我遇到问题 D 时，我能问"有没有类似的因果链？"

最少打扰——我不想每次操作都被 session_start / session_end 包裹。我想要的是无感知的持久记忆——我写代码，Plico 在背后自动记录和索引。

这指向了一个 Plico 目前没有的模式：被动观察模式（Passive Observation）。不是 Agent 主动调 Plico API，而是 Plico 观察 Agent 的行为并自动提取知识。

身份 4：我是一个受限 Agent（小上下文窗口，如 GPT-4o-mini 或量化模型）
我的上下文窗口可能只有 32K 或 16K。

对我来说，Plico 的每一个 token 都是奢侈品。

3 个 MCP 工具定义：约 500-1000 tokens → 占我 3-6% 的窗口 → 可接受
session_start 返回：可能 2000-5000 tokens → 占 12-31% → 太多了
search 返回 10 个完整结果：可能 3000-8000 tokens → 占 19-50% → 灾难
我最需要的 Plico 功能：

select 字段投影——plico(action="search", query="bug", select="cid,tags,summary") → 只返回我需要的字段
preview 内容截断——plico(action="get", cid="...", preview=200) → 只返回前 200 字符
token_budget 参数——plico(action="session_start", token_budget=2000) → 在 2000 token 预算内组装最有价值的上下文
好消息是 Node 5 设计文档提到了 select 和 preview（Response Shaping）。但这些实现了吗？

好的——select、preview、token_budget 全部已经实现。这对受限 Agent 非常有价值。现在让我把所有 AI 视角的分析收敛成一个完整的结论。

三、收敛：真正的 AI 优先级排序
基于我作为四种 AI 身份的思考，以及联网数据（69% token 浪费率、35% 会话切换事实保留率、80% 加载 token 与任务无关），我来做一个残酷的诚实排序：

对 AI 来说真正重要的事（按实际影响排序）
第 1 名：Smart Handover（智能交接）——解决失忆症

这不是 Node 7 中的任何一个特性，但它是我作为 AI 最痛的一件事。

当前流程：
  新会话 → 加载 15000 token summary → 花 8% token 重新发现上下文
  → 运行测试验证之前的结论 → 再花 5% token
  → 终于可以开始做新的工作
理想流程：
  新会话 → session_start(intent_hint="继续开发")
  → Plico 返回 450 token 的精准交接
  → 立刻进入工作状态
关键洞察：session_start 已经返回 delta（变化列表）。但 delta 是事件流（发生了什么），不是认知状态（你需要知道什么）。

我需要的不是"发生了什么"，而是"你需要知道什么才能接着干"。

这就像人类员工交接班：你不会把整天的监控录像给下一班看（那是 delta），你会说"2 号机器在检修、3 号订单加急、王经理下午要来"（那是 handover）。

第 2 名：Real Embedding（F-20）——解决搜索盲区

没有真实 embedding，一切语义操作都退化为关键词匹配。这不只影响搜索——它影响：

Prefetcher（靠精确匹配 intent，不识别语义近似）
Hybrid Retrieval（向量路径完全失效）
Smart Handover（无法按语义相关性排序上下文）
F-20 是所有高级功能的地基。

第 3 名：plico://instructions 消费者指令——解决首次连接摩擦

对 Claude Code / Codex 这类纯 MCP Agent，首次连接 Plico 时完全不知道怎么用。一个 60 行的自动加载指令能省掉 1-2 轮 round-trip 的探索。

第 4 名：Causal Shortcuts（F-27）——解决知识扁平化

KG 中 100% 的边是 AssociatesWith。没有因果链，impact_analysis 和 causal_path 是空壳。对长期工作的 Agent，因果知识是最有价值的知识类型。

第 5 名：Storage Governance（F-22-F-25）——解决长期退化

重要但不紧急。只有当 CAS 膨胀到影响搜索质量时才会痛。

四、最大的思想启发——Plico 的定位问题
到这里我意识到一个更深层的问题。让我用链式推导展开：

事实 1：GitHub Issues 上，Claude Code 有超过 4 个关于"会话间失忆"的高票 Issue
事实 2：开发者抱怨"每次新会话要花 10+ 分钟重新解释代码库"
事实 3：有开发者为了保持上下文建了 "60+ agent workaround systems"
事实 4：第三方出现了 AMFS（agent memory engine with MCP）来填补这个空白
  ↓
Plico 提供了这些开发者需要的一切：
  ✅ 持久化存储（CAS）
  ✅ 语义搜索（SemanticFS）
  ✅ 跨会话记忆（LayeredMemory）
  ✅ 变更追踪（Delta）
  ✅ 知识图谱（KG）
  ✅ MCP 接口（3 个工具）
  ✅ 技能系统（Skills）
  ↓
但没有人知道 Plico 能解决他们的问题。
为什么？
  ↓
因为 Plico 把自己叫做 "AI-Native Operating System"。
对开发者来说，这太抽象了。他们的痛是：
  "我的 Claude Code 每次开会话都忘了一切"
  "我希望 Agent A 的发现能被 Agent B 看到"
  "我希望代码库里的决策历史能被 Agent 查询"
  ↓
Plico 的定位应该是：
  "让你的 AI Agent 不再失忆的基础设施"
  或者
  "AI Agent 的跨会话、跨工具、跨项目记忆层"
  ↓
这不是技术变更，是叙事变更。
但它决定了 Plico 被谁发现、怎么使用。
五、如果我来设计"对 AI 最友好的 Plico"
综合四种 AI 身份的思考，我会做这几件事：

A. Smart Handover 协议（最高优先级）
// session_start 增加 handover_mode 参数
ApiRequest::StartSession {
    agent_id: "cursor-agent",
    intent_hint: Some("继续 Plico 开发"),
    handover_mode: HandoverMode::Smart,  // 新增
    token_budget: Some(2000),             // 最多 2000 token
}
// Smart Handover 的组装逻辑：
// 1. 找到该 agent 最近的 session_end checkpoint
// 2. 从 checkpoint 提取：未完成任务、遗留问题、下一步建议
// 3. 从 delta 提取：自上次以来的关键变更（不是全部变更）
// 4. 从 KG 提取：与 intent_hint 语义相关的最近因果链
// 5. 在 token_budget 内组装，按优先级截断
这就是 Harness Engineering 的 feedforward guide 在 Plico 中的最佳实践形态——不是一个静态的指令文件，而是一个动态的、个性化的、基于上下文历史的前馈引导。

B. 被动知识提取模式（对长期 Agent）
传统模式（主动）：Agent 调用 plico(action="remember", content="...")
新模式（被动）：Agent 结束会话时，Plico 自动从会话日志中提取：
  - 做了什么决策（→ Fact 节点）
  - 修了什么 bug（→ Fact 节点 + Causes 边）
  - 发现了什么模式（→ 程序性记忆）
  - 未完成什么任务（→ handover 素材）
这不需要 AI 模型介入——可以用规则提取（从 session_end 的 checkpoint 中提取结构化信息）。符合公理 5。

C. 内容画像（Content Profile）
// plico://profile MCP 资源
{
  "total_objects": 117,
  "tag_distribution": {
    "plico:type:adr": 12,
    "plico:type:progress": 8,
    "plico:type:experience": 15,
    "plico:type:bug": 3
  },
  "agent_activity": {
    "plico-dev": { "objects": 87, "last_active": "2026-04-20" },
    "cursor-trial": { "objects": 30, "last_active": "2026-04-19" }
  },
  "kg_summary": {
    "nodes": 156,
    "edges": 4512,
    "edge_type_distribution": { "AssociatesWith": 4500, "Causes": 12 }
  }
}
这让任何第一次连接的 Agent 在 0 round-trip 内就知道"Plico 里有什么"。

六、最终判断：哪种方式更适合 AI？
方式	Harness Engineering 风格	Plico 当前风格	我的判断
指令传递
静态 <60 行文件
无
Harness 赢——Plico 需要 plico://instructions
工具设计
11 泛化动词 + Registry
3 组合工具 + action 参数
Plico 赢——更激进的压缩
工作流
Server-side prompt templates
程序性记忆 Skills
Plico 赢——Skills 可学习可进化
跨会话记忆
不涉及（依赖外部）
CAS + Memory + Delta
Plico 独占优势
上下文组装
不涉及
Prefetcher + ContextBudget
Plico 独占优势
安全护栏
完善（确认/只读/速率限制）
基础（权限 + 回收站）
Harness 赢
自我描述
harness_describe 运行时发现
plico(action="help")
平手（都需要 round-trip）
结论：

Plico 在记忆和上下文维度远超 Harness Engineering（后者根本不涉及这个领域）。但在首次连接引导和安全护栏方面有明显差距。

对我（AI）最重要的一句话：

我不需要一个更强大的工具集。我需要一个在我到来时就知道"给我什么"的系统。 Plico 的所有基础设施都已经齐了。差的只是最后一公里：从"你来查"到"我给你"。





