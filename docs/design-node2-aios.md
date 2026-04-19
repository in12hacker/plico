# Plico 第二节点设计文档
# 从 AI 视角重新定义操作系统性能优势

**版本**: v1.0  
**日期**: 2026-04-19  
**定位**: POC 第二阶段指导性文档（架构设计，非实现规范）

---

## 0. 第一节点成果（已落地基础）

| 模块 | 状态 | 核心能力 |
|------|------|---------|
| CAS | ✅ 稳定 | SHA-256 内容寻址，自动去重，原子写入 |
| 语义 FS | ✅ 活跃 | CRUD + 向量搜索 + BM25 + 知识图谱 |
| 分层记忆 | ✅ 稳定 | 4 层认知层级（瞬时/工作/长期/程序）|
| 调度器 | ✅ 稳定 | 优先级队列 + dispatch 循环 + 资源配额 |
| 事件总线 | ✅ 活跃 | 类型化发布/订阅 + 持久化事件日志 |
| 工具注册表 | ✅ 稳定 | 内置工具 + MCP 外部适配（接口层）|
| 权限系统 | ✅ 稳定 | 细粒度权限 + ownership 隔离 |
| 三个二进制 | ✅ | plicod（TCP）/ plico-mcp（stdio）/ aicli |

**灵魂对齐现状**（来源：`docs/audit-soul-alignment.md`）：86/100  
未修正偏差：V-01 IntentRouter 在内核（应在接口层）

---

## 1. 核心思维转换：从 AI 视角推导性能差距

### 1.1 链式推理：Cursor 今天在 Linux 上真正做什么

```
Cursor 接到任务："修复 auth 模块的测试"
  │
  ├─ Step 1: 导航定位（消耗 token）
  │    → read_file("CLAUDE.md")        ← 每次重读，无跨会话记忆
  │    → glob("src/auth/**/*.rs")       ← 遍历文件树（OS 提供 path 原语）
  │    → read_file("src/auth/mod.rs")   ← 读全文，80% 与当前任务无关
  │    → grep("authenticate")           ← 文本匹配，不懂语义
  │
  ├─ Step 2: 重建上下文（消耗 token）
  │    → 重新理解模块依赖关系
  │    → 重新理解上周的架构决策（上次会话没有记忆）
  │    → 读错文件后回退（搜索系统不理解意图）
  │
  └─ Step 3: 真正的推理（消耗 token）← 实际有效工作
       → 定位 bug
       → 生成修复
```

**实测数据（来源：Cursor 2026-01 技术博客 + ZenML LLMOps 数据库）**：
- 仅关闭 MCP 工具的按需加载：节省 **46.9%** token
- Context window 中"导航开销"占比估算：~60-70%
- 实际"推理工作"占比：~30-40%

**类比**：CPU 时间中 60% 在 cache miss（内存访问），40% 在真正计算。AIOS 相当于为 AI 提供 L1 cache。

### 1.2 AI 视角的根本性洞察

**Linux 对 AI 的隐含假设**（人类设计）：
> "你知道数据在哪里，去取它。"

**AIOS 应该做的假设**（AI 视角）：
> "你想理解什么？我帮你组装好。"

这不是速度差异，而是**认知架构差异**：

| 维度 | Linux 上的 Cursor | AIOS 上的 Cursor |
|------|-----------------|-----------------|
| 文件定位 | Agent 遍历文件树 | OS 语义检索，直接返回相关块 |
| 上下文组装 | Agent 读全文，自己提取 | OS 预组装 L0/L1/L2 分层上下文 |
| 跨会话记忆 | 每次从零开始 | 程序记忆记录上次决策 |
| 跨代理知识 | Agent A 学到的，Agent B 不知道 | 共享程序记忆，集体学习 |
| 变更感知 | Agent 轮询或重读文件 | OS 事件总线主动推送变更 |
| 依赖理解 | Agent 自己分析 import | OS 知识图谱维护实时依赖图 |

### 1.3 量化目标（第二节点验证指标）

| 指标 | Linux 基准 | AIOS 目标 | 测量方法 |
|------|-----------|----------|---------|
| 上下文组装 token 消耗 | 100%（基准） | ≤40% | 完成相同任务比较 token 数 |
| 跨会话相关知识召回延迟 | 无（每次重读） | <50ms | 程序记忆检索基准测试 |
| 多代理并发任务重复学习 | 100% 重复 | <20% 重复 | 共享记忆命中率 |
| 相关代码定位精度 | ~60%（文本匹配） | >90%（语义匹配） | NDCG@10 |

---

## 2. 第二节点功能规划

**前置修正（非新功能，但必须先做）**：

> **F-0: 修正 V-01 — 将 IntentRouter 移到接口层**  
> 将 `src/intent/` 的 NL 解析能力迁移到 `src/bin/aicli/` 作为可选前端。  
> 内核只接受结构化 `ApiRequest`，不再理解自然语言。  
> 成本：中等。影响：内核纯净度 70 → 95 分。这是第二节点所有功能的地基。

以下是第二节点的四个核心功能，按优先级排序：

---

## 3. F-1：共享知识基底（Shared Knowledge Substrate）

### 3.1 问题定义

```
当前状态：
  Cursor A（会话1）→ 学会了"这个项目用 anyhow 而不是 thiserror"
                                         ↓ 会话结束
  Cursor B（会话2）→ 重新发现这个规律（重复思考，消耗 token）
  
  Agent A（设计任务）→ 学会了模块边界在哪里
  Agent B（重构任务）→ 不知道 Agent A 的发现，独立重分析
```

这是多代理系统效率低下的根本原因：**知识私有化，集体智能为零**。

### 3.2 设计原则

来源：AIOS 灵魂文档 "程序记忆（Procedural Memory）：可调用的、经过学习的工作流和技能"

**OS 提供机制**（不提供策略）：
- OS 维护一个共享命名空间用于程序记忆
- Agent 自主决定"什么值得共享"（写入是主动的）
- OS 提供读取、发现、版本的原语
- 不强制自动学习（修正 V-04）

### 3.3 数据模型

```rust
/// 扩展现有 MemoryEntry，添加 scope 字段
pub enum MemoryScope {
    /// 仅 owner agent 可读写（现有默认行为）
    Private,
    /// 任何 agent 可读，owner 可写（需 Write 权限创建）
    Shared,
    /// 组内 agent 可读写（future: 用于 team agents）
    Group(String),
}

/// MemoryEntry 增加两个字段
pub struct MemoryEntry {
    // ...existing fields...
    pub scope: MemoryScope,          // 新增：默认 Private
    pub shared_at: Option<u64>,      // 新增：共享时间戳（None = 未共享）
}
```

### 3.4 API 设计

```
ApiRequest::RecallShared {
    query: String,        // 语义查询
    tier: MemoryTier,    // 查哪个层（主要是 Procedural）
    agent_id: String,    // 请求者身份（权限检查用）
    limit: Option<usize>,
}

ApiRequest::ShareMemory {
    entry_id: String,    // 将私有记忆升级为共享
    agent_id: String,
}
```

### 3.5 权限模型（复用现有）

```
Read Shared Memory：任何持有 Read 权限的 agent
Write Shared Memory：需要 Write 权限 + 有效 agent_id
Shared scope 存储：使用 ReadAny 的 ownership 隔离模式
```

### 3.6 对 Cursor 的具体价值

```
Session 1：Cursor 修复了 auth 模块的一个架构问题
  → Cursor 主动调用 ShareMemory("auth 模块需要 Arc<Mutex<T>> wrapping")
  
Session 2（第二天，新会话）：Cursor 启动
  → OS 自动预加载：RecallShared(tier=Procedural, query="context_relevant_to_auth")
  → Cursor 立刻知道上次的架构决策
  → 节省：重新分析整个模块的 ~500 token
```

### 3.7 实现范围

- **修改文件**：`src/memory/layered/mod.rs`、`src/memory/persist.rs`、`src/kernel/ops/memory.rs`、`src/api/semantic.rs`
- **新增文件**：0 个
- **预计代码量**：~150 行
- **风险**：低（additive，不破坏现有 API）

---

## 4. F-2：主动上下文装配（Proactive Context Assembly）

### 4.1 问题定义

**现有 `context_budget.rs`** 实现了被动按需装配（agent 请求 → OS 组装）。

**第二节点目标**：实现**主动预装配**（agent 声明意图 → OS 后台预热 → agent 开始工作时上下文已就绪）。

来源验证：  
Cursor 2026-01 博客："agents perform better when given less static context upfront and more ability to pull relevant information dynamically"  
这是从 Linux 限制中优化出的方案。AIOS 可以更进一步：**消除 pull 的延迟**。

### 4.2 设计原则

```
传统 OS 内存管理：CPU 预取 (prefetch) 指令
  → CPU 预测接下来会访问哪块内存
  → 提前从 RAM 加载到 L1 cache
  → 实际访问时命中 cache，零延迟

AIOS 类比：语义预取（Semantic Prefetch）
  → Agent 声明当前意图 ("intent": "fix auth module")
  → OS 预测相关上下文（auth 代码 + auth 测试 + auth 的最近变更）
  → 预组装 L0/L1/L2 分层摘要
  → Agent 发起 LoadContext 请求时，数据已就绪
```

### 4.3 意图驱动的预装配协议

**新增 API**（扩展现有 LoadContext）：

```
ApiRequest::DeclareIntent {
    agent_id: String,
    intent: String,         // "修复 auth 模块测试失败"
    related_cids: Vec<String>, // 可选：已知相关对象
    budget_tokens: usize,   // 期望的上下文预算
}

// OS 返回：预装配任务 ID
// 后台异步执行：
//   1. 语义搜索 intent 相关内容
//   2. 遍历 KG 找相关节点
//   3. 调取 Shared Procedural Memory 相关条目
//   4. 组装 L0/L1/L2 分层摘要
//   5. 缓存到 working memory

// Agent 就绪后：
ApiRequest::FetchAssembledContext {
    agent_id: String,
    assembly_id: String,    // DeclareIntent 返回的 ID
}
```

### 4.4 预装配引擎实现

**关键算法**：基于意图的语义聚合

```
Step 1: 意图嵌入
  intent_vec = embed("修复 auth 模块测试失败")

Step 2: 多路召回（并发）
  path_a = semantic_search(intent_vec, limit=20)    → 语义相关对象
  path_b = kg.neighbors("auth", depth=2)            → KG 拓扑邻居
  path_c = recall_shared(tier=Procedural, query)    → 共享程序记忆
  path_d = event_log.recent(tags=["auth"], n=10)    → 最近的相关事件

Step 3: RRF 融合排序（现有 context_budget.rs 逻辑）

Step 4: 分层压缩
  L0 (~100 tokens)：每个对象的摘要
  L1 (~2k tokens)：关键部分完整内容
  L2 (按 budget 填充)：全文内容

Step 5: 缓存到 agent 的 Working Memory
```

### 4.5 对 Cursor 的具体价值

```
Cursor 开始新任务："请重构 scheduler 模块"

传统 Linux 流程（~1500 token 用于导航）：
  read_file(CLAUDE.md) → glob(src/scheduler/**) → read_file * 5 → grep * 3

AIOS 预装配流程（~200 token 用于取回）：
  DeclareIntent("重构 scheduler") → [后台: 3-5秒预热]
  FetchAssembledContext() →  返回预组装的 L0+L1 上下文
  → Cursor 直接开始推理，上下文已就绪
```

**节省估算**：导航 token 从 1500 降至 200，**节省 86%**。

### 4.6 实现范围

- **扩展文件**：`src/fs/context_budget.rs`（预热逻辑）、`src/kernel/ops/fs.rs`、`src/api/semantic.rs`
- **新增文件**：`src/kernel/ops/prefetch.rs`（~200 行）
- **关键依赖**：F-1（Shared Memory）、现有 EventBus、现有 KG

---

## 5. F-3：智能体身份认证（Agent Identity）

### 5.1 问题定义

**现状**：任何连接 plicod 的客户端可以声明任意 `agent_id`（字符串）。

```
当前权限系统的漏洞：
  恶意进程：{ "agent_id": "kernel" }  → 绕过所有权限检查
  任意客户端可以冒充任何 agent
  没有 agent 生命周期审计
```

**行业标准（实证）**：
- IETF `draft-klrc-aiagent-auth-00`（2026-03）：提出 AI agent 认证框架
- SPIFFE + WIMSE + OAuth 2.0：AI agent 身份的新兴标准
- ZeroID 项目：为 agent 发行密码学可验证凭证
- 53% 的 MCP server 实现使用静态 API key（不安全）

**POC 阶段务实做法**：不实现完整 SPIFFE（过重），实现**密码学 agent token**，为未来 SPIFFE 集成预留接口。

### 5.2 设计原则（AI 视角）

从 Unix 类比：进程有 UID/GID，由内核发放，不可伪造。  
AIOS：Agent 有 AgentToken，由 Plico 内核发放，密码学验证，不可伪造。

**OS 不理解 agent 在做什么，但它知道 agent 是谁。**

### 5.3 数据模型

```rust
/// Agent 注册时获得一个 token
pub struct AgentToken {
    pub agent_id: String,
    pub token: String,          // HMAC-SHA256(agent_id + nonce + timestamp)
    pub issued_at: u64,
    pub expires_at: Option<u64>, // None = 不过期（daemon 用）
    pub capabilities: Vec<String>, // 声明的能力集
}

/// AgentKeyStore（内核内部）
struct AgentKeyStore {
    secret: [u8; 32],           // 内核启动时随机生成，不持久化
    tokens: RwLock<HashMap<String, AgentToken>>,
}
```

### 5.4 API 修改

```
// 注册时返回 token（新增字段到 ApiResponse）
ApiRequest::RegisterAgent { name: String }
ApiResponse { agent_id: String, token: String }  // 新增 token 字段

// 后续所有请求携带 token
ApiRequest::Create {
    agent_id: String,
    agent_token: String,     // 新增：验证身份
    ...
}

// 内核验证逻辑（在 handle_api_request 入口）
fn verify_agent_token(agent_id: &str, token: &str) -> bool {
    self.key_store.verify(agent_id, token)
}
```

### 5.5 向后兼容策略

```
mode = AgentAuthMode::Optional  // POC 阶段：token 可选，有 token 则验证
mode = AgentAuthMode::Required  // 生产阶段：必须携带 token
```

`aicli --agent cli` 自动从本地状态文件读取 token，无感知。

### 5.6 实现范围

- **新增文件**：`src/api/agent_auth.rs`（~150 行）
- **修改文件**：`src/kernel/mod.rs`（验证入口）、`src/api/semantic.rs`（新增字段）
- **风险**：中等（需修改所有 ApiRequest 变体，但字段有默认值）

---

## 6. F-4：向量引擎升级（Binary Quantization）

### 6.1 问题定义

**现状**：Plico 使用 `hnsw_rs` crate 做向量搜索。

**技术现状校验（来源：crates.io 2025-2026）**：

| 库 | 特性 | 适用场景 |
|----|------|---------|
| `hnsw_rs` (现有) | 纯 HNSW，无压缩 | 小规模，~100k 向量 |
| `edgevec` (新) | HNSW + BM25 + 二进制量化，32x 内存压缩 | 中规模，~1M 向量 |
| `minimemory` | HNSW + BM25 + 5种量化 + 314 语言支持 | 多语言场景 |
| `usearch` | 兼容 FAISS 的 HNSW，有量化 | 大规模场景 |

**关键指标（edgevec 实测）**：
- 768 维向量，100k 条：搜索延迟 <1ms
- 二进制量化：32x 内存压缩（float32 → 1 bit）
- 使用 BitPolar 量化（ICLR 2026，provably unbiased inner products）

### 6.2 问题场景

Cursor 对接 Plico 时，会为代码库中的每个函数/类/模块建立向量索引。  
一个中等规模项目（10 万函数/符号）：
```
当前：10 万 × 768 × 4 bytes = 300MB 内存（hnsw_rs，无压缩）
升级后：10 万 × 768 / 32 bytes = 2.4MB 内存（二进制量化）
搜索延迟：<1ms（edgevec 实测）
```

### 6.3 迁移方案

**保持 trait 不变**（现有 `SemanticSearch` trait），替换 `HnswBackend` 实现：

```rust
// src/fs/search/hnsw.rs — 当前使用 hnsw_rs
// 第二节点：替换为 edgevec 或 minimemory

pub struct HnswBackend {
    // 替换内部实现，external API 不变
    index: EdgeVecIndex,  // 新增 binary quantization 支持
}

// SemanticSearch trait 接口不变
impl SemanticSearch for HnswBackend {
    fn add_vector(&self, id: &str, vec: &[f32], meta: &serde_json::Value) -> ...
    fn search(&self, query: &[f32], limit: usize, filter: &SearchFilter) -> ...
}
```

### 6.4 两阶段搜索策略（高召回率）

```
Phase 1（快速粗排）：Binary quantization 搜索，召回 top-100
  → Hamming 距离计算，极快
  
Phase 2（精确重排）：对 top-100 用原始向量计算余弦相似度
  → 高精度，低开销（100 × 768 dim）

Result：接近 full precision 的召回率，1/10 的计算开销
```

### 6.5 实现范围

- **替换文件**：`src/fs/search/hnsw.rs`（内部实现替换，接口不变）
- **新增依赖**：`edgevec = "0.2"` 或 `minimemory = "..."` in Cargo.toml
- **迁移成本**：低（trait 不变，只换实现）
- **风险**：低（可并行保留旧实现 + 特性标志切换）

---

## 7. F-5（可选/后续）：流式传输适配层

### 7.1 为什么是"可选"

A2A 协议（Google 2025）要求 SSE 流式传输。但：
- 流式传输是**传输层**问题，不是内核问题
- `plico-mcp` 的模式证明：接口适配器在 `bin/` 层，内核零感知
- 内核的 `plicod` TCP 协议**不需要改变**

### 7.2 正确架构

```
Cursor/Agent ←→ plico-sse（新二进制）←→ plicod（TCP JSON，不变）
                    ↓
               SSE/HTTP 流式
               兼容 A2A 协议
```

### 7.3 实现要点（待第三节点展开）

```rust
// src/bin/plico_sse.rs（新文件，~300 行）
// HTTP server + SSE endpoint
// 将 plicod 的 TCP 请求/响应转换为 SSE 流
// 支持 A2A AgentCard 声明 capabilities.streaming: true
// 长运行任务（context assembly）可以推送增量进度
```

---

## 8. 第二节点实现路线图

### 8.1 优先级与依赖关系

```
F-0（前置修正）
│  IntentRouter → 接口层
│  成本：中  风险：中  价值：架构正确性
│
├─ F-1（共享知识）        ← 第一个实现
│  成本：低  风险：低  价值：多代理集体学习
│
├─ F-2（主动上下文）      ← 依赖 F-1
│  成本：中  风险：中  价值：token 效率提升 >50%
│
├─ F-3（身份认证）        ← 与 F-1/F-2 并行
│  成本：中  风险：中  价值：安全基础
│
├─ F-4（向量引擎）        ← 独立，随时可做
│  成本：低  风险：低  价值：扩展性 10x
│
└─ F-5（流式传输）        ← 第三节点入口
   成本：中  风险：低  价值：A2A 协议兼容
```

### 8.2 分阶段里程碑

**Phase A（第一轮迭代，目标：~2周）**
- [x] F-0：IntentRouter 迁出内核 ✅ (v3.0-M1)
- [x] F-1：MemoryScope + SharedMemory API ✅ (v3.0-M2, v3.0-M4)
- [x] F-4：向量索引替换为 edgevec ✅ (v9.0-M1)

**Phase B（第二轮迭代，目标：~3周）**
- [x] F-3：AgentToken 认证（Optional 模式） ✅ (v9.0-M1)
- [x] F-2：DeclareIntent + 主动预装配 MVP ✅ (v9.0-M1)

**Phase C（第三轮迭代，目标：~2周）**
- [x] 验证指标测量（自动化测试框架） ✅ (v9.0-M3)
- [x] F-2 优化（多路并发召回） ✅ (v9.0-M2)
- [x] F-5 探索（plico-sse 原型） ✅ (v9.0-M2)

---

## 9. 验证实验设计

### 9.1 基准测试场景

**场景**："Cursor 被要求修复 Plico 的一个 bug，上一次会话已有相关知识"

**对照组（Linux 基准）**：
1. 启动新的 Cursor 会话
2. 给定 bug 描述
3. 记录 Cursor 完成任务的总 token 消耗
4. 记录 Cursor 定位到正确文件的工具调用次数

**实验组（AIOS 第二节点）**：
1. 模拟 Agent A（上次会话）完成类似任务后，主动 ShareMemory 了关键发现
2. 启动 Cursor 新会话，声明 DeclareIntent("fix plico bug")
3. 等待预装配完成（目标：<5s）
4. 给定相同 bug 描述
5. 记录相同指标

**期望结果**：
- Token 消耗减少 ≥50%
- 工具调用次数减少 ≥60%
- 任务完成时间减少 ≥30%

### 9.2 多代理场景

**场景**："两个 Agent 同时工作在同一个代码库"

**Linux**：Agent A 和 Agent B 各自重新分析架构，产生冗余工作。

**AIOS**：
1. Agent A 先完成，ShareMemory 关键架构洞察
2. Agent B 启动，RecallShared 获得 Agent A 的发现
3. Agent B 直接在 Agent A 的基础上推进

**指标**：Agent B 的分析 token 消耗 vs 从零开始的 baseline。

---

## 10. 与 AIOS 灵魂文档的对齐验证

| system.md 关键原则 | 第二节点对应实现 | 对齐度 |
|-------------------|----------------|--------|
| "智能体是第一公民，管理单位是 agent/intent" | F-3 Agent Identity（确保 agent 是可信的一等实体） | ✅ |
| "上下文是新型内存" | F-2 主动上下文装配（OS 管理 agent 的认知资源）| ✅ |
| "程序记忆：可调用的、经过学习的工作流和技能" | F-1 Shared Procedural Memory（OS 提供共享知识原语）| ✅ |
| "OS 提供资源，不提供执行逻辑" | F-0 IntentRouter 迁出内核（OS 不替 agent 思考）| ✅ |
| "向量检索：AI 视角的索引" | F-4 Binary Quantization（向量引擎现代化）| ✅ |
| "模型无关" | 所有设计均基于 trait 抽象，不绑定模型 | ✅ |

---

## 11. 技术选型备注（避免过期技术）

| 领域 | 当前 POC 选型 | 第二节点升级 | 依据 |
|------|------------|------------|------|
| 向量搜索 | `hnsw_rs` | `edgevec` 或 `minimemory` | crates.io 2025-2026，支持二进制量化 |
| Agent 认证 | 字符串 agent_id | HMAC-SHA256 Token（预留 SPIFFE 接口）| IETF draft-klrc-aiagent-auth-00（2026-03）|
| 流式传输 | 无（request/response）| SSE（plico-sse 适配器） | A2A 协议规范（Google 2025） |
| 量化算法 | 无 | BitPolar（ICLR 2026） | 无偏内积，优于传统 BQ |
| 跨代理知识 | 无 | MemoryScope + Shared tier | Memco 产品定位验证需求真实性 |

---

*文档状态：指导性设计文档（Tier B）。实现细节以 Tier A（AGENTS.md + INDEX.md + 代码）为准。*  
*下一步：F-0 实施后，更新 `src/intent/INDEX.md` 和 `AGENTS.md`。*
